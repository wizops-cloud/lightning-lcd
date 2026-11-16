//! Accès D-Bus : état du démon `lightning` et pilotage du service systemd.

use std::collections::HashMap;

use lightning_ipc::{INTERFACE, LightningProxy, SYSTEMD_UNIT};
use zbus::zvariant::OwnedValue;
use zbus::{Connection, Proxy};

/// Copie locale des propriétés du démon.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DaemonState {
    pub lcd_state: String,
    pub lcd_detail: String,
    pub mode: String,
    pub brightness: i32,
    pub firmware_brightness: i32,
    pub fps: f64,
    pub rotation: u16,
    pub widgets: Vec<String>,
    pub available_widgets: Vec<String>,
    pub on_exit: String,
    pub http_enabled: bool,
    pub http_addr: String,
    pub image_path: String,
    pub image_fit: String,
    pub video_path: String,
    pub video_loop: bool,
    pub color: String,
    pub measured_fps: f64,
    pub render_ms: f64,
    pub frames_sent: u64,
    pub slideshow_folders: Vec<String>,
    pub slideshow_interval: f64,
    pub slideshow_order: String,
    pub slideshow_fit: String,
    pub slideshow_transition_ms: u32,
    pub slideshow_overlay: bool,
    pub slideshow_clock: bool,
    pub slideshow_count: u32,
    pub slideshow_current: String,
    pub clock_seconds: bool,
    pub clock_date: bool,
    pub now_playing_player: String,
    pub now_playing_title: String,
    pub google_state: String,
    pub google_detail: String,
    pub google_url: String,
    pub google_cached_count: u32,
    pub google_client_id: String,
    pub version: String,
}

fn get<T: TryFrom<OwnedValue>>(map: &mut HashMap<String, OwnedValue>, key: &str) -> Option<T> {
    map.remove(key).and_then(|v| T::try_from(v).ok())
}

impl DaemonState {
    /// Construit l'état à partir de la réponse de `Properties.GetAll`.
    /// Une propriété absente ou d'un type inattendu garde sa valeur par défaut.
    pub fn from_properties(mut map: HashMap<String, OwnedValue>) -> Self {
        let m = &mut map;
        Self {
            lcd_state: get(m, "LcdState").unwrap_or_default(),
            lcd_detail: get(m, "LcdDetail").unwrap_or_default(),
            mode: get(m, "Mode").unwrap_or_default(),
            brightness: get(m, "Brightness").unwrap_or(-1),
            firmware_brightness: get(m, "FirmwareBrightness").unwrap_or(-1),
            fps: get(m, "Fps").unwrap_or(2.0),
            rotation: get(m, "Rotation").unwrap_or(0),
            widgets: get(m, "Widgets").unwrap_or_default(),
            available_widgets: get(m, "AvailableWidgets").unwrap_or_default(),
            on_exit: get(m, "OnExit").unwrap_or_default(),
            http_enabled: get(m, "HttpEnabled").unwrap_or(false),
            http_addr: get(m, "HttpAddr").unwrap_or_default(),
            image_path: get(m, "ImagePath").unwrap_or_default(),
            image_fit: get(m, "ImageFit").unwrap_or_default(),
            video_path: get(m, "VideoPath").unwrap_or_default(),
            video_loop: get(m, "VideoLoop").unwrap_or(true),
            color: get(m, "Color").unwrap_or_default(),
            measured_fps: get(m, "MeasuredFps").unwrap_or(0.0),
            render_ms: get(m, "RenderMs").unwrap_or(0.0),
            frames_sent: get(m, "FramesSent").unwrap_or(0),
            slideshow_folders: get(m, "SlideshowFolders").unwrap_or_default(),
            slideshow_interval: get(m, "SlideshowInterval").unwrap_or(15.0),
            slideshow_order: get(m, "SlideshowOrder").unwrap_or_default(),
            slideshow_fit: get(m, "SlideshowFit").unwrap_or_default(),
            slideshow_transition_ms: get(m, "SlideshowTransitionMs").unwrap_or(0),
            slideshow_overlay: get(m, "SlideshowOverlay").unwrap_or(true),
            slideshow_clock: get(m, "SlideshowClock").unwrap_or(true),
            slideshow_count: get(m, "SlideshowCount").unwrap_or(0),
            slideshow_current: get(m, "SlideshowCurrent").unwrap_or_default(),
            clock_seconds: get(m, "ClockSeconds").unwrap_or(false),
            clock_date: get(m, "ClockDate").unwrap_or(true),
            now_playing_player: get(m, "NowPlayingPlayer").unwrap_or_default(),
            now_playing_title: get(m, "NowPlayingTitle").unwrap_or_default(),
            google_state: get(m, "GoogleState").unwrap_or_default(),
            google_detail: get(m, "GoogleDetail").unwrap_or_default(),
            google_url: get(m, "GoogleUrl").unwrap_or_default(),
            google_cached_count: get(m, "GoogleCachedCount").unwrap_or(0),
            google_client_id: get(m, "GoogleClientId").unwrap_or_default(),
            version: get(m, "Version").unwrap_or_default(),
        }
    }

