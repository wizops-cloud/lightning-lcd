//! Démon : boucle d'affichage pilotable à chaud (D-Bus, `lightning ctl`,
//! application de réglage).
//!
//! Le moteur ([`Engine`]) possède les sorties et l'état ; il reçoit des
//! [`Request`] par un canal et publie un [`PublicState`] après chaque
//! changement. La couche D-Bus ([`dbus`]) n'est qu'un adaptateur au-dessus.

pub mod dbus;

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use lightning_dashboard::{ClockConfig, Collector, Renderer};
use lightning_ipc::{Fit, Mode, OnExit, WIDGETS};
use lightning_lcd::HidControl;
use lightning_lcd::display::encode_jpeg;
use tracing::{debug, info, warn};

use crate::config::{self, Config, OutputKind};
use crate::google::{self, Importer};
use crate::media;
use crate::mjpeg::JpegSplitter;
use crate::mpris;
use crate::outputs::http::HttpSink;
use crate::outputs::lcd::{HID_LOCK, LcdControl, LcdOptions, LcdSink};
use crate::outputs::{
    HEIGHT, LatestFrame, LcdState, MemorySink, Outputs, SharedStats, Sink, WIDTH,
};
use crate::slideshow::{Slideshow, SlideshowConfig};
use crate::text;

/// Demande de changement adressée au moteur.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    SetMode(Mode),
    ShowImage {
        path: String,
        fit: Fit,
    },
    PlayVideo {
        path: String,
        looped: bool,
    },
    ShowColor(String),
    Restore,
    SetBrightness(u8),
    SetFps(f64),
    SetRotation(u16),
    SetWidgets(Vec<String>),
    SetOnExit(OnExit),
    SetHttp {
        enabled: bool,
        addr: String,
    },
    SetSlideshow(SlideshowConfig),
    SlideshowNext,
    SetClock(ClockConfig),
    SetNowPlaying {
        player: String,
    },
    SetGoogleCredentials {
        client_id: String,
        client_secret: String,
    },
    GoogleStart,
    GoogleCancel,
    GoogleClearCache,
    SaveConfig,
    ReloadConfig,
}

/// Réponse à une [`Request`] : rien, ou un message d'erreur lisible.
pub type Reply = std::result::Result<(), String>;

/// Événements traités par la boucle du moteur.
pub enum Event {
    Request(Request, Sender<Reply>),
    VideoFrame(u64, Vec<u8>),
    VideoEnded(u64),
    /// Luminosité confirmée par le firmware (ou erreur).
    Brightness(std::result::Result<u32, String>),
    /// Fin d'un import Google Photos : nombre de photos, ou erreur.
    GoogleDone(std::result::Result<usize, String>),
}

/// État public du démon : une valeur par propriété D-Bus.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PublicState {
    pub lcd_state: String,
    pub lcd_detail: String,
    pub mode: String,
    pub brightness: i32,
    pub firmware_brightness: i32,
    pub fps: f64,
    pub rotation: u16,
    pub widgets: Vec<String>,
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
    pub google_cache_dir: String,
}

/// Accès au moteur depuis un autre fil.
#[derive(Clone)]
pub struct Handle {
    pub tx: Sender<Event>,
    pub state: Arc<Mutex<PublicState>>,
    pub latest: LatestFrame,
}

impl Handle {
    /// Envoie une demande et attend sa prise en compte.
    pub fn call(&self, request: Request) -> Reply {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Event::Request(request, tx))
            .map_err(|_| "le démon s'arrête".to_owned())?;
        rx.recv_timeout(Duration::from_secs(20))
            .map_err(|_| "le démon n'a pas répondu".to_owned())?
    }

    pub fn state(&self) -> PublicState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Dernière image diffusée, en JPEG (vide si aucune).
    pub fn frame_jpeg(&self) -> Vec<u8> {
        self.latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|f| f.jpeg.clone())
            .unwrap_or_default()
    }
}

/// Réglages de démarrage du moteur.
pub struct EngineOptions {
    pub config: Config,
    /// Fichier où persister les réglages ; `None` = ne rien écrire.
    pub config_path: Option<PathBuf>,
    /// Basculer l'écran en mode étendu via HID à la connexion.
    pub extend: bool,
    /// Services Google, fichier du jeton et dossier du cache.
    pub google: GoogleRuntime,
    /// Démonstration : métriques factices et, sans écran, liaison simulée
    /// (captures d'écran de la documentation).
    pub demo: bool,
}

/// Emplacements et services utilisés par l'import Google Photos.
#[derive(Debug, Clone)]
pub struct GoogleRuntime {
    pub endpoints: google::Endpoints,
    pub token_path: std::path::PathBuf,
    pub cache_root: std::path::PathBuf,
}

impl GoogleRuntime {
    /// Jeton à côté du fichier de configuration, cache dans `~/.cache/lightning`.
    pub fn standard(config_path: Option<&Path>) -> Self {
        let dir = config_path
            .and_then(Path::parent)
            .map(Path::to_owned)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            endpoints: google::Endpoints::default(),
            token_path: dir.join("google.json"),
            cache_root: google::cache_root(),
        }
    }
}

type Notifier = Box<dyn FnMut(&PublicState) + Send>;

struct Video {
    generation: u64,
    child: Child,
}

