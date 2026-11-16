//! Canal de commande HID (interface 1, rapports de 1024 octets sans report ID).
//!
//! Le firmware ne traite l'entrée HID qu'environ une fois toutes les 4,1 s et
//! jette les autres rapports : chaque requête est donc **répétée** toutes les
//! 100 ms jusqu'à réception d'un rapport. `conn` renvoie le JSON d'état
//! (`AckNumber=2`) ; les commandes sont acquittées par `AckNumber = SeqNumber + 1`.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tracing::{debug, trace};

use crate::error::{Error, Result};
use crate::{device, frame};

/// Délai maximal d'attente d'une réponse (deux périodes du firmware).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(9);
const REPEAT_PERIOD: Duration = Duration::from_millis(100);

/// État renvoyé par le firmware (`POST conn`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Status {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub sn: String,
    #[serde(default)]
    pub version: Version,
    /// Luminosité, de 0 à 100.
    #[serde(default)]
    pub brightness: u32,
    /// Rotation de la dalle déclarée par le firmware, en degrés.
    #[serde(default)]
    pub degree: u32,
    #[serde(default, rename = "bootFinish")]
    pub boot_finish: u32,
    #[serde(default, rename = "realtimeDisplay")]
    pub realtime_display: u32,
    /// 1 quand l'écran affiche le flux USB, 0 quand il joue son animation interne.
    #[serde(default, rename = "extendedDisplay")]
    pub extended_display: u32,
}

/// Versions déclarées par le firmware.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Version {
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub firmware: String,
    #[serde(default)]
    pub sdk: String,
    #[serde(default)]
    pub hardware: String,
}

impl Status {
    /// Vrai si l'écran est en mode « écran étendu ».
    pub fn is_extended(&self) -> bool {
        self.extended_display != 0
    }

    /// Analyse la réponse texte du firmware (`1 200\r\n…\r\n\r\n{json}`).
    pub fn parse(reply: &str) -> Option<Self> {
        let (_, json) = reply.split_once("\r\n\r\n")?;
        // Le firmware annonce une longueur fixe : on ne lit que le premier objet
        // JSON et on ignore ce qui suit (résidus de la réponse précédente).
        serde_json::Deserializer::from_str(json)
            .into_iter::<Self>()
            .next()?
            .ok()
    }
}

/// Canal de commande HID de l'écran.
pub struct HidControl {
    file: File,
    path: PathBuf,
    pid: u16,
}

