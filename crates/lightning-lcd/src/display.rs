//! Session d'affichage : bascule en mode étendu, authentification, envoi d'images.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
use rusb::{DeviceHandle, GlobalContext, UsbContext};
use tracing::{debug, info, warn};

use crate::error::{Error, Result};
use crate::frame::{self, AUTH_DEV_MAGIC, AUTH_HOST_MAGIC, FORMAT_JPEG, FRAME_MAGIC};
use crate::hid::HidControl;
use crate::{HEIGHT, PID, VID, WIDTH, rsa};

const IFACE: u8 = 0;
const EP_OUT: u8 = 0x01;
const EP_IN: u8 = 0x81;
const CHUNK: usize = 64 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const AUTH_TIMEOUT: Duration = Duration::from_secs(3);
/// Délai minimal entre deux tentatives après une déconnexion de l'écran.
const RETRY_DELAY: Duration = Duration::from_secs(3);

/// Mode d'affichage constaté à l'ouverture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Le firmware confirme le mode « écran étendu » : le flux USB est affiché.
    Extended,
    /// Le firmware a répondu mais reste sur son animation interne.
    Standalone,
    /// Pas de réponse HID (ou bascule désactivée) : état inconnu.
    Unknown,
}

/// Options d'ouverture d'une session.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    /// Basculer l'écran en mode étendu via HID si nécessaire.
    pub extend: bool,
    /// Durée maximale d'attente de l'énumération USB de l'écran.
    pub wait: Duration,
    /// Nombre de tentatives d'ouverture de session.
    pub retries: u32,
    /// Qualité JPEG (1 à 100) utilisée par [`Display::show_rgb`].
    pub jpeg_quality: u8,
    /// Rouvrir la session et renvoyer l'image quand un envoi échoue. À désactiver
    /// si l'appelant gère lui-même la reconnexion (il doit alors rouvrir un
    /// [`Display`] après une erreur).
    pub auto_reconnect: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            extend: true,
            wait: Duration::from_secs(15),
            retries: 4,
            jpeg_quality: 85,
            auto_reconnect: true,
        }
    }
}

/// Session d'affichage sur l'écran.
///
/// La session se rétablit d'elle-même si l'écran se réénumère : un envoi qui
/// échoue provoque une reconnexion puis un second essai.
pub struct Display {
    /// Verrou consultatif : un seul processus pilote l'écran à la fois.
    _lock: std::fs::File,
    handle: Option<DeviceHandle<GlobalContext>>,
    opts: OpenOptions,
    mode: Mode,
    width: u32,
    height: u32,
    fps: u32,
    frame_id: u16,
    fresh: bool,
    reconnects: u64,
}

impl Display {
    /// Ouvre une session avec les options par défaut.
    pub fn open() -> Result<Self> {
        Self::open_with(OpenOptions::default())
    }

    /// Ouvre une session.
    ///
    /// Séquence validée sur l'écran réel :
    /// 1. HID : état puis `extendedDisplay` si nécessaire ;
    /// 2. bulk : claim de l'interface 0 et authentification RSA, **sans aucune
    ///    requête vendor** (elles font réénumérer l'écran).
    pub fn open_with(opts: OpenOptions) -> Result<Self> {
        let mut display = Self {
            _lock: acquire_lock()?,
            handle: None,
            opts,
            mode: Mode::Unknown,
            width: WIDTH,
            height: HEIGHT,
            fps: 60,
            frame_id: 0,
            fresh: false,
            reconnects: 0,
        };
        if display.opts.extend {
            match display.ensure_extended() {
                Ok(mode) => display.mode = mode,
                Err(e) => warn!("bascule HID impossible : {e}"),
            }
        }
        display.connect()?;
        Ok(display)
    }

    /// Mode constaté à l'ouverture.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Largeur du mode d'affichage, en pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Hauteur du mode d'affichage, en pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Fréquence de rafraîchissement annoncée par l'écran.
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// Nombre de reconnexions effectuées depuis l'ouverture.
    pub fn reconnects(&self) -> u64 {
        self.reconnects
    }