    /// Libellé français de l'état de l'import Google Photos.
    pub fn google_label(&self) -> String {
        let base = match self.google_state.as_str() {
            "unconfigured" => "Identifiants OAuth à renseigner",
            "idle" => "Prêt",
            "awaiting_auth" => "En attente de votre autorisation dans le navigateur",
            "awaiting_selection" => "En attente de votre sélection dans Google Photos",
            "downloading" => "Téléchargement",
            "error" => "Échec",
            _ => "Inconnu",
        };
        match (self.google_state.as_str(), self.google_detail.is_empty()) {
            ("idle" | "error" | "downloading", false) => format!("{base} — {}", self.google_detail),
            _ => base.to_owned(),
        }
    }

    pub fn google_busy(&self) -> bool {
        matches!(
            self.google_state.as_str(),
            "awaiting_auth" | "awaiting_selection" | "downloading"
        )
    }

    /// Libellé français de l'état de l'écran.
    pub fn lcd_label(&self) -> &'static str {
        match self.lcd_state.as_str() {
            "connected" => "Connecté",
            "connecting" => "Connexion en cours…",
            "reconnecting" => "Reconnexion en cours…",
            "absent" => "Introuvable",
            "disabled" if self.mode == "msi_animation" => "Rendu au firmware",
            "disabled" => "Désactivé",
            _ => "Inconnu",
        }
    }
}

/// État du service systemd utilisateur.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ServiceState {
    /// `active`, `inactive`, `failed`, `activating`… ; vide si inconnu.
    pub active: String,
    /// `enabled`, `disabled`, `static`… ; vide si l'unité n'est pas installée.
    pub enabled: String,
}

impl ServiceState {
    pub fn is_installed(&self) -> bool {
        !self.enabled.is_empty()
    }

    pub fn is_active(&self) -> bool {
        matches!(self.active.as_str(), "active" | "activating" | "reloading")
    }

    pub fn is_enabled(&self) -> bool {
        matches!(
            self.enabled.as_str(),
            "enabled" | "enabled-runtime" | "linked"
        )
    }

    /// Libellé français de l'état d'activité.
    pub fn active_label(&self) -> &'static str {
        match self.active.as_str() {
            "active" => "En cours d'exécution",
            "activating" => "Démarrage…",
            "deactivating" => "Arrêt…",
            "failed" => "En échec",
            "inactive" => "Arrêté",
            _ if !self.is_installed() => "Non installé",
            _ => "Inconnu",
        }
    }
}

/// Client des deux services : le démon et le gestionnaire systemd utilisateur.
#[derive(Clone)]
pub struct Remote {
    connection: Connection,
    pub daemon: LightningProxy<'static>,
    properties: Proxy<'static>,
    systemd: Proxy<'static>,
}

impl Remote {
    pub async fn connect() -> zbus::Result<Self> {
        let connection = Connection::session().await?;
        let daemon = LightningProxy::builder(&connection)
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?;
        let properties = Proxy::new(
            &connection,
            lightning_ipc::BUS_NAME,
            lightning_ipc::OBJECT_PATH,
            "org.freedesktop.DBus.Properties",
        )
        .await?;
        let systemd = Proxy::new(
            &connection,
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
        )
        .await?;
        Ok(Self {
            connection,
            daemon,
            properties,
            systemd,
        })
    }

    /// Toutes les propriétés du démon en un appel ; erreur si le démon ne tourne pas.
    pub async fn daemon_state(&self) -> zbus::Result<DaemonState> {
        let map: HashMap<String, OwnedValue> =
            self.properties.call("GetAll", &(INTERFACE,)).await?;
        Ok(DaemonState::from_properties(map))
    }

    pub async fn service_state(&self) -> ServiceState {
        let enabled: String = self
            .systemd
            .call("GetUnitFileState", &(SYSTEMD_UNIT,))
            .await
            .unwrap_or_default();
        let active = self.active_state().await.unwrap_or_default();
        ServiceState { active, enabled }
    }

    async fn active_state(&self) -> zbus::Result<String> {
        let path: zbus::zvariant::OwnedObjectPath =
            self.systemd.call("LoadUnit", &(SYSTEMD_UNIT,)).await?;
        let unit = Proxy::new(
            &self.connection,
            "org.freedesktop.systemd1",
            path,
            "org.freedesktop.systemd1.Unit",
        )
        .await?;
        unit.get_property("ActiveState").await
    }

