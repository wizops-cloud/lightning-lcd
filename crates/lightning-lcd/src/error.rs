use std::path::PathBuf;

/// Erreurs du pilote.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("l'écran est déjà piloté par un autre processus lightning (verrou pris)")]
    Busy,
    #[error("écran 0db0:7881 introuvable")]
    DeviceNotFound,
    #[error("interface HID de l'écran introuvable (hidraw)")]
    HidNotFound,
    #[error("accès refusé à {0} : installer packaging/70-lightning-lcd.rules ou lancer avec sudo")]
    PermissionDenied(PathBuf),
    #[error("accès refusé au périphérique USB : installer packaging/70-lightning-lcd.rules")]
    UsbPermissionDenied,
    #[error("erreur USB : {0}")]
    Usb(#[from] rusb::Error),
    #[error("erreur d'E/S : {0}")]
    Io(#[from] std::io::Error),
    #[error("authentification refusée : {0}")]
    Auth(&'static str),
    #[error("image de {got} octets, {expected} attendus ({width}x{height} RGB)")]
    BadFrameSize {
        got: usize,
        expected: usize,
        width: u32,
        height: u32,
    },
    #[error("encodage JPEG : {0}")]
    Jpeg(String),
    #[error("valeur hors limites : {0}")]
    OutOfRange(&'static str),
    #[error("impossible d'ouvrir une session avec l'écran après {0} tentatives")]
    SessionFailed(u32),
}

/// Alias de résultat du pilote.
pub type Result<T> = std::result::Result<T, Error>;
