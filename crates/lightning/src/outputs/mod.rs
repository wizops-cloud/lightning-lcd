//! Sorties d'affichage : chaque image produite (tableau de bord, couleur, image,
//! vidéo, flux) est diffusée vers une ou plusieurs sorties ([`Sink`]).
//!
//! - `lcd` : l'écran réel ;
//! - `window` : fenêtre d'aperçu sur le bureau ;
//! - `http` : serveur local (page, flux MJPEG, dernière image, état JSON).
//!
//! Aucune sortie ne bloque les autres : chacune a son fil d'exécution et ne
//! garde que la dernière image. L'état de l'écran (connecté, reconnexion,
//! absent…) est publié dans [`Stats`].

pub mod http;
pub mod lcd;
#[cfg(feature = "window")]
pub mod window;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::Result;
use lightning_lcd::display::encode_jpeg;
use serde::Serialize;

/// Largeur des images diffusées.
pub const WIDTH: u32 = lightning_lcd::WIDTH;
/// Hauteur des images diffusées.
pub const HEIGHT: u32 = lightning_lcd::HEIGHT;

/// Une image prête à être diffusée.
pub struct Frame {
    /// Numéro d'ordre, croissant.
    pub seq: u64,
    /// JPEG baseline 4:2:0 à la taille de l'écran.
    pub jpeg: Vec<u8>,
    /// Pixels RGBA d'origine, quand une sortie en a l'usage.
    pub rgba: Option<Vec<u8>>,
    /// Image fixe (commande ponctuelle) : l'écran la réémet brièvement pour
    /// qu'elle tienne après une énumération USB fraîche.
    pub still: bool,
}

/// État de la liaison avec l'écran réel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LcdState {
    /// La sortie `lcd` n'est pas activée.
    #[default]
    Disabled,
    /// Première connexion en cours.
    Connecting,
    Connected,
    /// Liaison perdue, nouvelle tentative en cours.
    Reconnecting,
    /// Écran introuvable ou inutilisable ; nouvelles tentatives périodiques.
    Absent,
}

impl LcdState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "désactivé",
            Self::Connecting => "connexion…",
            Self::Connected => "connecté",
            Self::Reconnecting => "reconnexion…",
            Self::Absent => "absent",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LcdStatus {
    pub state: LcdState,
    /// Dernière erreur ou précision sur l'état.
    pub detail: String,
    pub frames_sent: u64,
    pub errors: u64,
    pub reconnects: u64,
}

/// Métriques de debug partagées entre les sorties (overlay, `/status.json`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    pub command: String,
    pub outputs: Vec<String>,
    pub width: u32,
    pub height: u32,
    /// Images diffusées depuis le démarrage.
    pub frames: u64,
    /// Cadence réelle mesurée sur les dernières images.
    pub fps: f32,
    /// Durée de collecte + rendu de la dernière image (0 pour un flux externe).
    pub render_ms: f32,
    pub encode_ms: f32,
    pub jpeg_bytes: usize,
    pub uptime_s: f64,
    /// Adresse du serveur d'aperçu, quand la sortie `http` est active.
    pub http: Option<String>,
    pub lcd: LcdStatus,
}

/// État partagé, verrouillé brièvement par chaque sortie.
#[derive(Clone, Default)]
pub struct SharedStats(Arc<Mutex<Stats>>);

impl SharedStats {
    pub fn lock(&self) -> MutexGuard<'_, Stats> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> Stats {
        self.lock().clone()
    }

    pub fn set_lcd(&self, state: LcdState, detail: impl Into<String>) {
        let mut stats = self.lock();
        stats.lcd.state = state;
        stats.lcd.detail = detail.into();
    }
}

/// Une sortie d'affichage.
pub trait Sink: Send {
    fn name(&self) -> &'static str;

    /// Vrai si la sortie a besoin des pixels (une source JPEG est alors décodée).
    fn needs_pixels(&self) -> bool {
        false
    }

