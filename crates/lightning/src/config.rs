//! Fichier de configuration TOML (`~/.config/lightning/config.toml`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lightning_dashboard::{ClockConfig, DashboardConfig};
use lightning_ipc::{Fit, Mode, OnExit};
use serde::{Deserialize, Serialize};

use crate::google::GoogleConfig;
use crate::mpris::NowPlayingConfig;
use crate::slideshow::SlideshowConfig;

/// Configuration complète de l'application.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub dashboard: DashboardConfig,
    pub output: OutputConfig,
    pub daemon: DaemonConfig,
    pub slideshow: SlideshowConfig,
    pub clock: ClockConfig,
    pub now_playing: NowPlayingConfig,
    pub google: GoogleConfig,
}

/// Sorties d'affichage (section `[output]`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct OutputConfig {
    /// Sorties actives : `lcd`, `window`, `http`.
    pub outputs: Vec<OutputKind>,
    /// Adresse d'écoute du serveur d'aperçu.
    pub http_addr: String,
    /// Bandeau de debug dans la fenêtre d'aperçu.
    pub overlay: bool,
    /// Rotation simulée dans la fenêtre d'aperçu : 0, 90, 180 ou 270.
    pub preview_rotation: u16,
    /// Comportement de l'écran à l'arrêt : `keep`, `blank` ou `restore`.
    pub on_exit: OnExit,
    /// Accepter les écrans 0db0:7880 à 0db0:7919 (non testés) en plus de 0db0:7881.
    pub any_supported_pid: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            outputs: vec![OutputKind::Lcd],
            http_addr: "127.0.0.1:7881".into(),
            overlay: true,
            preview_rotation: 0,
            on_exit: OnExit::Keep,
            any_supported_pid: false,
        }
    }
}

/// Contenu affiché par le démon (section `[daemon]`), modifiable à chaud par D-Bus.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    /// `dashboard`, `image`, `video`, `color`, `slideshow`, `clock`, `now_playing`
    /// ou `msi_animation`.
    pub mode: Mode,
    /// Image du mode `image` (chemin absolu).
    pub image_path: String,
    pub image_fit: Fit,
    /// Vidéo du mode `video` (chemin absolu ou URL lisible par ffmpeg).
    pub video_path: String,
    pub video_loop: bool,
    /// Couleur du mode `color` (`RRGGBB`).
    pub color: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Dashboard,
            image_path: String::new(),
            image_fit: Fit::Cover,
            video_path: String::new(),
            video_loop: true,
            color: "101418".into(),
        }
    }
}

/// Une sortie d'affichage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum OutputKind {
    /// L'écran réel de la carte graphique
    Lcd,
    /// Fenêtre d'aperçu sur le bureau
    Window,
    /// Serveur HTTP local (page, flux MJPEG, image, état JSON)
    Http,
}

/// Emplacement par défaut : `$XDG_CONFIG_HOME/lightning/config.toml`.
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("lightning/config.toml"))
}

/// Charge la configuration. Sans chemin explicite, un fichier absent n'est pas
/// une erreur : les valeurs par défaut s'appliquent.
pub fn load(explicit: Option<&Path>) -> Result<Config> {
    let path = match explicit {
        Some(p) => p.to_owned(),
        None => match default_path() {
            Some(p) if p.exists() => p,
            _ => return Ok(Config::default()),
        },
    };
    let text = std::fs::read_to_string(&path)
        .with_context(|| lightning_i18n::trf("lecture de {path}", &[("path", &path.display())]))?;
    parse(&text).with_context(|| {
        lightning_i18n::trf(
            "configuration invalide : {path}",
            &[("path", &path.display())],
        )
    })
}