    fn ensure_extended(&mut self) -> Result<Mode> {
        let mut hid = HidControl::open()?;
        let Some(status) = hid.status()? else {
            warn!("pas de réponse HID à conn : état de l'écran inconnu");
            return Ok(Mode::Unknown);
        };
        if status.is_extended() {
            debug!("écran déjà en mode étendu");
            return Ok(Mode::Extended);
        }
        info!("écran en mode autonome : bascule en mode étendu");
        // Aucune requête vendor ici : même en mode autonome, la lecture des
        // paramètres (0xC0, requête 0) peut faire réénumérer l'écran, ce qui
        // casse le canal HID avant la bascule. Le mode 1280×800@60 est fixe.
        match hid.extend()? {
            Some(st) if st.is_extended() => Ok(Mode::Extended),
            Some(_) => {
                warn!("le firmware n'a pas confirmé le mode étendu");
                Ok(Mode::Standalone)
            }
            None => Ok(Mode::Unknown),
        }
    }

    fn connect(&mut self) -> Result<()> {
        self.handle = None;
        let mut last = Error::DeviceNotFound;
        for attempt in 1..=self.opts.retries.max(1) {
            match self.try_connect() {
                Ok(()) => return Ok(()),
                Err(e @ (Error::UsbPermissionDenied | Error::DeviceNotFound)) => return Err(e),
                Err(e) => {
                    warn!(attempt, "session interrompue ({e}), nouvelle tentative");
                    last = e;
                    std::thread::sleep(RETRY_DELAY);
                }
            }
        }
        warn!("dernière erreur : {last}");
        Err(Error::SessionFailed(self.opts.retries.max(1)))
    }

    fn try_connect(&mut self) -> Result<()> {
        let (handle, key) = find_device(self.opts.wait)?;
        handle.claim_interface(IFACE).map_err(map_access)?;
        self.fresh = false;
        if auth_cache_read().as_deref() == Some(key.as_str()) {
            debug!(key, "session déjà authentifiée pour cette énumération USB");
        } else {
            match authenticate(&handle) {
                Ok(()) => self.fresh = true,
                // En mode étendu, l'écran n'accepte l'authentification qu'une fois
                // par énumération : une seconde tentative reste sans réponse, mais
                // les images continuent d'être acceptées.
                Err(Error::Usb(rusb::Error::Timeout)) => {
                    warn!("authentification sans réponse : écran supposé déjà authentifié");
                }
                Err(e) => return Err(e),
            }
            auth_cache_write(&key);
        }
        self.frame_id = 0;
        self.handle = Some(handle);
        Ok(())
    }

    fn send(&mut self, jpeg: &[u8]) -> Result<()> {
        let handle = self.handle.as_ref().ok_or(Error::DeviceNotFound)?;
        let len = u32::try_from(jpeg.len()).map_err(|_| Error::OutOfRange("JPEG trop gros"))?;
        let header = frame::bulk_header(FRAME_MAGIC, len, self.frame_id, FORMAT_JPEG);
        write_all(handle, &header)?;
        for chunk in jpeg.chunks(CHUNK) {
            write_all(handle, chunk)?;
        }
        self.frame_id = self.frame_id.wrapping_add(1);
        Ok(())
    }

    /// Affiche une image JPEG (baseline, 4:2:0, à la taille de l'écran).
    ///
    /// En cas d'erreur USB (réénumération de l'écran), la session est rouverte
    /// et l'image renvoyée une fois.
    pub fn show_jpeg(&mut self, jpeg: &[u8]) -> Result<()> {
        if self.handle.is_some() {
            match self.send(jpeg) {
                Ok(()) => return Ok(()),
                Err(e) if !self.opts.auto_reconnect => {
                    self.handle = None;
                    return Err(e);
                }
                Err(e) => warn!("envoi interrompu ({e}), reconnexion"),
            }
            self.handle = None;
            std::thread::sleep(RETRY_DELAY);
        }
        if !self.opts.auto_reconnect {
            return Err(Error::DeviceNotFound);
        }
        self.reconnects += 1;
        self.connect()?;
        self.send(jpeg)
    }