    /// Faux pour les sorties qui ne servent pas à regarder l'image depuis le
    /// bureau (écran réel, mémoire du démon).
    fn is_preview(&self) -> bool {
        true
    }

    /// Prend en charge une image. Ne doit pas bloquer.
    fn present(&mut self, frame: &Arc<Frame>);

    /// Attend la fin du travail en cours (image fixe réellement affichée, etc.).
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Dernière image diffusée, partagée avec qui veut la lire (démon D-Bus).
pub type LatestFrame = Arc<Mutex<Option<Arc<Frame>>>>;

/// Sortie `memory` : conserve simplement la dernière image.
pub struct MemorySink(pub LatestFrame);

impl Sink for MemorySink {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn is_preview(&self) -> bool {
        false
    }

    fn present(&mut self, frame: &Arc<Frame>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame.clone());
    }
}

/// Diffuseur : encode chaque image une seule fois et la remet à toutes les sorties.
pub struct Outputs {
    sinks: Vec<Box<dyn Sink>>,
    stats: SharedStats,
    quality: u8,
    seq: u64,
    started: Instant,
    recent: VecDeque<Instant>,
}

impl Outputs {
    pub fn new(sinks: Vec<Box<dyn Sink>>, stats: SharedStats, quality: u8, command: &str) -> Self {
        {
            let mut s = stats.lock();
            s.command = command.to_owned();
            s.outputs = sinks.iter().map(|k| k.name().to_owned()).collect();
            s.width = WIDTH;
            s.height = HEIGHT;
        }
        Self {
            sinks,
            stats,
            quality,
            seq: 0,
            started: Instant::now(),
            recent: VecDeque::with_capacity(32),
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.sinks.iter().map(|s| s.name()).collect()
    }

    /// Ajoute une sortie à chaud.
    pub fn add_sink(&mut self, sink: Box<dyn Sink>) {
        self.sinks.push(sink);
        self.stats.lock().outputs = self.sinks.iter().map(|k| k.name().to_owned()).collect();
    }

    /// Retire à chaud les sorties portant ce nom ; renvoie vrai s'il y en avait.
    pub fn remove_sink(&mut self, name: &str) -> bool {
        let before = self.sinks.len();
        self.sinks.retain(|s| s.name() != name);
        self.stats.lock().outputs = self.sinks.iter().map(|k| k.name().to_owned()).collect();
        self.sinks.len() != before
    }

    /// Métriques de debug courantes.
    pub fn stats(&self) -> Stats {
        self.stats.snapshot()
    }

    /// Vrai si une autre sortie que l'écran réel est active.
    pub fn has_preview(&self) -> bool {
        self.sinks.iter().any(|s| s.is_preview())
    }

    fn wants_pixels(&self) -> bool {
        self.sinks.iter().any(|s| s.needs_pixels())
    }

    /// Diffuse une image RGBA (4 octets par pixel, alpha ignoré).
    pub fn present_rgba(&mut self, rgba: &[u8], render: Duration, still: bool) -> Result<()> {
        let t0 = Instant::now();
        let jpeg = encode_jpeg(rgba, WIDTH, HEIGHT, 4, self.quality)?;
        let encode = t0.elapsed();
        let pixels = self.has_preview().then(|| rgba.to_vec());
        self.publish(jpeg, pixels, still, render, encode);
        Ok(())
    }

    /// Diffuse une image déjà encodée en JPEG (vidéo, flux).
    pub fn present_jpeg(&mut self, jpeg: Vec<u8>, still: bool) -> Result<()> {
        let pixels = if self.wants_pixels() {
            match image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg) {
                Ok(img) if (img.width(), img.height()) == (WIDTH, HEIGHT) => {
                    Some(img.into_rgba8().into_raw())
                }
                Ok(img) => {
                    tracing::debug!(
                        w = img.width(),
                        h = img.height(),
                        "taille de JPEG inattendue"
                    );
                    None
                }
                Err(e) => {
                    tracing::debug!("JPEG non décodable pour l'aperçu : {e}");
                    None
                }
            }
        } else {
            None
        };
        self.publish(jpeg, pixels, still, Duration::ZERO, Duration::ZERO);
        Ok(())
    }

