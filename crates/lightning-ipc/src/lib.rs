//! Interface D-Bus du démon `lightning` : noms, types partagés et proxy client.
//!
//! Le démon expose l'objet [`OBJECT_PATH`] sous le nom [`BUS_NAME`] sur le bus
//! de session, avec l'interface versionnée [`INTERFACE`]. Tous les réglages se
//! changent par des méthodes ; les propriétés sont en lecture seule et
//! signalent leurs changements (`PropertiesChanged`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Nom de bus du démon.
pub const BUS_NAME: &str = "cloud.wizops.Lightning.Daemon";
/// Chemin de l'objet exposé.
pub const OBJECT_PATH: &str = "/cloud/wizops/Lightning";
/// Interface versionnée.
pub const INTERFACE: &str = "cloud.wizops.Lightning1";
/// Unité systemd utilisateur qui fait tourner le démon.
pub const SYSTEMD_UNIT: &str = "lightning-dashboard.service";

/// Panneaux connus du tableau de bord, dans l'ordre par défaut.
pub const WIDGETS: [&str; 5] = ["gpu", "cpu", "memory", "disks", "network"];

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $text)] $variant),+
        }

        impl $name {
            /// Toutes les valeurs, dans l'ordre de déclaration.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// Nom utilisé sur D-Bus, en ligne de commande et dans la configuration.
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($text => Ok($name::$variant),)+
                    other => Err(lightning_i18n::trf(
                        "valeur « {value} » inconnue (attendu : {expected})",
                        &[("value", &other), ("expected", &[$($text),+].join(", "))],
                    )),
                }
            }
        }
    };
}

string_enum! {
    /// Contenu affiché sur l'écran.
    Mode {
        /// Tableau de bord de l'état du PC.
        #[default]
        Dashboard => "dashboard",
        /// Image fixe.
        Image => "image",
        /// Vidéo lue par ffmpeg.
        Video => "video",
        /// Couleur unie.
        Color => "color",
        /// Diaporama de photos (dossiers locaux, cache Google Photos).
        Slideshow => "slideshow",
        /// Horloge et date en plein écran.
        Clock => "clock",
        /// Morceau en cours de lecture (lecteurs MPRIS).
        NowPlaying => "now_playing",
        /// Écran rendu au firmware : animation MSI d'origine.
        MsiAnimation => "msi_animation",
    }
}

string_enum! {
    /// Ajustement d'une image à l'écran.
    Fit {
        /// Remplit l'écran en rognant.
        #[default]
        Cover => "cover",
        /// Image entière, bandes noires.
        Contain => "contain",
        /// Étire sans conserver les proportions.
        Stretch => "stretch",
    }
}

string_enum! {
    /// État de l'écran à l'arrêt du démon.
    OnExit {
        /// Laisse la dernière image affichée.
        #[default]
        Keep => "keep",
        /// Affiche un écran noir « arrêté ».
        Blank => "blank",
        /// Rend la main au firmware (animation MSI d'origine).
        Restore => "restore",
    }
}

/// Proxy client de l'interface `cloud.wizops.Lightning1`.
///
/// Génère `LightningProxy` (asynchrone) et `LightningProxyBlocking`.
#[zbus::proxy(
    interface = "cloud.wizops.Lightning1",
    default_service = "cloud.wizops.Lightning.Daemon",
    default_path = "/cloud/wizops/Lightning"
)]
pub trait Lightning {
    /// Change le contenu affiché (`dashboard`, `image`, `video`, `color`,
    /// `slideshow`, `clock`, `now_playing`, `msi_animation`).
    fn set_mode(&self, mode: &str) -> zbus::Result<()>;
    /// Affiche une image (chemin absolu) ; `fit` : `cover`, `contain` ou `stretch`.
    fn show_image(&self, path: &str, fit: &str) -> zbus::Result<()>;
    /// Lit une vidéo (chemin absolu ou URL lisible par ffmpeg).
    fn play_video(&self, path: &str, looped: bool) -> zbus::Result<()>;
    /// Affiche une couleur unie (`RRGGBB`).
    fn show_color(&self, rgb: &str) -> zbus::Result<()>;
    /// Rend l'écran au firmware (animation MSI d'origine).
    fn restore(&self) -> zbus::Result<()>;
    /// Règle la luminosité (0 à 100) ; appliquée en quelques secondes.
    fn set_brightness(&self, level: u8) -> zbus::Result<()>;
    /// Cadence du tableau de bord, en images par seconde.
    fn set_fps(&self, fps: f64) -> zbus::Result<()>;
    /// Rotation logicielle de l'image, sens horaire : 0, 90, 180 ou 270
    /// (90 et 270 : contenu composé en portrait).
    fn set_rotation(&self, degrees: u16) -> zbus::Result<()>;
    /// Panneaux du tableau de bord, dans l'ordre d'affichage.
    fn set_widgets(&self, widgets: &[&str]) -> zbus::Result<()>;
    /// Comportement à l'arrêt : `keep`, `blank` ou `restore`.
    fn set_on_exit(&self, behaviour: &str) -> zbus::Result<()>;
    /// Active ou coupe l'aperçu HTTP ; `addr` vide = adresse inchangée.
    fn set_http(&self, enabled: bool, addr: &str) -> zbus::Result<()>;
    /// Règle le diaporama : dossiers (récursifs), durée par photo en secondes,
    /// ordre (`random`, `chronological`), ajustement (`cover`, `contain`), durée
    /// du fondu en ms (0 = aucun), légende date/album, horloge.
    #[allow(clippy::too_many_arguments)]
    fn set_slideshow(
        &self,
        folders: &[&str],
        interval_s: f64,
        order: &str,
        fit: &str,
        transition_ms: u32,
        overlay: bool,
        clock: bool,
    ) -> zbus::Result<()>;
    /// Horloge plein écran : secondes, date.
    fn set_clock(&self, seconds: bool, date: bool) -> zbus::Result<()>;
    /// Lecteur préféré du mode `now_playing` (partie du nom MPRIS, vide = le premier qui joue).
    fn set_now_playing(&self, player: &str) -> zbus::Result<()>;
    /// Passe à la photo suivante du diaporama.
    fn slideshow_next(&self) -> zbus::Result<()>;
    /// Enregistre l'identifiant client OAuth Google (type « application de bureau »).
    fn set_google_credentials(&self, client_id: &str, client_secret: &str) -> zbus::Result<()>;
    /// Démarre un import Google Photos ; suivre `GoogleState` et ouvrir `GoogleUrl`.
    fn google_photos_start(&self) -> zbus::Result<()>;
    fn google_photos_cancel(&self) -> zbus::Result<()>;
    /// Supprime les photos Google mises en cache.
    fn google_photos_clear_cache(&self) -> zbus::Result<()>;
    /// Dernière image diffusée, en JPEG (vide si aucune).
    fn get_frame_jpeg(&self) -> zbus::Result<Vec<u8>>;
    /// Écrit la configuration courante dans le fichier TOML.
    fn save_config(&self) -> zbus::Result<()>;
    /// Relit le fichier TOML et applique ses réglages.
    fn reload_config(&self) -> zbus::Result<()>;