    /// Affiche une image fixe de façon robuste.
    ///
    /// Observé : juste après une énumération, une session d'une seule image
    /// n'empêche pas l'écran de se réénumérer au bout de 13 s, alors qu'un flux
    /// de quelques dizaines d'images si. Après une authentification fraîche,
    /// l'image est donc réémise pendant 1,5 s.
    pub fn show_still(&mut self, jpeg: &[u8]) -> Result<()> {
        self.show_jpeg(jpeg)?;
        if self.fresh {
            let end = Instant::now() + Duration::from_millis(1500);
            while Instant::now() < end {
                std::thread::sleep(Duration::from_millis(33));
                self.show_jpeg(jpeg)?;
            }
            self.fresh = false;
        }
        Ok(())
    }

    /// Encode une image RGB (3 octets par pixel) en JPEG pour l'écran.
    pub fn encode_rgb(&self, rgb: &[u8]) -> Result<Vec<u8>> {
        encode_jpeg(rgb, self.width, self.height, 3, self.opts.jpeg_quality)
    }

    /// Encode une image RGBA (4 octets par pixel, alpha ignoré) en JPEG pour l'écran.
    pub fn encode_rgba(&self, rgba: &[u8]) -> Result<Vec<u8>> {
        encode_jpeg(rgba, self.width, self.height, 4, self.opts.jpeg_quality)
    }

    /// Affiche une image RGB de la taille de l'écran.
    pub fn show_rgb(&mut self, rgb: &[u8]) -> Result<()> {
        let jpeg = self.encode_rgb(rgb)?;
        self.show_jpeg(&jpeg)
    }

    /// Affiche une image RGBA de la taille de l'écran (alpha ignoré).
    pub fn show_rgba(&mut self, rgba: &[u8]) -> Result<()> {
        let jpeg = self.encode_rgba(rgba)?;
        self.show_jpeg(&jpeg)
    }
}

/// Encode des pixels RGB (`channels` = 3) ou RGBA (`channels` = 4) en JPEG
/// baseline 4:2:0, seul format validé avec le décodeur de l'écran.
pub fn encode_jpeg(
    pixels: &[u8],
    width: u32,
    height: u32,
    channels: usize,
    quality: u8,
) -> Result<Vec<u8>> {
    let expected = width as usize * height as usize * channels;
    if pixels.len() != expected {
        return Err(Error::BadFrameSize {
            got: pixels.len(),
            expected,
            width,
            height,
        });
    }
    let (w, h) = match (u16::try_from(width), u16::try_from(height)) {
        (Ok(w), Ok(h)) => (w, h),
        _ => return Err(Error::OutOfRange("dimensions d'image")),
    };
    let color = match channels {
        3 => ColorType::Rgb,
        4 => ColorType::Rgba,
        _ => return Err(Error::OutOfRange("nombre de canaux")),
    };
    let mut out = Vec::with_capacity(96 * 1024);
    let mut encoder = Encoder::new(&mut out, quality.clamp(1, 100));
    encoder.set_sampling_factor(SamplingFactor::F_2_2);
    encoder
        .encode(pixels, w, h, color)
        .map_err(|e| Error::Jpeg(e.to_string()))?;
    Ok(out)
}

fn map_access(e: rusb::Error) -> Error {
    match e {
        rusb::Error::Access => Error::UsbPermissionDenied,
        other => Error::Usb(other),
    }
}