/// Vérifie les valeurs d'une configuration.
pub fn validate(config: &Config) -> Result<()> {
    let d = &config.dashboard;
    anyhow::ensure!(
        matches!(d.rotation, 0 | 180),
        lightning_i18n::tr("rotation : seules 0 et 180 sont prises en charge")
    );
    anyhow::ensure!(
        d.brightness.is_none_or(|b| b <= 100),
        lightning_i18n::tr("brightness : valeur attendue entre 0 et 100")
    );
    anyhow::ensure!(
        (1..=100).contains(&d.quality),
        lightning_i18n::tr("quality : 1 à 100")
    );
    anyhow::ensure!(
        d.fps.is_finite() && d.fps > 0.0,
        lightning_i18n::tr("fps : valeur strictement positive attendue")
    );
    anyhow::ensure!(
        matches!(config.output.preview_rotation, 0 | 90 | 180 | 270),
        lightning_i18n::tr("preview_rotation : 0, 90, 180 ou 270")
    );
    crate::media::parse_color(&config.daemon.color).context("daemon.color")?;
    config
        .slideshow
        .validate()
        .map_err(|e| anyhow::anyhow!("slideshow : {e}"))?;
    Ok(())
}

pub fn parse(text: &str) -> Result<Config> {
    let config: Config = toml::from_str(text)?;
    validate(&config)?;
    Ok(config)
}