    fn publish(
        &mut self,
        jpeg: Vec<u8>,
        rgba: Option<Vec<u8>>,
        still: bool,
        render: Duration,
        encode: Duration,
    ) {
        self.seq += 1;
        let now = Instant::now();
        if self.recent.len() == 30 {
            self.recent.pop_front();
        }
        self.recent.push_back(now);
        let fps = match (self.recent.front(), self.recent.len()) {
            (Some(first), n) if n >= 2 => {
                (n - 1) as f32 / now.duration_since(*first).as_secs_f32().max(1e-3)
            }
            _ => 0.0,
        };
        {
            let mut s = self.stats.lock();
            s.frames = self.seq;
            s.fps = fps;
            s.render_ms = render.as_secs_f32() * 1000.0;
            s.encode_ms = encode.as_secs_f32() * 1000.0;
            s.jpeg_bytes = jpeg.len();
            s.uptime_s = self.started.elapsed().as_secs_f64();
        }
        let frame = Arc::new(Frame {
            seq: self.seq,
            jpeg,
            rgba,
            still,
        });
        for sink in &mut self.sinks {
            sink.present(&frame);
        }
    }

    /// Attend que chaque sortie ait fini ; renvoie la première erreur.
    pub fn finish(&mut self) -> Result<()> {
        let mut first = Ok(());
        for sink in &mut self.sinks {
            if let Err(e) = sink.finish()
                && first.is_ok()
            {
                first = Err(e);
            }
        }
        first
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `(numéro, taille du JPEG, pixels joints, image fixe)` par image reçue.
    type Seen = Arc<Mutex<Vec<(u64, usize, bool, bool)>>>;

    /// Sortie factice qui mémorise ce qu'elle reçoit.
    pub struct Recorder {
        pub name: &'static str,
        pub pixels: bool,
        pub seen: Seen,
    }

    impl Sink for Recorder {
        fn name(&self) -> &'static str {
            self.name
        }
        fn needs_pixels(&self) -> bool {
            self.pixels
        }
        fn is_preview(&self) -> bool {
            self.name != "lcd"
        }
        fn present(&mut self, frame: &Arc<Frame>) {
            self.seen.lock().unwrap().push((
                frame.seq,
                frame.jpeg.len(),
                frame.rgba.is_some(),
                frame.still,
            ));
        }
    }