impl Drop for Video {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Boucle d'affichage du démon.
pub struct Engine {
    cfg: Config,
    config_path: Option<PathBuf>,
    out: Outputs,
    stats: SharedStats,
    lcd: Option<LcdControl>,
    stop: Arc<AtomicBool>,
    rx: Receiver<Event>,
    tx: Sender<Event>,
    state: Arc<Mutex<PublicState>>,
    notifier: Option<Notifier>,
    collector: Collector,
    renderer: Renderer,
    /// Image des modes fixes (image, couleur, message), déjà tournée.
    still: Option<Vec<u8>>,
    video: Option<Video>,
    video_generation: u64,
    brightness_tx: Option<Sender<BrightnessJob>>,
    firmware_brightness: i32,
    brightness_initialised: bool,
    slideshow: Option<Slideshow>,
    /// Lecteurs MPRIS observés (mode `now_playing` seulement).
    now_playing: Option<mpris::Watcher>,
    /// Dernière diffusion d'une image du diaporama (réémission périodique).
    last_slide: Instant,
    google: GoogleRuntime,
    importer: Importer,
    google_cached: u32,
    demo: bool,
}

fn new_collector(cfg: &Config, demo: bool) -> Collector {
    if demo {
        Collector::new_demo(cfg.dashboard.clone())
    } else {
        Collector::new(cfg.dashboard.clone())
    }
}

fn count_cached(root: &Path) -> u32 {
    crate::slideshow::scan(&[root.to_string_lossy().into_owned()], false).len() as u32
}

enum BrightnessJob {
    Set(u8),
    Read,
}

/// Fil qui parle au firmware pour la luminosité (plusieurs secondes par appel) ;
/// seule la dernière valeur demandée est appliquée.
fn brightness_worker(jobs: Receiver<BrightnessJob>, events: Sender<Event>) {
    while let Ok(mut job) = jobs.recv() {
        while let Ok(newer) = jobs.try_recv() {
            job = newer;
        }
        let result = {
            let _hid = HID_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            HidControl::open().and_then(|mut hid| match job {
                BrightnessJob::Set(level) => hid.set_brightness(level),
                BrightnessJob::Read => hid.status(),
            })
        };
        let event = match result {
            Ok(Some(status)) => Ok(status.brightness),
            Ok(None) => Err("pas de réponse du firmware".to_owned()),
            Err(e) => Err(e.to_string()),
        };
        if events.send(Event::Brightness(event)).is_err() {
            break;
        }
    }
}

fn message_frame(title: &str, subtitle: &str) -> Vec<u8> {
    let (w, h) = (WIDTH as usize, HEIGHT as usize);
    let mut buf = vec![0x0009_0B10u32; w * h];
    let mut painter = text::TextPainter::new();
    let tw = painter.measure(title, 44.0);
    painter.draw(
        &mut buf,
        w,
        h,
        (w as f32 - tw) / 2.0,
        h as f32 / 2.0,
        44.0,
        (190, 198, 212),
        title,
    );
    let sw = painter.measure(subtitle, 24.0);
    painter.draw(
        &mut buf,
        w,
        h,
        (w as f32 - sw) / 2.0,
        h as f32 / 2.0 + 48.0,
        24.0,
        (120, 130, 148),
        subtitle,
    );
    text::to_rgba(&buf)
}

impl Engine {
    /// Construit les sorties et le moteur. L'écran absent n'est pas une erreur.
    pub fn new(opts: EngineOptions, stop: Arc<AtomicBool>) -> Result<(Self, Handle)> {
        let EngineOptions {
            config: cfg,
            config_path,
            extend,
            google,
            demo,
        } = opts;
        config::validate(&cfg)?;
        let stats = SharedStats::default();
        let latest = LatestFrame::default();
        let quality = cfg.dashboard.quality;
        let mut sinks: Vec<Box<dyn Sink>> = vec![Box::new(MemorySink(latest.clone()))];
        let mut lcd = None;
        for kind in &cfg.output.outputs {
            match kind {
                OutputKind::Lcd => {
                    let rgba = text::stopped_frame(WIDTH, HEIGHT, "démon arrêté");
                    let sink = LcdSink::spawn(
                        LcdOptions {
                            extend,
                            quality,
                            on_exit: cfg.output.on_exit,
                            blank_jpeg: Some(encode_jpeg(&rgba, WIDTH, HEIGHT, 4, quality)?),
                        },
                        stats.clone(),
                        stop.clone(),
                    );
                    lcd = Some(sink.control());
                    sinks.push(Box::new(sink));
                }
                OutputKind::Http => {
                    match HttpSink::spawn(&cfg.output.http_addr, stats.clone(), stop.clone()) {
                        Ok(sink) => sinks.push(Box::new(sink)),
                        Err(e) => warn!("aperçu HTTP non démarré : {e:#}"),
                    }
                }
                OutputKind::Window => match crate::window_sink(&cfg, &stats, &stop) {
                    Ok(sink) => sinks.push(sink),
                    Err(e) => warn!("fenêtre d'aperçu non ouverte : {e:#}"),
                },
            }
        }
        let out = Outputs::new(sinks, stats.clone(), quality, "daemon");
        let (tx, rx) = mpsc::channel();
        let brightness_tx = lcd.is_some().then(|| {
            let (jobs_tx, jobs_rx) = mpsc::channel();
            let events = tx.clone();
            std::thread::Builder::new()
                .name("brightness".into())
                .spawn(move || brightness_worker(jobs_rx, events))
                .expect("création du fil de luminosité");
            jobs_tx
        });
        let state = Arc::new(Mutex::new(PublicState::default()));
        let handle = Handle {
            tx: tx.clone(),
            state: state.clone(),
            latest,
        };
        let mut engine = Self {
            demo,
            collector: new_collector(&cfg, demo),
            renderer: Renderer::new(),
            cfg,
            config_path,
            out,
            stats,
            lcd,
            stop,
            rx,
            tx,
            state,
            notifier: None,
            still: None,
            video: None,
            video_generation: 0,
            brightness_tx,
            firmware_brightness: -1,
            brightness_initialised: false,
            slideshow: None,
            now_playing: None,
            last_slide: Instant::now(),
            google_cached: count_cached(&google.cache_root),
            importer: Importer::new(false),
            google,
        };
        engine.importer.reset(engine.cfg.google.is_configured());
        if demo && engine.lcd.is_none() {
            engine.stats.set_lcd(LcdState::Connected, "démonstration");
            engine.firmware_brightness = 100;
        }
        let wanted = engine.cfg.daemon.mode;
        if let Err(e) = engine.enter_mode(wanted) {
            warn!("mode « {wanted} » inutilisable ({e}) : retour au tableau de bord");
            engine.cfg.daemon.mode = Mode::Dashboard;
        }
        engine.publish();
        Ok((engine, handle))
    }

    /// Fonction appelée après chaque changement de l'état public.
    pub fn set_notifier(&mut self, notifier: Notifier) {
        self.notifier = Some(notifier);
    }