    pub async fn start_service(&self) -> zbus::Result<()> {
        self.systemd
            .call::<_, _, zbus::zvariant::OwnedObjectPath>("StartUnit", &(SYSTEMD_UNIT, "replace"))
            .await
            .map(drop)
    }

    pub async fn stop_service(&self) -> zbus::Result<()> {
        self.systemd
            .call::<_, _, zbus::zvariant::OwnedObjectPath>("StopUnit", &(SYSTEMD_UNIT, "replace"))
            .await
            .map(drop)
    }

    pub async fn restart_service(&self) -> zbus::Result<()> {
        self.systemd
            .call::<_, _, zbus::zvariant::OwnedObjectPath>(
                "RestartUnit",
                &(SYSTEMD_UNIT, "replace"),
            )
            .await
            .map(drop)
    }

    /// Active ou désactive le lancement à l'ouverture de session.
    pub async fn set_service_enabled(&self, enabled: bool) -> zbus::Result<()> {
        let units = vec![SYSTEMD_UNIT];
        if enabled {
            self.systemd
                .call::<_, _, (bool, Vec<(String, String, String)>)>(
                    "EnableUnitFiles",
                    &(units, false, true),
                )
                .await?;
        } else {
            self.systemd
                .call::<_, _, Vec<(String, String, String)>>("DisableUnitFiles", &(units, false))
                .await?;
        }
        self.systemd.call::<_, _, ()>("Reload", &()).await
    }
}

/// Message court et lisible pour une erreur D-Bus.
pub fn error_text(err: &zbus::Error) -> String {
    match err {
        zbus::Error::MethodError(_, Some(detail), _) => detail.clone(),
        zbus::Error::FDO(e) => match e.as_ref() {
            zbus::fdo::Error::Failed(m) | zbus::fdo::Error::InvalidArgs(m) => m.clone(),
            other => other.to_string(),
        },
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn owned(v: Value<'_>) -> OwnedValue {
        v.try_to_owned().unwrap()
    }

    #[test]
    fn state_from_properties_tolerates_missing_and_wrong_types() {
        let mut map = HashMap::new();
        map.insert("Mode".to_owned(), owned(Value::new("image")));
        map.insert("Brightness".to_owned(), owned(Value::new(70i32)));
        map.insert("Fps".to_owned(), owned(Value::new(5.0f64)));
        map.insert("Widgets".to_owned(), owned(Value::new(vec!["cpu", "gpu"])));
        map.insert("VideoLoop".to_owned(), owned(Value::new(false)));
        map.insert("Rotation".to_owned(), owned(Value::new("pas un nombre")));
        map.insert("LcdState".to_owned(), owned(Value::new("connected")));
        map.insert("ClockSeconds".to_owned(), owned(Value::new(true)));
        map.insert("NowPlayingTitle".to_owned(), owned(Value::new("T — A")));
        let st = DaemonState::from_properties(map);
        assert!(
            st.clock_seconds && st.clock_date,
            "date affichée par défaut"
        );
        assert_eq!(st.now_playing_title, "T — A");
        assert_eq!(st.mode, "image");
        assert_eq!(st.brightness, 70);
        assert_eq!(st.fps, 5.0);
        assert_eq!(st.widgets, ["cpu", "gpu"]);
        assert!(!st.video_loop);
        assert_eq!(st.rotation, 0, "type inattendu : valeur par défaut");
        assert_eq!(st.firmware_brightness, -1);
        assert_eq!(st.lcd_label(), "Connecté");
    }

    #[test]
    fn labels() {
        let msi = DaemonState {
            lcd_state: "disabled".into(),
            mode: "msi_animation".into(),
            ..Default::default()
        };
        assert_eq!(msi.lcd_label(), "Rendu au firmware");
        let google = |state: &str, detail: &str| DaemonState {
            google_state: state.into(),
            google_detail: detail.into(),
            ..Default::default()
        };
        assert_eq!(
            google("idle", "12 photos importées").google_label(),
            "Prêt — 12 photos importées"
        );
        assert_eq!(
            google("unconfigured", "").google_label(),
            "Identifiants OAuth à renseigner"
        );
        assert!(google("downloading", "Téléchargement 3 / 9").google_busy());
        assert!(!google("error", "refusé").google_busy());
        assert_eq!(google("error", "refusé").google_label(), "Échec — refusé");
        let s = ServiceState {
            active: "active".into(),
            enabled: "enabled".into(),
        };
        assert!(s.is_active() && s.is_enabled() && s.is_installed());
        assert_eq!(s.active_label(), "En cours d'exécution");
        let missing = ServiceState::default();
        assert!(!missing.is_installed());
        assert_eq!(missing.active_label(), "Non installé");
        assert_eq!(
            ServiceState {
                active: "failed".into(),
                enabled: "disabled".into()
            }
            .active_label(),
            "En échec"
        );
    }
}