    /// `disabled`, `connecting`, `connected`, `reconnecting` ou `absent`.
    #[zbus(property)]
    fn lcd_state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn lcd_detail(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn mode(&self) -> zbus::Result<String>;
    /// Luminosité demandée (0 à 100), -1 si jamais réglée.
    #[zbus(property)]
    fn brightness(&self) -> zbus::Result<i32>;
    /// Luminosité confirmée par le firmware, -1 si inconnue.
    #[zbus(property)]
    fn firmware_brightness(&self) -> zbus::Result<i32>;
    #[zbus(property)]
    fn fps(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn rotation(&self) -> zbus::Result<u16>;
    #[zbus(property)]
    fn widgets(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn available_widgets(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn on_exit(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn http_enabled(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn http_addr(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn image_path(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn image_fit(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn video_path(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn video_loop(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn color(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn slideshow_folders(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn slideshow_interval(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn slideshow_order(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn slideshow_fit(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn slideshow_transition_ms(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn slideshow_overlay(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn slideshow_clock(&self) -> zbus::Result<bool>;
    /// Nombre de photos trouvées (0 hors mode diaporama).
    #[zbus(property)]
    fn slideshow_count(&self) -> zbus::Result<u32>;
    /// Photo en cours d'affichage.
    #[zbus(property)]
    fn slideshow_current(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn clock_seconds(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn clock_date(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn now_playing_player(&self) -> zbus::Result<String>;
    /// Morceau affiché (« Titre — Artiste »), vide si rien ne joue ou hors de ce mode.
    #[zbus(property)]
    fn now_playing_title(&self) -> zbus::Result<String>;
    /// `unconfigured`, `idle`, `awaiting_auth`, `awaiting_selection`, `downloading`, `error`.
    #[zbus(property)]
    fn google_state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn google_detail(&self) -> zbus::Result<String>;
    /// Page à ouvrir dans le navigateur pendant un import.
    #[zbus(property)]
    fn google_url(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn google_cached_count(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn google_client_id(&self) -> zbus::Result<String>;
    /// Dossier du cache des photos Google.
    #[zbus(property)]
    fn google_cache_dir(&self) -> zbus::Result<String>;
    /// Cadence réellement mesurée.
    #[zbus(property)]
    fn measured_fps(&self) -> zbus::Result<f64>;
    /// Durée de rendu de la dernière image, en millisecondes.
    #[zbus(property)]
    fn render_ms(&self) -> zbus::Result<f64>;
    /// Nombre d'images envoyées à l'écran depuis le démarrage.
    #[zbus(property)]
    fn frames_sent(&self) -> zbus::Result<u64>;
    #[zbus(property)]
    fn version(&self) -> zbus::Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_roundtrip_through_strings() {
        for mode in Mode::ALL {
            assert_eq!(mode.as_str().parse::<Mode>().unwrap(), *mode);
        }
        assert_eq!("msi_animation".parse::<Mode>().unwrap(), Mode::MsiAnimation);
        assert_eq!("now_playing".parse::<Mode>().unwrap(), Mode::NowPlaying);
        assert_eq!(Mode::Clock.to_string(), "clock");
        assert_eq!(Fit::Contain.to_string(), "contain");
        assert_eq!("blank".parse::<OnExit>().unwrap(), OnExit::Blank);
        let err = "tv".parse::<Mode>().unwrap_err();
        assert!(err.contains("tv") && err.contains("dashboard"), "{err}");
    }

    #[test]
    fn names_are_consistent() {
        // Le nom de bus du démon NE DOIT PAS être l'App ID de l'application GTK
        // (`cloud.wizops.Lightning`) : GApplication le prendrait pour une instance
        // déjà lancée et quitterait (« Unknown interface 'org.gtk.Actions' »).
        assert_ne!(BUS_NAME, "cloud.wizops.Lightning");
        assert!(BUS_NAME.starts_with("cloud.wizops.Lightning."));
        assert!(INTERFACE.starts_with("cloud.wizops.Lightning"));
        assert_eq!(OBJECT_PATH, "/cloud/wizops/Lightning");
    }
}