    fn public_state(&self) -> PublicState {
        let stats = self.stats.snapshot();
        let (c, d, o) = (&self.cfg.dashboard, &self.cfg.daemon, &self.cfg.output);
        let google = self.importer.status();
        let lcd_state = serde_json::to_value(stats.lcd.state)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        PublicState {
            lcd_state,
            lcd_detail: stats.lcd.detail,
            mode: d.mode.to_string(),
            brightness: c.brightness.map_or(-1, i32::from),
            firmware_brightness: self.firmware_brightness,
            fps: f64::from(c.fps),
            rotation: c.rotation,
            widgets: c.widgets.clone(),
            on_exit: o.on_exit.to_string(),
            http_enabled: self.out.names().contains(&"http"),
            http_addr: o.http_addr.clone(),
            image_path: d.image_path.clone(),
            image_fit: d.image_fit.to_string(),
            video_path: d.video_path.clone(),
            video_loop: d.video_loop,
            color: d.color.clone(),
            measured_fps: (f64::from(stats.fps) * 10.0).round() / 10.0,
            render_ms: (f64::from(stats.render_ms) * 10.0).round() / 10.0,
            frames_sent: stats.lcd.frames_sent,
            slideshow_folders: self.cfg.slideshow.folders.clone(),
            slideshow_interval: self.cfg.slideshow.interval_s,
            slideshow_order: self.cfg.slideshow.order.as_str().to_owned(),
            slideshow_fit: self.cfg.slideshow.fit.as_str().to_owned(),
            slideshow_transition_ms: self.cfg.slideshow.transition_ms,
            slideshow_overlay: self.cfg.slideshow.overlay,
            slideshow_clock: self.cfg.slideshow.clock,
            slideshow_count: self.slideshow.as_ref().map_or(0, |s| s.count() as u32),
            slideshow_current: self
                .slideshow
                .as_ref()
                .and_then(|s| s.current_path())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            clock_seconds: self.cfg.clock.seconds,
            clock_date: self.cfg.clock.date,
            now_playing_player: self.cfg.now_playing.player.clone(),
            now_playing_title: self
                .now_playing
                .as_ref()
                .and_then(|w| w.current(Instant::now()).0)
                .map(|t| t.summary())
                .unwrap_or_default(),
            google_state: google.state,
            google_detail: google.detail,
            google_url: google.url,
            google_cached_count: self.google_cached,
            google_client_id: self.cfg.google.client_id.clone(),
            google_cache_dir: self.google.cache_root.to_string_lossy().into_owned(),
        }
    }

    /// Met à jour l'état public et prévient l'observateur s'il a changé.
    fn publish(&mut self) {
        let new = self.public_state();
        let changed = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let changed = *state != new;
            *state = new.clone();
            changed
        };
        if changed && let Some(notify) = &mut self.notifier {
            notify(&new);
        }
    }

    fn persist(&self) {
        if let Some(path) = &self.config_path
            && let Err(e) = config::save(path, &self.cfg)
        {
            warn!("configuration non enregistrée : {e:#}");
        }
    }

    fn rotated(&self, mut rgba: Vec<u8>) -> Vec<u8> {
        if self.cfg.dashboard.rotation == 180 {
            media::rotate_180(&mut rgba);
        }
        rgba
    }

    fn start_video(&mut self) -> Reply {
        let d = &self.cfg.daemon;
        if d.video_path.is_empty() {
            return Err("aucune vidéo choisie".into());
        }
        if !d.video_path.contains("://") && !Path::new(&d.video_path).exists() {
            return Err(format!("vidéo introuvable : {}", d.video_path));
        }
        let mut child = media::spawn_ffmpeg(
            &d.video_path,
            30.0,
            d.video_loop,
            self.cfg.dashboard.rotation == 180,
            WIDTH,
            HEIGHT,
        )
        .map_err(|e| format!("{e:#}"))?;
        let stdout = child.stdout.take().ok_or("sortie de ffmpeg indisponible")?;
        self.video_generation += 1;
        let generation = self.video_generation;
        let events = self.tx.clone();
        std::thread::Builder::new()
            .name("video".into())
            .spawn(move || {
                for jpeg in JpegSplitter::new(stdout) {
                    let Ok(jpeg) = jpeg else { break };
                    if events.send(Event::VideoFrame(generation, jpeg)).is_err() {
                        return;
                    }
                }
                let _ = events.send(Event::VideoEnded(generation));
            })
            .map_err(|e| e.to_string())?;
        self.video = Some(Video { generation, child });
        Ok(())
    }

    /// Prépare le contenu d'un mode. En cas d'erreur, rien n'est changé.
    fn enter_mode(&mut self, mode: Mode) -> Reply {
        let still = match mode {
            Mode::Dashboard | Mode::Video | Mode::Slideshow | Mode::Clock | Mode::NowPlaying => {
                None
            }
            Mode::Image => {
                let d = &self.cfg.daemon;
                if d.image_path.is_empty() {
                    return Err("aucune image choisie".into());
                }
                let rgba = media::load_image(Path::new(&d.image_path), d.image_fit, WIDTH, HEIGHT)
                    .map_err(|e| format!("{e:#}"))?;
                Some(self.rotated(rgba))
            }
            Mode::Color => {
                let rgb = media::parse_color(&self.cfg.daemon.color).map_err(|e| e.to_string())?;
                Some(media::solid(rgb, WIDTH, HEIGHT))
            }
            Mode::MsiAnimation => Some(message_frame(
                "animation MSI",
                "l'écran est rendu à son firmware",
            )),
        };
        self.video = None;
        self.slideshow = None;
        self.now_playing = (mode == Mode::NowPlaying).then(|| {
            if self.demo {
                mpris::Watcher::demo()
            } else {
                mpris::Watcher::spawn(self.cfg.now_playing.player.clone())
            }
        });
        if mode == Mode::Video {
            self.start_video()?;
        }
        if mode == Mode::Slideshow {
            self.slideshow = Some(Slideshow::new(self.cfg.slideshow.clone(), WIDTH, HEIGHT));
        }
        if let Some(lcd) = &self.lcd {
            if mode == Mode::MsiAnimation {
                lcd.suspend();
            } else {
                lcd.resume();
            }
        }
        self.still = still;
        self.cfg.daemon.mode = mode;
        if let Some(rgba) = &self.still
            && let Err(e) = self.out.present_rgba(rgba, Duration::ZERO, false)
        {
            warn!("image non diffusée : {e:#}");
        }
        info!(%mode, "contenu affiché");
        Ok(())
    }