/// Écrit la configuration (remplacement atomique du fichier).
pub fn save(path: &Path, config: &Config) -> Result<()> {
    let text = format!(
        "# Configuration de lightning. Ce fichier est réécrit par le démon quand un\n\
         # réglage change (application de réglage, `lightning ctl`) ; les commentaires\n\
         # ajoutés à la main ne sont pas conservés.\n\n{}",
        toml::to_string_pretty(config)?
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| {
            lightning_i18n::trf("création de {path}", &[("path", &dir.display())])
        })?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)
        .with_context(|| lightning_i18n::trf("écriture de {path}", &[("path", &tmp.display())]))?;
    std::fs::rename(&tmp, path).with_context(|| {
        lightning_i18n::trf("remplacement de {path}", &[("path", &path.display())])
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn example_file_parses() {
        let text = include_str!("../../../packaging/config.example.toml");
        let cfg = parse(text).unwrap();
        assert_eq!(cfg.dashboard.fps, 2.0);
        assert!(cfg.dashboard.has_widget("gpu"));
        assert_eq!(
            cfg,
            Config::default(),
            "l'exemple documente les valeurs par défaut"
        );
    }

    #[test]
    fn partial_and_invalid_values() {
        let cfg = parse("[dashboard]\nfps = 1.0\nwidgets = [\"cpu\"]\nbrightness = 60\n").unwrap();
        assert_eq!(cfg.dashboard.fps, 1.0);
        assert_eq!(cfg.dashboard.brightness, Some(60));
        assert!(!cfg.dashboard.has_widget("gpu"));
        assert!(parse("[dashboard]\nrotation = 90\n").is_err());
        assert!(parse("[dashboard]\nbrightness = 150\n").is_err());
        assert!(parse("[dashboard]\nfps = 0\n").is_err());
        assert!(parse("[dashboard]\ninconnu = 1\n").is_err());
    }

    #[test]
    fn output_section() {
        let cfg = parse(
            "[output]\noutputs = [\"lcd\", \"http\", \"window\"]\nhttp_addr = \"0.0.0.0:9000\"\n\
             overlay = false\npreview_rotation = 90\non_exit = \"blank\"\n",
        )
        .unwrap();
        assert_eq!(
            cfg.output.outputs,
            [OutputKind::Lcd, OutputKind::Http, OutputKind::Window]
        );
        assert_eq!(cfg.output.http_addr, "0.0.0.0:9000");
        assert!(!cfg.output.overlay);
        assert_eq!(cfg.output.on_exit, OnExit::Blank);
        assert_eq!(Config::default().output.outputs, [OutputKind::Lcd]);
        assert_eq!(Config::default().output.http_addr, "127.0.0.1:7881");
        assert!(parse("[output]\noutputs = [\"tv\"]\n").is_err());
        assert!(parse("[output]\npreview_rotation = 45\n").is_err());
        assert!(
            !Config::default().output.any_supported_pid,
            "7881 seul par défaut"
        );
        assert!(
            parse("[output]\nany_supported_pid = true\n")
                .unwrap()
                .output
                .any_supported_pid
        );
    }

    #[test]
    fn daemon_section() {
        let cfg = parse(
            "[daemon]\nmode = \"image\"\nimage_path = \"/tmp/a.png\"\nimage_fit = \"contain\"\n\
             video_loop = false\ncolor = \"ff8800\"\n",
        )
        .unwrap();
        assert_eq!(cfg.daemon.mode, Mode::Image);
        assert_eq!(cfg.daemon.image_fit, Fit::Contain);
        assert!(!cfg.daemon.video_loop);
        assert!(parse("[daemon]\nmode = \"tv\"\n").is_err());
        assert!(parse("[daemon]\ncolor = \"rouge\"\n").is_err());
    }

    #[test]
    fn slideshow_and_google_sections() {
        let cfg = parse(
            "[slideshow]\nfolders = [\"/home/moi/Images\"]\ninterval_s = 30\norder = \"chronological\"\n\
             fit = \"cover\"\ntransition_ms = 0\noverlay = false\n[google]\nclient_id = \"x.apps.googleusercontent.com\"\n\
             client_secret = \"s\"\n",
        )
        .unwrap();
        assert_eq!(cfg.slideshow.folders, ["/home/moi/Images"]);
        assert_eq!(cfg.slideshow.interval_s, 30.0);
        assert_eq!(cfg.slideshow.order, crate::slideshow::Order::Chronological);
        assert!(cfg.slideshow.clock && !cfg.slideshow.overlay);
        assert!(cfg.google.is_configured());
        assert!(!Config::default().google.is_configured());
        assert!(parse("[slideshow]\ninterval_s = 0.1\n").is_err());
        assert!(parse("[slideshow]\norder = \"alpha\"\n").is_err());
    }

    #[test]
    fn clock_and_now_playing_sections() {
        let cfg =
            parse("[clock]\nseconds = true\ndate = false\n[now_playing]\nplayer = \"spotify\"\n")
                .unwrap();
        assert!(cfg.clock.seconds && !cfg.clock.date);
        assert_eq!(cfg.now_playing.player, "spotify");
        let default = Config::default();
        assert!(!default.clock.seconds && default.clock.date);
        assert!(default.now_playing.player.is_empty());
        assert!(parse("[clock]\nformat = 12\n").is_err());
        assert_eq!(
            parse("[daemon]\nmode = \"now_playing\"\n")
                .unwrap()
                .daemon
                .mode,
            Mode::NowPlaying
        );
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("lightning-config-test-{}", std::process::id()));
        let path = dir.join("sub/config.toml");
        let mut cfg = Config::default();
        cfg.dashboard.fps = 5.0;
        cfg.dashboard.brightness = Some(70);
        cfg.dashboard.widgets = vec!["cpu".into(), "gpu".into()];
        cfg.output.on_exit = OnExit::Restore;
        cfg.output.outputs = vec![OutputKind::Lcd, OutputKind::Http];
        cfg.daemon.mode = Mode::MsiAnimation;
        cfg.daemon.image_path = "/home/moi/Images/été.jpg".into();
        cfg.slideshow.folders = vec!["/home/moi/Images".into()];
        cfg.slideshow.order = crate::slideshow::Order::Chronological;
        cfg.google.client_id = "x.apps.googleusercontent.com".into();
        cfg.clock.seconds = true;
        cfg.now_playing.player = "rhythmbox".into();
        save(&path, &cfg).unwrap();
        assert_eq!(load(Some(&path)).unwrap(), cfg);
        // Seconde écriture : remplace le fichier existant.
        cfg.daemon.mode = Mode::Dashboard;
        save(&path, &cfg).unwrap();
        assert_eq!(load(Some(&path)).unwrap(), cfg);
        let _ = std::fs::remove_dir_all(dir);
    }
}