    fn recorder(name: &'static str, pixels: bool) -> (Box<dyn Sink>, Seen) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Box::new(Recorder {
                name,
                pixels,
                seen: seen.clone(),
            }),
            seen,
        )
    }

    pub fn test_rgba(value: u8) -> Vec<u8> {
        vec![value; (WIDTH * HEIGHT * 4) as usize]
    }

    #[test]
    fn every_sink_gets_every_frame_encoded_once() {
        let (a, seen_a) = recorder("lcd", false);
        let (b, seen_b) = recorder("http", false);
        let (c, seen_c) = recorder("window", true);
        let stats = SharedStats::default();
        let mut out = Outputs::new(vec![a, b, c], stats.clone(), 80, "test");
        assert_eq!(out.names(), ["lcd", "http", "window"]);

        out.present_rgba(&test_rgba(40), Duration::from_millis(7), false)
            .unwrap();
        out.present_rgba(&test_rgba(200), Duration::from_millis(9), true)
            .unwrap();

        let a = seen_a.lock().unwrap().clone();
        assert_eq!(a.len(), 2);
        assert_eq!(a, *seen_b.lock().unwrap());
        assert_eq!(a, *seen_c.lock().unwrap());
        assert_eq!((a[0].0, a[1].0), (1, 2));
        assert!(a[0].2 && a[1].2, "pixels joints quand un aperçu est actif");
        assert!(!a[0].3 && a[1].3);

        let s = stats.snapshot();
        assert_eq!(s.frames, 2);
        assert_eq!(s.outputs, ["lcd", "http", "window"]);
        assert_eq!((s.width, s.height), (1280, 800));
        assert!((s.render_ms - 9.0).abs() < 0.01);
        assert_eq!(s.jpeg_bytes, a[1].1);
        assert!(s.fps > 0.0);
        assert_eq!(s.lcd.state, LcdState::Disabled);
    }

    #[test]
    fn lcd_only_skips_pixel_copies_and_jpeg_decoding() {
        let (a, seen) = recorder("lcd", false);
        let mut out = Outputs::new(vec![a], SharedStats::default(), 80, "test");
        assert!(!out.has_preview());
        out.present_rgba(&test_rgba(1), Duration::ZERO, false)
            .unwrap();
        out.present_jpeg(vec![0xFF, 0xD8, 0xFF, 0xD9], false)
            .unwrap();
        let seen = seen.lock().unwrap();
        assert!(!seen[0].2 && !seen[1].2);
        assert_eq!(seen[1].1, 4);
    }

    #[test]
    fn jpeg_sources_are_decoded_only_for_sinks_that_need_pixels() {
        let jpeg = encode_jpeg(&test_rgba(90), WIDTH, HEIGHT, 4, 70).unwrap();
        let (http, seen_http) = recorder("http", false);
        let mut out = Outputs::new(vec![http], SharedStats::default(), 80, "test");
        out.present_jpeg(jpeg.clone(), false).unwrap();
        assert!(!seen_http.lock().unwrap()[0].2);

        let (window, seen_window) = recorder("window", true);
        let mut out = Outputs::new(vec![window], SharedStats::default(), 80, "test");
        out.present_jpeg(jpeg, false).unwrap();
        out.present_jpeg(vec![1, 2, 3], false).unwrap(); // JPEG invalide : pas de panique
        let seen = seen_window.lock().unwrap();
        assert!(seen[0].2);
        assert!(!seen[1].2);
    }

    #[test]
    fn sinks_can_be_added_and_removed_while_running() {
        let (lcd, seen_lcd) = recorder("lcd", false);
        let stats = SharedStats::default();
        let latest = LatestFrame::default();
        let mut out = Outputs::new(
            vec![lcd, Box::new(MemorySink(latest.clone()))],
            stats.clone(),
            80,
            "test",
        );
        out.present_rgba(&test_rgba(3), Duration::ZERO, false)
            .unwrap();
        assert_eq!(latest.lock().unwrap().as_ref().unwrap().seq, 1);

        let (http, seen_http) = recorder("http", false);
        out.add_sink(http);
        assert_eq!(stats.snapshot().outputs, ["lcd", "memory", "http"]);
        out.present_rgba(&test_rgba(4), Duration::ZERO, false)
            .unwrap();
        assert_eq!(seen_http.lock().unwrap().len(), 1);

        assert!(out.remove_sink("http"));
        assert!(!out.remove_sink("http"));
        out.present_rgba(&test_rgba(5), Duration::ZERO, false)
            .unwrap();
        assert_eq!(seen_http.lock().unwrap().len(), 1);
        assert_eq!(seen_lcd.lock().unwrap().len(), 3);
        assert_eq!(stats.snapshot().outputs, ["lcd", "memory"]);
        assert_eq!(latest.lock().unwrap().as_ref().unwrap().seq, 3);
    }

    #[test]
    fn memory_sink_alone_is_not_a_preview() {
        let mut out = Outputs::new(
            vec![Box::new(MemorySink(LatestFrame::default()))],
            SharedStats::default(),
            80,
            "test",
        );
        assert!(!out.has_preview());
        out.present_rgba(&test_rgba(1), Duration::ZERO, false)
            .unwrap();
    }

    #[test]
    fn wrong_frame_size_is_an_error() {
        let (a, _) = recorder("lcd", false);
        let mut out = Outputs::new(vec![a], SharedStats::default(), 80, "test");
        assert!(out.present_rgba(&[0u8; 16], Duration::ZERO, false).is_err());
    }
}