    fn set_http(&mut self, enabled: bool, addr: &str) -> Reply {
        let addr = if addr.is_empty() {
            self.cfg.output.http_addr.clone()
        } else {
            addr.to_owned()
        };
        addr.parse::<std::net::SocketAddr>()
            .map_err(|_| format!("adresse « {addr} » invalide (attendu : IP:port)"))?;
        self.out.remove_sink("http");
        if enabled {
            // Le port de l'ancien serveur se libère en quelques dizaines de ms.
            let mut last = String::new();
            let mut started = false;
            for _ in 0..10 {
                match HttpSink::spawn(&addr, self.stats.clone(), self.stop.clone()) {
                    Ok(sink) => {
                        self.out.add_sink(Box::new(sink));
                        started = true;
                        break;
                    }
                    Err(e) => {
                        last = format!("{e:#}");
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            if !started {
                return Err(last);
            }
        }
        self.cfg.output.http_addr = addr;
        self.cfg.output.outputs.retain(|o| *o != OutputKind::Http);
        if enabled {
            self.cfg.output.outputs.push(OutputKind::Http);
        }
        Ok(())
    }

    fn request_brightness(&mut self, level: u8) {
        self.cfg.dashboard.brightness = Some(level);
        if let Some(jobs) = &self.brightness_tx {
            let _ = jobs.send(BrightnessJob::Set(level));
        }
    }

    fn handle(&mut self, request: Request) -> Reply {
        debug!(?request, "demande");
        match request {
            Request::SetMode(mode) => self.enter_mode(mode)?,
            Request::ShowImage { path, fit } => {
                let previous = (
                    self.cfg.daemon.image_path.clone(),
                    self.cfg.daemon.image_fit,
                );
                self.cfg.daemon.image_path = path;
                self.cfg.daemon.image_fit = fit;
                if let Err(e) = self.enter_mode(Mode::Image) {
                    (self.cfg.daemon.image_path, self.cfg.daemon.image_fit) = previous;
                    return Err(e);
                }
            }
            Request::PlayVideo { path, looped } => {
                let previous = (
                    self.cfg.daemon.video_path.clone(),
                    self.cfg.daemon.video_loop,
                );
                self.cfg.daemon.video_path = path;
                self.cfg.daemon.video_loop = looped;
                if let Err(e) = self.enter_mode(Mode::Video) {
                    (self.cfg.daemon.video_path, self.cfg.daemon.video_loop) = previous;
                    // Le mode précédent a perdu sa vidéo éventuelle : on le relance.
                    let mode = self.cfg.daemon.mode;
                    let _ = self.enter_mode(mode);
                    return Err(e);
                }
            }
            Request::ShowColor(rgb) => {
                let rgb = rgb.trim_start_matches('#').to_ascii_lowercase();
                media::parse_color(&rgb).map_err(|e| e.to_string())?;
                self.cfg.daemon.color = rgb;
                self.enter_mode(Mode::Color)?;
            }
            Request::Restore => self.enter_mode(Mode::MsiAnimation)?,
            Request::SetBrightness(level) => {
                if level > 100 {
                    return Err("luminosité attendue entre 0 et 100".into());
                }
                self.request_brightness(level);
            }
            Request::SetFps(fps) => {
                if !fps.is_finite() || !(0.1..=30.0).contains(&fps) {
                    return Err("cadence attendue entre 0,1 et 30 images par seconde".into());
                }
                self.cfg.dashboard.fps = fps as f32;
            }
            Request::SetRotation(degrees) => {
                if !matches!(degrees, 0 | 180) {
                    return Err("rotation : seules 0 et 180 sont prises en charge".into());
                }
                if self.cfg.dashboard.rotation != degrees {
                    self.cfg.dashboard.rotation = degrees;
                    // Les contenus fixes et la vidéo sont préparés déjà tournés.
                    let mode = self.cfg.daemon.mode;
                    if mode != Mode::Dashboard {
                        self.enter_mode(mode)?;
                    }
                }
            }
            Request::SetWidgets(widgets) => {
                let mut clean: Vec<String> = Vec::new();
                for w in widgets {
                    let w = w.to_ascii_lowercase();
                    if !WIDGETS.contains(&w.as_str()) {
                        return Err(format!(
                            "panneau « {w} » inconnu (attendu : {})",
                            WIDGETS.join(", ")
                        ));
                    }
                    if !clean.contains(&w) {
                        clean.push(w);
                    }
                }
                self.cfg.dashboard.widgets = clean;
            }
            Request::SetOnExit(on_exit) => {
                self.cfg.output.on_exit = on_exit;
                if let Some(lcd) = &self.lcd {
                    lcd.set_on_exit(on_exit);
                }
            }
            Request::SetHttp { enabled, addr } => self.set_http(enabled, &addr)?,
            Request::SetSlideshow(mut new) => {
                new.validate()?;
                new.folders.retain(|f| !f.trim().is_empty());
                new.folders.dedup();
                if let Some(relative) = new.folders.iter().find(|f| !Path::new(f).is_absolute()) {
                    return Err(format!("chemin absolu attendu : {relative}"));
                }
                self.cfg.slideshow = new;
                if self.cfg.daemon.mode == Mode::Slideshow {
                    self.enter_mode(Mode::Slideshow)?;
                }
            }
            Request::SlideshowNext => match &mut self.slideshow {
                Some(show) => show.skip(),
                None => return Err("le diaporama n'est pas affiché".into()),
            },
            Request::SetClock(clock) => self.cfg.clock = clock,
            Request::SetNowPlaying { player } => {
                let player = player.trim().to_owned();
                if player.contains(char::is_whitespace) {
                    return Err("lecteur : un nom sans espace est attendu (ex. spotify)".into());
                }
                if player != self.cfg.now_playing.player {
                    self.cfg.now_playing.player = player;
                    if self.cfg.daemon.mode == Mode::NowPlaying {
                        self.enter_mode(Mode::NowPlaying)?;
                    }
                }
            }
            Request::SetGoogleCredentials {
                client_id,
                client_secret,
            } => {
                if self.importer.is_running() {
                    return Err("un import est en cours".into());
                }
                self.cfg.google.client_id = client_id.trim().to_owned();
                self.cfg.google.client_secret = client_secret.trim().to_owned();
                self.importer.reset(self.cfg.google.is_configured());
            }
            Request::GoogleStart => {
                let events = self.tx.clone();
                self.importer.start(
                    google::ImportOptions {
                        config: self.cfg.google.clone(),
                        endpoints: self.google.endpoints.clone(),
                        token_path: self.google.token_path.clone(),
                        cache_root: self.google.cache_root.clone(),
                        width: WIDTH,
                        height: HEIGHT,
                        auth_timeout: Duration::from_secs(600),
                    },
                    move |result| {
                        let _ = events.send(Event::GoogleDone(result));
                    },
                )?;
                return Ok(());
            }
            Request::GoogleCancel => {
                self.importer.cancel();
                return Ok(());
            }
            Request::GoogleClearCache => {
                if self.importer.is_running() {
                    return Err("un import est en cours".into());
                }
                google::clear_cache(&self.google.cache_root).map_err(|e| e.to_string())?;
                self.google_cached = 0;
                if self.cfg.daemon.mode == Mode::Slideshow {
                    self.enter_mode(Mode::Slideshow)?;
                }
                return Ok(());
            }
            Request::SaveConfig => {
                let path = self
                    .config_path
                    .as_ref()
                    .ok_or("aucun fichier de configuration")?;
                return config::save(path, &self.cfg).map_err(|e| format!("{e:#}"));
            }
            Request::ReloadConfig => return self.reload(),
        }
        self.persist();
        Ok(())
    }

    fn reload(&mut self) -> Reply {
        let path = self
            .config_path
            .clone()
            .ok_or("aucun fichier de configuration")?;
        let mut new = if path.exists() {
            config::load(Some(&path)).map_err(|e| format!("{e:#}"))?
        } else {
            Config::default()
        };
        // Les sorties lcd/window ne changent pas à chaud ; l'aperçu HTTP si.
        let http = new.output.outputs.contains(&OutputKind::Http);
        let http_addr = new.output.http_addr.clone();
        new.output.outputs = self.cfg.output.outputs.clone();
        new.output.http_addr = self.cfg.output.http_addr.clone();
        let brightness_changed = new.dashboard.brightness != self.cfg.dashboard.brightness;
        self.cfg = new;
        self.collector = new_collector(&self.cfg, self.demo);
        if let Some(lcd) = &self.lcd {
            lcd.set_on_exit(self.cfg.output.on_exit);
        }
        if let (true, Some(level)) = (brightness_changed, self.cfg.dashboard.brightness) {
            self.request_brightness(level);
        }
        self.importer.reset(self.cfg.google.is_configured());
        let http_result = self.set_http(http, &http_addr);
        let mode = self.cfg.daemon.mode;
        if let Err(e) = self.enter_mode(mode) {
            self.cfg.daemon.mode = Mode::Dashboard;
            let _ = self.enter_mode(Mode::Dashboard);
            return Err(format!("mode « {mode} » inutilisable : {e}"));
        }
        http_result
    }

    fn tick(&mut self) {
        match self.cfg.daemon.mode {
            Mode::Dashboard => {
                let t0 = Instant::now();
                let snapshot = self.collector.sample();
                let rgba =
                    self.renderer
                        .render(&snapshot, self.collector.history(), &self.cfg.dashboard);
                let render = t0.elapsed();
                if let Err(e) = self.out.present_rgba(rgba, render, false) {
                    warn!("image non diffusée : {e:#}");
                }
            }
            Mode::Video => {}
            Mode::Clock => {
                let t0 = Instant::now();
                let now = chrono::Local::now().naive_local();
                let rgba =
                    self.renderer
                        .render_clock(now, &self.cfg.clock, self.cfg.dashboard.rotation);
                if let Err(e) = self.out.present_rgba(rgba, t0.elapsed(), false) {
                    warn!("image non diffusée : {e:#}");
                }
            }
            Mode::NowPlaying => {
                let t0 = Instant::now();
                let (track, art) = self
                    .now_playing
                    .as_ref()
                    .map(|w| w.current(t0))
                    .unwrap_or_default();
                let now = chrono::Local::now().naive_local();
                let rgba = self.renderer.render_now_playing(
                    track.as_ref(),
                    art.as_deref(),
                    now,
                    self.cfg.dashboard.rotation,
                );
                if let Err(e) = self.out.present_rgba(rgba, t0.elapsed(), false) {
                    warn!("image non diffusée : {e:#}");
                }
            }
            Mode::Slideshow => {
                let Some(show) = &mut self.slideshow else {
                    return;
                };
                let now = Instant::now();
                if let Some(mut rgba) = show.poll(now) {
                    if self.cfg.dashboard.rotation == 180 {
                        media::rotate_180(&mut rgba);
                    }
                    self.still = Some(rgba);
                    self.last_slide = now - Duration::from_secs(60);
                }
                // Nouvelle image tout de suite ; sinon réémission toutes les 2 s
                // (retour de l'écran après une reconnexion, aperçu vivant).
                if now.duration_since(self.last_slide) >= Duration::from_secs(2)
                    && let Some(rgba) = &self.still
                {
                    self.last_slide = now;
                    if let Err(e) = self.out.present_rgba(rgba, Duration::ZERO, false) {
                        warn!("image non diffusée : {e:#}");
                    }
                }
            }
            Mode::Image | Mode::Color | Mode::MsiAnimation => {
                // Réémission périodique : l'image revient d'elle-même après une
                // reconnexion de l'écran, et l'aperçu reste vivant.
                if let Some(rgba) = &self.still
                    && let Err(e) = self.out.present_rgba(rgba, Duration::ZERO, false)
                {
                    warn!("image non diffusée : {e:#}");
                }
            }
        }
    }

    fn tick_interval(&self) -> Duration {
        match self.cfg.daemon.mode {
            Mode::Dashboard => self.cfg.dashboard.frame_interval(),
            // Pas court : étapes du fondu enchaîné.
            Mode::Slideshow => Duration::from_millis(50),
            // Calé sur le changement de seconde de l'horloge murale.
            Mode::Clock | Mode::NowPlaying => {
                let ms = chrono::Local::now().timestamp_subsec_millis().min(999);
                Duration::from_millis(u64::from(1000 - ms) + 5)
            }
            _ => Duration::from_secs(1),
        }
    }

    /// Première connexion de l'écran : applique ou lit la luminosité.
    fn init_brightness(&mut self) {
        if self.brightness_initialised || self.stats.snapshot().lcd.state != LcdState::Connected {
            return;
        }
        self.brightness_initialised = true;
        if let Some(jobs) = &self.brightness_tx {
            let job = match self.cfg.dashboard.brightness {
                Some(level) => BrightnessJob::Set(level),
                None => BrightnessJob::Read,
            };
            let _ = jobs.send(job);
        }
    }

    /// Fait tourner le démon jusqu'à l'arrêt demandé.
    pub fn run(mut self) {
        info!(mode = %self.cfg.daemon.mode, sorties = ?self.out.names(), "démon démarré");
        let mut next_tick = Instant::now();
        let mut next_publish = Instant::now();
        while !self.stop.load(Ordering::SeqCst) {
            let wait = next_tick
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(200));
            match self.rx.recv_timeout(wait) {
                Ok(Event::Request(request, reply)) => {
                    let result = self.handle(request);
                    if let Err(e) = &result {
                        debug!("demande refusée : {e}");
                    }
                    // État publié avant la réponse : l'appelant le lit à jour.
                    self.publish();
                    let _ = reply.send(result);
                    next_tick = Instant::now(); // application immédiate
                }
                Ok(Event::VideoFrame(generation, jpeg)) => {
                    if self
                        .video
                        .as_ref()
                        .is_some_and(|v| v.generation == generation)
                        && let Err(e) = self.out.present_jpeg(jpeg, false)
                    {
                        warn!("image non diffusée : {e:#}");
                    }
                }
                Ok(Event::VideoEnded(generation)) => {
                    if self
                        .video
                        .as_ref()
                        .is_some_and(|v| v.generation == generation)
                    {
                        info!("vidéo terminée");
                        self.video = None;
                    }
                }
                Ok(Event::Brightness(result)) => {
                    match result {
                        Ok(level) => self.firmware_brightness = level as i32,
                        Err(e) => warn!("luminosité : {e}"),
                    }
                    self.publish();
                }
                Ok(Event::GoogleDone(result)) => {
                    self.google_cached = count_cached(&self.google.cache_root);
                    if result.is_ok() {
                        // Le cache devient un dossier du diaporama.
                        let root = self.google.cache_root.to_string_lossy().into_owned();
                        if !self.cfg.slideshow.folders.contains(&root) {
                            self.cfg.slideshow.folders.push(root);
                            self.persist();
                        }
                        if self.cfg.daemon.mode == Mode::Slideshow {
                            let _ = self.enter_mode(Mode::Slideshow);
                        }
                    }
                    self.publish();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let now = Instant::now();
            if now >= next_tick {
                self.tick();
                next_tick = now + self.tick_interval();
            }
            if now >= next_publish {
                self.init_brightness();
                self.publish();
                next_publish = now + Duration::from_secs(1);
            }
        }
        self.video = None;
        self.importer.cancel();
        info!("démon arrêté");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::thread::JoinHandle;

    pub struct Running {
        pub handle: Handle,
        pub stop: Arc<AtomicBool>,
        pub dir: PathBuf,
        pub config_path: PathBuf,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Démarre un moteur sans écran, avec un fichier de configuration temporaire.
    pub fn start(
        name: &str,
        tweak: impl FnOnce(&mut Config),
        notifier: Option<Notifier>,
    ) -> Running {
        let dir =
            std::env::temp_dir().join(format!("lightning-daemon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.toml");
        let mut cfg = Config::default();
        cfg.output.outputs.clear();
        cfg.dashboard.fps = 20.0;
        tweak(&mut cfg);
        let stop = Arc::new(AtomicBool::new(false));
        let (mut engine, handle) = Engine::new(
            EngineOptions {
                config: cfg,
                config_path: Some(config_path.clone()),
                extend: false,
                demo: false,
                google: GoogleRuntime {
                    endpoints: google::Endpoints::default(),
                    token_path: dir.join("google.json"),
                    cache_root: dir.join("google-cache"),
                },
            },
            stop.clone(),
        )
        .unwrap();
        if let Some(n) = notifier {
            engine.set_notifier(n);
        }
        let thread = std::thread::spawn(move || engine.run());
        Running {
            handle,
            stop,
            dir,
            config_path,
            thread: Some(thread),
        }
    }

    pub fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(Instant::now() < deadline, "délai dépassé : {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn frame_pixel(handle: &Handle, x: u32, y: u32) -> [u8; 3] {
        let frame = handle.latest.lock().unwrap().clone().expect("une image");
        let rgba = frame.rgba.as_ref().map(|p| p.to_vec()).unwrap_or_else(|| {
            image::load_from_memory(&frame.jpeg)
                .unwrap()
                .into_rgba8()
                .into_raw()
        });
        let i = ((y * WIDTH + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    }

    fn near(a: [u8; 3], b: [u8; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 12)
    }

    pub fn write_png(path: &Path, w: u32, h: u32, left: [u8; 3], right: [u8; 3]) {
        let img = image::RgbImage::from_fn(w, h, |x, _| {
            image::Rgb(if x < w / 2 { left } else { right })
        });
        img.save(path).unwrap();
    }

    #[test]
    fn dashboard_runs_and_settings_apply_live_and_persist() {
        let run = start("settings", |_| {}, None);
        let h = &run.handle;
        wait_for("première image", || !h.frame_jpeg().is_empty());
        let st = h.state();
        assert_eq!(st.mode, "dashboard");
        assert_eq!(st.lcd_state, "disabled");
        assert_eq!(st.brightness, -1);
        assert_eq!(st.widgets, WIDGETS);
        assert!(!st.http_enabled);

        h.call(Request::SetFps(10.0)).unwrap();
        h.call(Request::SetWidgets(vec![
            "CPU".into(),
            "gpu".into(),
            "cpu".into(),
        ]))
        .unwrap();
        h.call(Request::SetOnExit(OnExit::Blank)).unwrap();
        h.call(Request::SetBrightness(70)).unwrap();
        let st = h.state();
        assert_eq!(st.fps, 10.0);
        assert_eq!(st.widgets, ["cpu", "gpu"]);
        assert_eq!(st.on_exit, "blank");
        assert_eq!(st.brightness, 70);
        assert_eq!(st.firmware_brightness, -1, "pas d'écran : rien de confirmé");

        // Persisté à chaque changement.
        let saved = config::load(Some(&run.config_path)).unwrap();
        assert_eq!(saved.dashboard.fps, 10.0);
        assert_eq!(saved.dashboard.widgets, ["cpu", "gpu"]);
        assert_eq!(saved.dashboard.brightness, Some(70));
        assert_eq!(saved.output.on_exit, OnExit::Blank);

        // Valeurs refusées : message clair, état inchangé.
        assert!(
            h.call(Request::SetFps(0.0))
                .unwrap_err()
                .contains("cadence")
        );
        assert!(h.call(Request::SetFps(f64::NAN)).is_err());
        assert!(h.call(Request::SetRotation(90)).is_err());
        assert!(h.call(Request::SetBrightness(101)).is_err());
        assert!(
            h.call(Request::SetWidgets(vec!["meteo".into()]))
                .unwrap_err()
                .contains("meteo")
        );
        assert_eq!(h.state().fps, 10.0);

        wait_for("cadence mesurée", || h.state().measured_fps > 5.0);
    }

    #[test]
    fn modes_color_image_and_rotation() {
        let run = start("modes", |_| {}, None);
        let h = &run.handle;

        h.call(Request::ShowColor("#FF0000".into())).unwrap();
        assert_eq!(h.state().mode, "color");
        assert_eq!(h.state().color, "ff0000");
        wait_for("rouge", || near(frame_pixel(h, 640, 400), [255, 0, 0]));
        assert!(h.call(Request::ShowColor("rouge".into())).is_err());
        assert_eq!(h.state().color, "ff0000");

        let img = run.dir.join("deux.png");
        write_png(&img, 1280, 800, [0, 0, 255], [0, 255, 0]);
        let path = img.to_string_lossy().into_owned();
        h.call(Request::ShowImage {
            path: path.clone(),
            fit: Fit::Stretch,
        })
        .unwrap();
        assert_eq!(h.state().mode, "image");
        assert_eq!(h.state().image_fit, "stretch");
        wait_for("bleu à gauche", || {
            near(frame_pixel(h, 100, 400), [0, 0, 255])
        });
        assert!(near(frame_pixel(h, 1180, 400), [0, 255, 0]));

        // Rotation de 180° appliquée à chaud au contenu fixe.
        h.call(Request::SetRotation(180)).unwrap();
        wait_for("vert à gauche", || {
            near(frame_pixel(h, 100, 400), [0, 255, 0])
        });
        h.call(Request::SetRotation(0)).unwrap();

        // Image introuvable : erreur, on reste sur l'image précédente.
        let err = h
            .call(Request::ShowImage {
                path: "/nonexistent/x.png".into(),
                fit: Fit::Cover,
            })
            .unwrap_err();
        assert!(err.contains("/nonexistent/x.png"), "{err}");
        assert_eq!(h.state().image_path, path);
        assert_eq!(h.state().mode, "image");

        h.call(Request::SetMode(Mode::Dashboard)).unwrap();
        assert_eq!(h.state().mode, "dashboard");
        // Le mode image se rappelle de la dernière image.
        h.call(Request::SetMode(Mode::Image)).unwrap();
        wait_for("bleu à gauche", || {
            near(frame_pixel(h, 100, 400), [0, 0, 255])
        });

        h.call(Request::Restore).unwrap();
        assert_eq!(h.state().mode, "msi_animation");
        assert!(
            h.call(Request::SetMode(Mode::Video))
                .unwrap_err()
                .contains("vidéo")
        );
        assert_eq!(h.state().mode, "msi_animation");

        let saved = config::load(Some(&run.config_path)).unwrap();
        assert_eq!(saved.daemon.mode, Mode::MsiAnimation);
        assert_eq!(saved.daemon.image_path, path);
    }

    #[test]
    fn http_preview_can_be_toggled_live() {
        let run = start("http", |_| {}, None);
        let h = &run.handle;
        // Port libre choisi par le système, puis réutilisé.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr = format!("127.0.0.1:{port}");
        h.call(Request::SetHttp {
            enabled: true,
            addr: addr.clone(),
        })
        .unwrap();
        assert!(h.state().http_enabled);
        assert_eq!(h.state().http_addr, addr);
        wait_for("serveur joignable", || {
            std::net::TcpStream::connect(&addr).is_ok()
        });

        h.call(Request::SetHttp {
            enabled: false,
            addr: String::new(),
        })
        .unwrap();
        assert!(!h.state().http_enabled);
        wait_for("serveur arrêté", || {
            std::net::TcpStream::connect(&addr).is_err()
        });
        // Réactivation sur le même port.
        h.call(Request::SetHttp {
            enabled: true,
            addr: String::new(),
        })
        .unwrap();
        wait_for("serveur relancé", || {
            std::net::TcpStream::connect(&addr).is_ok()
        });
        assert!(
            h.call(Request::SetHttp {
                enabled: true,
                addr: "ici".into()
            })
            .is_err()
        );
        let saved = config::load(Some(&run.config_path)).unwrap();
        assert!(saved.output.outputs.contains(&OutputKind::Http));
    }

    #[test]
    fn reload_applies_the_file_and_notifier_sees_changes() {
        let seen = Arc::new(Mutex::new(Vec::<PublicState>::new()));
        let sink = seen.clone();
        let run = start(
            "reload",
            |_| {},
            Some(Box::new(move |s| sink.lock().unwrap().push(s.clone()))),
        );
        let h = &run.handle;
        std::fs::write(
            &run.config_path,
            "[dashboard]\nfps = 4.0\nwidgets = [\"network\"]\n[output]\non_exit = \"restore\"\n\
             [daemon]\nmode = \"color\"\ncolor = \"00ff00\"\n",
        )
        .unwrap();
        h.call(Request::ReloadConfig).unwrap();
        let st = h.state();
        assert_eq!(
            (st.fps, st.mode.as_str(), st.on_exit.as_str()),
            (4.0, "color", "restore")
        );
        assert_eq!(st.widgets, ["network"]);
        wait_for("vert", || near(frame_pixel(h, 10, 10), [0, 255, 0]));
        assert!(seen.lock().unwrap().iter().any(|s| s.mode == "color"));

        // Fichier invalide : erreur, réglages conservés.
        std::fs::write(&run.config_path, "[dashboard]\nfps = -1\n").unwrap();
        assert!(h.call(Request::ReloadConfig).is_err());
        assert_eq!(h.state().fps, 4.0);

        // SaveConfig réécrit un fichier valide.
        h.call(Request::SaveConfig).unwrap();
        assert_eq!(
            config::load(Some(&run.config_path)).unwrap().dashboard.fps,
            4.0
        );
    }

    #[test]
    fn slideshow_mode_settings_and_google_guards() {
        let run = start("slideshow", |_| {}, None);
        let h = &run.handle;
        let photos = run.dir.join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        write_png(&photos.join("a.png"), 64, 40, [250, 0, 0], [250, 0, 0]);
        write_png(&photos.join("b.png"), 64, 40, [0, 250, 0], [0, 250, 0]);
        let cfg = SlideshowConfig {
            folders: vec![photos.to_string_lossy().into_owned()],
            interval_s: 1.0,
            order: crate::slideshow::Order::Chronological,
            fit: crate::slideshow::SlideFit::Cover,
            transition_ms: 0,
            overlay: false,
            clock: false,
        };
        h.call(Request::SetSlideshow(cfg.clone())).unwrap();
        assert_eq!(
            h.state().mode,
            "dashboard",
            "les réglages ne changent pas le mode"
        );
        assert!(
            h.call(Request::SlideshowNext)
                .unwrap_err()
                .contains("diaporama")
        );
        h.call(Request::SetMode(Mode::Slideshow)).unwrap();
        wait_for("deux photos trouvées", || h.state().slideshow_count == 2);
        wait_for("une photo affichée", || {
            let p = frame_pixel(h, 640, 400);
            near(p, [250, 0, 0]) || near(p, [0, 250, 0])
        });
        let first = frame_pixel(h, 640, 400);
        assert!(h.state().slideshow_current.ends_with(".png"));
        h.call(Request::SlideshowNext).unwrap();
        wait_for("photo suivante", || {
            let p = frame_pixel(h, 640, 400);
            !near(p, first) && (near(p, [250, 0, 0]) || near(p, [0, 250, 0]))
        });

        let st = h.state();
        assert_eq!(st.slideshow_folders, cfg.folders);
        assert_eq!(
            (
                st.slideshow_interval,
                st.slideshow_order.as_str(),
                st.slideshow_fit.as_str()
            ),
            (1.0, "chronological", "cover")
        );
        let saved = config::load(Some(&run.config_path)).unwrap();
        assert_eq!(saved.slideshow, cfg);
        assert_eq!(saved.daemon.mode, Mode::Slideshow);

        // Réglages refusés.
        let bad = SlideshowConfig {
            interval_s: 0.0,
            ..cfg.clone()
        };
        assert!(h.call(Request::SetSlideshow(bad)).is_err());
        let relative = SlideshowConfig {
            folders: vec!["photos".into()],
            ..cfg.clone()
        };
        assert!(
            h.call(Request::SetSlideshow(relative))
                .unwrap_err()
                .contains("absolu")
        );
        assert_eq!(h.state().slideshow_folders, cfg.folders);

        // Google : rien sans identifiants ; le cache se vide et se compte.
        assert_eq!(h.state().google_state, "unconfigured");
        assert!(
            h.call(Request::GoogleStart)
                .unwrap_err()
                .contains("client_id")
        );
        h.call(Request::SetGoogleCredentials {
            client_id: " abc ".into(),
            client_secret: "s".into(),
        })
        .unwrap();
        assert_eq!(h.state().google_state, "idle");
        assert_eq!(h.state().google_client_id, "abc");
        assert!(
            config::load(Some(&run.config_path))
                .unwrap()
                .google
                .is_configured()
        );
        h.call(Request::GoogleClearCache).unwrap();
        assert_eq!(h.state().google_cached_count, 0);
        assert!(h.state().google_cache_dir.ends_with("google-cache"));
    }

    #[test]
    fn clock_and_now_playing_modes() {
        let run = start("clock", |_| {}, None);
        let h = &run.handle;
        h.call(Request::SetClock(ClockConfig {
            seconds: true,
            date: false,
        }))
        .unwrap();
        assert_eq!(
            h.state().mode,
            "dashboard",
            "les réglages ne changent pas le mode"
        );
        assert!(h.state().clock_seconds && !h.state().clock_date);
        h.call(Request::SetMode(Mode::Clock)).unwrap();
        assert_eq!(h.state().mode, "clock");
        // Fond sombre et grands chiffres clairs au centre.
        wait_for("horloge diffusée", || {
            !h.frame_jpeg().is_empty() && near(frame_pixel(h, 2, 2), [9, 11, 16])
        });
        let bright = (300..980)
            .step_by(4)
            .filter(|x| frame_pixel(h, *x, 330).iter().all(|c| *c > 200))
            .count();
        assert!(bright > 10, "chiffres attendus ({bright})");

        h.call(Request::SetNowPlaying {
            player: " spotify ".into(),
        })
        .unwrap();
        assert_eq!(h.state().now_playing_player, "spotify");
        assert!(
            h.call(Request::SetNowPlaying {
                player: "deux mots".into()
            })
            .is_err()
        );
        h.call(Request::SetMode(Mode::NowPlaying)).unwrap();
        assert_eq!(h.state().mode, "now_playing");
        // Aucun lecteur « spotify » sur la machine de test : écran d'attente.
        wait_for("écran de lecture", || !h.frame_jpeg().is_empty());

        let saved = config::load(Some(&run.config_path)).unwrap();
        assert!(saved.clock.seconds && !saved.clock.date);
        assert_eq!(saved.now_playing.player, "spotify");
        assert_eq!(saved.daemon.mode, Mode::NowPlaying);
        h.call(Request::SetMode(Mode::Dashboard)).unwrap();
        assert!(h.state().now_playing_title.is_empty());
    }

    #[test]
    fn unusable_startup_mode_falls_back_to_dashboard() {
        let run = start(
            "fallback",
            |cfg| {
                cfg.daemon.mode = Mode::Image;
                cfg.daemon.image_path = "/nonexistent/photo.jpg".into();
            },
            None,
        );
        assert_eq!(run.handle.state().mode, "dashboard");
        wait_for("tableau de bord", || !run.handle.frame_jpeg().is_empty());
    }
}