/// Attend l'écran et l'ouvre ; renvoie aussi la clé `bus:adresse` de l'énumération.
fn find_device(wait: Duration) -> Result<(DeviceHandle<GlobalContext>, String)> {
    let deadline = Instant::now() + wait;
    loop {
        for device in GlobalContext::default().devices()?.iter() {
            let Ok(desc) = device.device_descriptor() else {
                continue;
            };
            if desc.vendor_id() == VID && desc.product_id() == PID {
                let key = format!("{}:{}", device.bus_number(), device.address());
                match device.open() {
                    Ok(handle) => return Ok((handle, key)),
                    Err(rusb::Error::Access) => return Err(Error::UsbPermissionDenied),
                    // Périphérique en cours de réénumération : on réessaie.
                    Err(e) => debug!("ouverture USB impossible : {e}"),
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(Error::DeviceNotFound);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn write_all(handle: &DeviceHandle<GlobalContext>, data: &[u8]) -> Result<()> {
    let mut sent = 0;
    while sent < data.len() {
        let n = handle.write_bulk(EP_OUT, &data[sent..], IO_TIMEOUT)?;
        if n == 0 {
            return Err(Error::Usb(rusb::Error::Io));
        }
        sent += n;
    }
    Ok(())
}

/// Authentification RSA en deux phases.
fn authenticate(handle: &DeviceHandle<GlobalContext>) -> Result<()> {
    let block_len = rsa::BLOCK_LEN as u32;
    let mut buf = [0u8; 512];

    // Phase 1 : l'écran prouve qu'il détient la clé privée en déchiffrant un défi.
    let mut challenge = [0u8; 32];
    rsa::random_bytes(&mut challenge)?;
    write_all(handle, &frame::bulk_header(AUTH_DEV_MAGIC, block_len, 0, 0))?;
    write_all(handle, &rsa::encrypt(&challenge)?)?;
    let n = handle.read_bulk(EP_IN, &mut buf, AUTH_TIMEOUT)?;
    if n < challenge.len() || buf[..challenge.len()] != challenge {
        return Err(Error::Auth("l'écran n'a pas renvoyé le défi"));
    }

    // Phase 2 : l'écran envoie un bloc signé, l'hôte en renvoie le contenu.
    write_all(
        handle,
        &frame::bulk_header(AUTH_HOST_MAGIC, block_len, 0, 0),
    )?;
    let n = handle.read_bulk(EP_IN, &mut buf, AUTH_TIMEOUT)?;
    let plain = rsa::public_recover(&buf[..n])?;
    write_all(handle, &plain)?;
    debug!("authentification RSA réussie");
    Ok(())
}

fn runtime_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into()))
}

/// Chemin du fichier de verrou partagé par tous les processus qui pilotent l'écran.
pub fn lock_path() -> PathBuf {
    runtime_dir().join("msi-lightning-lcd.lock")
}

/// Prend le verrou exclusif (libéré à la fermeture du fichier, donc à la fin
/// du processus quoi qu'il arrive).
fn acquire_lock() -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path())?;
    // SAFETY: le descripteur est valide tant que `file` existe.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let err = std::io::Error::last_os_error();
        return Err(match err.kind() {
            std::io::ErrorKind::WouldBlock => Error::Busy,
            _ => Error::Io(err),
        });
    }
    Ok(file)
}

fn auth_cache_path() -> PathBuf {
    runtime_dir().join("msi-lightning-lcd.auth")
}

fn auth_cache_read() -> Option<String> {
    std::fs::read_to_string(auth_cache_path())
        .ok()
        .map(|s| s.trim().to_owned())
}

fn auth_cache_write(key: &str) {
    if let Err(e) = std::fs::write(auth_cache_path(), key) {
        debug!("cache d'authentification non écrit : {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_is_baseline_420_at_requested_size() {
        let rgb = vec![200u8; 64 * 32 * 3];
        let jpeg = encode_jpeg(&rgb, 64, 32, 3, 85).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xFF, 0xD9]);
        // SOF0 (baseline) : hauteur, largeur, 3 composantes, Y échantillonné 2x2.
        let sof = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        assert_eq!(&jpeg[sof + 5..sof + 9], &[0, 32, 0, 64]);
        assert_eq!(jpeg[sof + 9], 3);
        assert_eq!(jpeg[sof + 11], 0x22);
        // Pas de JPEG progressif.
        assert!(!jpeg.windows(2).any(|w| w == [0xFF, 0xC2]));
    }

    #[test]
    fn rgba_input_is_accepted() {
        let rgba = vec![10u8; 16 * 16 * 4];
        assert!(encode_jpeg(&rgba, 16, 16, 4, 80).is_ok());
    }

    #[test]
    fn wrong_buffer_size_is_reported() {
        let err = encode_jpeg(&[0u8; 10], 1280, 800, 3, 85).unwrap_err();
        assert!(matches!(
            err,
            Error::BadFrameSize {
                expected: 3_072_000,
                ..
            }
        ));
    }
}
