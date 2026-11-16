//! Choix du périphérique : identifiants USB acceptés.
//!
//! Seul `0db0:7881` (MSI RTX 5090 Lightning Z) a été testé. Le pilote Windows
//! déclare aussi les identifiants `0db0:7880` à `0db0:7919` : ils ne sont
//! acceptés que sur demande explicite ([`DevicePolicy::AnySupported`]), réglée
//! une fois au démarrage du programme par [`set_policy`]. La même politique
//! sert au canal USB (bulk) et au canal HID, qui désignent ainsi le même écran.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::{PID, VID};

/// Vrai pour les 40 identifiants déclarés par le pilote Windows (non testés,
/// sauf [`PID`]) : `7880` à `7889`, `7890` à `7899`, `7900` à `7909` et `7910`
/// à `7919`. Ce sont des identifiants hexadécimaux qui n'emploient que des
/// chiffres décimaux : `788a` ou `790f`, par exemple, n'en font pas partie.
pub fn is_declared_pid(pid: u16) -> bool {
    let (high, tens, units) = (pid >> 8, (pid >> 4) & 0xF, pid & 0xF);
    units <= 9 && matches!((high, tens), (0x78, 8 | 9) | (0x79, 0 | 1))
}

/// Identifiants USB acceptés.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DevicePolicy {
    /// `0db0:7881` uniquement (comportement par défaut).
    #[default]
    Tested,
    /// Tous les identifiants déclarés par le pilote Windows ; `7881` reste préféré.
    AnySupported,
}

impl DevicePolicy {
    /// Vrai si l'identifiant `vid:pid` est accepté.
    pub fn accepts(self, vid: u16, pid: u16) -> bool {
        vid == VID
            && match self {
                DevicePolicy::Tested => pid == PID,
                DevicePolicy::AnySupported => is_declared_pid(pid),
            }
    }
}

static POLICY: AtomicU8 = AtomicU8::new(0);

/// Règle la politique du processus (à appeler avant d'ouvrir l'écran).
pub fn set_policy(policy: DevicePolicy) {
    POLICY.store(
        match policy {
            DevicePolicy::Tested => 0,
            DevicePolicy::AnySupported => 1,
        },
        Ordering::SeqCst,
    );
}

/// Politique en vigueur.
pub fn policy() -> DevicePolicy {
    match POLICY.load(Ordering::SeqCst) {
        0 => DevicePolicy::Tested,
        _ => DevicePolicy::AnySupported,
    }
}

/// Choisit un écran parmi des identifiants `(vid, pid)` : `7881` d'abord, sinon
/// le plus petit identifiant accepté (choix stable entre USB et HID).
pub fn select(candidates: &[(u16, u16)], policy: DevicePolicy) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, (vid, pid))| policy.accepts(*vid, *pid))
        .min_by_key(|(_, (_, pid))| (*pid != PID, *pid))
        .map(|(i, _)| i)
}

/// `vid:pid` d'une ligne `HID_ID=0003:00000DB0:00007881` d'un `uevent` hidraw.
pub fn parse_hid_id(uevent: &str) -> Option<(u16, u16)> {
    let id = uevent
        .lines()
        .find_map(|line| line.strip_prefix("HID_ID="))?;
    let mut parts = id.trim().split(':');
    let _bus = parts.next()?;
    let vid = u32::from_str_radix(parts.next()?, 16).ok()?;
    let pid = u32::from_str_radix(parts.next()?, 16).ok()?;
    Some((u16::try_from(vid).ok()?, u16::try_from(pid).ok()?))
}

/// Libellé `0db0:7881`.
pub fn usb_id(pid: u16) -> String {
    format!("{VID:04x}:{pid:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_only_accepts_the_tested_screen() {
        assert_eq!(DevicePolicy::default(), DevicePolicy::Tested);
        let p = DevicePolicy::Tested;
        assert!(p.accepts(0x0db0, 0x7881));
        for pid in [0x7880, 0x7882, 0x7919, 0x84df, 0x0076, 0xe5c3] {
            assert!(!p.accepts(0x0db0, pid), "{pid:04x}");
        }
        assert!(!p.accepts(0x1234, 0x7881));
    }

    #[test]
    fn exactly_the_forty_pids_of_the_windows_driver_are_declared() {
        let declared: Vec<u16> = (0u16..=0xFFFF).filter(|p| is_declared_pid(*p)).collect();
        assert_eq!(declared.len(), 40);
        assert_eq!(declared.first(), Some(&0x7880));
        assert_eq!(declared.last(), Some(&0x7919));
        assert!(declared.contains(&crate::PID));
        // La règle udev (packaging/) emploie le motif équivalent 78[89][0-9]|79[01][0-9].
        assert!(
            declared
                .iter()
                .all(|p| format!("{p:04x}").chars().all(|c| c.is_ascii_digit()))
        );
    }

    #[test]
    fn any_supported_accepts_the_declared_range_only() {
        let p = DevicePolicy::AnySupported;
        for pid in [0x7880, 0x7881, 0x7899, 0x7900, 0x7919] {
            assert!(p.accepts(0x0db0, pid), "{pid:04x}");
        }
        // Autres périphériques MSI de la machine : jamais.
        // Hors liste, y compris les valeurs hexadécimales intercalées.
        for pid in [
            0x787f, 0x788a, 0x789f, 0x78ff, 0x790a, 0x791a, 0x7920, 0x84df, 0x0076, 0xe5c3,
        ] {
            assert!(!p.accepts(0x0db0, pid), "{pid:04x}");
        }
        assert!(!p.accepts(0x1234, 0x7890));
    }

    #[test]
    fn selection_prefers_7881_then_lowest_pid() {
        let list = [
            (0x0db0, 0x84df),
            (0x0db0, 0x7890),
            (0x0db0, 0x7881),
            (0x0db0, 0x7885),
        ];
        assert_eq!(select(&list, DevicePolicy::Tested), Some(2));
        assert_eq!(select(&list, DevicePolicy::AnySupported), Some(2));
        let others = [(0x0db0, 0x84df), (0x0db0, 0x7890), (0x0db0, 0x7885)];
        assert_eq!(
            select(&others, DevicePolicy::Tested),
            None,
            "défaut inchangé"
        );
        assert_eq!(select(&others, DevicePolicy::AnySupported), Some(2));
        assert_eq!(select(&[], DevicePolicy::AnySupported), None);
    }

    #[test]
    fn hid_ids_and_labels() {
        let uevent = "DRIVER=hid-generic\nHID_ID=0003:00000DB0:00007881\nHID_NAME=MSI\n";
        assert_eq!(parse_hid_id(uevent), Some((0x0db0, 0x7881)));
        assert_eq!(
            parse_hid_id("HID_ID=0003:00000DB0:0000790A"),
            Some((0x0db0, 0x790a))
        );
        assert_eq!(parse_hid_id("HID_NAME=x"), None);
        assert_eq!(parse_hid_id("HID_ID=0003:zz:0000"), None);
        assert_eq!(usb_id(0x7881), "0db0:7881");
    }

    #[test]
    fn process_policy_roundtrip() {
        assert_eq!(policy(), DevicePolicy::Tested);
        set_policy(DevicePolicy::AnySupported);
        assert_eq!(policy(), DevicePolicy::AnySupported);
        set_policy(DevicePolicy::Tested);
        assert_eq!(policy(), DevicePolicy::Tested);
    }
}