impl HidControl {
    /// Cherche le nœud hidraw de l'écran (identifiants acceptés par
    /// [`device::policy`], même choix que le canal USB) ; renvoie aussi son PID.
    pub fn find() -> Option<(PathBuf, u16)> {
        let entries: Vec<_> = std::fs::read_dir("/sys/class/hidraw")
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let uevent = std::fs::read_to_string(entry.path().join("device/uevent")).ok()?;
                Some((entry.file_name(), device::parse_hid_id(&uevent)?))
            })
            .collect();
        let ids: Vec<(u16, u16)> = entries.iter().map(|(_, id)| *id).collect();
        let index = device::select(&ids, device::policy())?;
        let (name, (_, pid)) = &entries[index];
        Some((Path::new("/dev").join(name), *pid))
    }

    /// Identifiant USB du produit (`0x7881` sauf option explicite).
    pub fn pid(&self) -> u16 {
        self.pid
    }

    /// Ouvre le canal HID de l'écran.
    pub fn open() -> Result<Self> {
        let (path, pid) = Self::find().ok_or(Error::HidNotFound)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| match e.kind() {
                ErrorKind::PermissionDenied => Error::PermissionDenied(path.clone()),
                _ => Error::Io(e),
            })?;
        Ok(Self { file, path, pid })
    }

    fn readable(&self, timeout: Duration) -> Result<bool> {
        let mut fds = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: `fds` est une structure valide pour la durée de l'appel.
        let n = unsafe { libc::poll(&mut fds, 1, ms) };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(err.into());
        }
        Ok(n > 0 && fds.revents & libc::POLLIN != 0)
    }

    /// Répète la requête jusqu'à recevoir un rapport accepté par `accept` ;
    /// renvoie son texte, ou `None` à l'expiration du délai.
    ///
    /// Le firmware garde en file quelques exemplaires de la requête précédente :
    /// le premier rapport reçu peut donc répondre à celle-ci plutôt qu'à la
    /// requête en cours, d'où le filtrage.
    pub fn request(
        &mut self,
        text: &str,
        timeout: Duration,
        accept: impl Fn(&str) -> bool,
    ) -> Result<Option<String>> {
        let report = frame::by_report(text.as_bytes())
            .ok_or(Error::OutOfRange("requête HID trop longue"))?;
        let mut buf = [0u8; 2048];
        while self.readable(Duration::ZERO)? {
            let _ = self.file.read(&mut buf)?; // purge des rapports en attente
        }
        let start = Instant::now();
        let mut writes = 0u32;
        while start.elapsed() < timeout {
            self.file.write_all(&report)?;
            writes += 1;
            if !self.readable(REPEAT_PERIOD)? {
                continue;
            }
            let n = self.file.read(&mut buf)?;
            let reply = frame::by_deframe(&buf[..n])
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            let accepted = accept(&reply);
            debug!(
                path = %self.path.display(),
                writes,
                elapsed_ms = start.elapsed().as_millis() as u64,
                accepted,
                "rapport HID reçu"
            );
            trace!(?reply);
            if accepted {
                return Ok(Some(reply));
            }
        }
        debug!(writes, "pas de réponse HID");
        Ok(None)
    }

    /// Envoie une commande ; renvoie vrai si le firmware l'a acquittée
    /// (`AckNumber = SeqNumber + 1`).
    fn command(&mut self, cmd: &str, seq: u32, json: &str) -> Result<bool> {
        let reply = self.request(&frame::post_request(cmd, seq, json), DEFAULT_TIMEOUT, |r| {
            ack_number(r) == Some(seq + 1)
        })?;
        Ok(reply.is_some())
    }

    fn conn(&mut self) -> Result<Option<Status>> {
        let epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let reply = self.request(&frame::conn_request(epoch_ms), DEFAULT_TIMEOUT, |r| {
            Status::parse(r).is_some()
        })?;
        Ok(reply.as_deref().and_then(Status::parse))
    }

    /// Lit l'état du firmware (`POST conn`).
    pub fn status(&mut self) -> Result<Option<Status>> {
        self.conn()
    }

    /// État relu après une commande. Si elle n'a pas été acquittée, le premier
    /// état reçu peut dater d'avant la commande : on en attend alors un second.
    fn status_after(&mut self, acked: bool) -> Result<Option<Status>> {
        let first = self.conn()?;
        if acked {
            Ok(first)
        } else {
            Ok(self.conn()?.or(first))
        }
    }

    /// Passe l'écran en mode « écran étendu » puis relit l'état.
    pub fn extend(&mut self) -> Result<Option<Status>> {
        let acked = self.command("extendedDisplay", 110, r#"{"enable":true}"#)?;
        self.status_after(acked)
    }

    /// Demande au firmware de quitter le mode « écran étendu » (retour à son
    /// animation interne) puis relit l'état.
    pub fn restore(&mut self) -> Result<Option<Status>> {
        let acked = self.command("extendedDisplay", 110, r#"{"enable":false}"#)?;
        self.status_after(acked)
    }

    /// Règle la luminosité (0 à 100) puis relit l'état.
    pub fn set_brightness(&mut self, level: u8) -> Result<Option<Status>> {
        if level > 100 {
            return Err(Error::OutOfRange("luminosité attendue entre 0 et 100"));
        }
        let acked = self.command("brightness", 100, &format!(r#"{{"value":{level}}}"#))?;
        self.status_after(acked)
    }
}

/// Extrait `AckNumber` de l'en-tête d'une réponse du firmware.
pub fn ack_number(reply: &str) -> Option<u32> {
    reply
        .lines()
        .find_map(|l| l.strip_prefix("AckNumber="))
        .and_then(|v| v.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = "1 200\r\nAckNumber=2\r\nContentType=json\r\nContentLength=279\r\n\r\n\
        {\"OS\":\"RTOS\",\"Manufacturer\":\"MSI\",\"model\":\"V530-9.6 Lightning\",\
        \"version\":{\"app\":\"V1.0.1\",\"firmware\":\"V530-9.6 Lightning_20251212v01\",\
        \"sdk\":\"V1.2.0\",\"hardware\":\"V2.0\"},\"brightness\":100,\"degree\":90,\
        \"sn\":\"SN000000000000000000\",\"bootFinish\":1,\"realtimeDisplay\":0,\
        \"extendedDisplay\":1}";

    #[test]
    fn parses_real_status_reply() {
        let st = Status::parse(REPLY).unwrap();
        assert_eq!(st.model, "V530-9.6 Lightning");
        assert_eq!(st.version.firmware, "V530-9.6 Lightning_20251212v01");
        assert_eq!(st.brightness, 100);
        assert_eq!(st.degree, 90);
        assert!(st.is_extended());
    }

    #[test]
    fn ignores_trailing_bytes_after_the_json_object() {
        let reply = format!("{REPLY}}}\u{0}\u{0}");
        assert!(Status::parse(&reply).unwrap().is_extended());
    }

    #[test]
    fn ack_numbers() {
        assert_eq!(ack_number(REPLY), Some(2));
        assert_eq!(ack_number("1 200\r\nAckNumber=101\r\n\r\n"), Some(101));
        assert_eq!(ack_number("1 200\r\n\r\n"), None);
        // Un simple acquittement n'est pas un état.
        assert!(Status::parse("1 200\r\nAckNumber=101\r\n\r\n").is_none());
    }

    #[test]
    fn rejects_replies_without_json() {
        assert!(Status::parse("1 200\r\nAckNumber=2").is_none());
        assert!(Status::parse("1 200\r\n\r\nnot json").is_none());
    }
}
