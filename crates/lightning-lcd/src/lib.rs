//! Pilote userspace de l'écran LCD « V530-9.6 Lightning » de la MSI RTX 5090
//! Lightning Z (USB `0db0:7881`).
//!
//! L'écran est un « ArtInChip USB Display » rebadgé, doublé d'un firmware
//! applicatif piloté par HID. Voir `docs/PROTOCOL.md` pour le détail ; en résumé :
//!
//! 1. **HID** (interface 1) : `POST extendedDisplay {"enable":true}` pour quitter
//!    l'animation interne, répété jusqu'à réponse ([`hid::HidControl`]) ;
//! 2. **bulk** (interface 0) : authentification RSA, une fois par énumération USB ;
//! 3. une image = en-tête de 20 octets + JPEG baseline 4:2:0 ([`Display`]).
//!
//! ```no_run
//! # fn main() -> Result<(), lightning_lcd::Error> {
//! let mut display = lightning_lcd::Display::open()?;
//! let rouge = vec![[255u8, 0, 0]; 1280 * 800].concat();
//! display.show_rgb(&rouge)?;
//! # Ok(()) }
//! ```
//!
//! L'upload de médias dans la flash de l'écran (`transport`/`transported`) n'est
//! volontairement pas implémenté.

pub mod device;
pub mod display;
pub mod error;
pub mod frame;
pub mod hid;
pub mod rsa;

pub use device::DevicePolicy;
pub use display::{Display, Mode, OpenOptions};
pub use error::Error;
pub use hid::{HidControl, Status};

/// Identifiant USB du constructeur (Micro-Star International).
pub const VID: u16 = 0x0DB0;
/// Identifiant USB de l'écran « V530-9.6 Lightning ».
pub const PID: u16 = 0x7881;
/// Largeur native de l'écran, en pixels.
pub const WIDTH: u32 = 1280;
/// Hauteur native de l'écran, en pixels.
pub const HEIGHT: u32 = 800;
