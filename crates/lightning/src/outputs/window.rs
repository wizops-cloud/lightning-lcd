//! Sortie `window` : fenêtre d'aperçu sur le bureau (winit + softbuffer,
//! Wayland natif ou X11).
//!
//! La fenêtre affiche la dernière image à l'échelle, proportions conservées,
//! avec un bandeau de debug optionnel. Touches : `O` bandeau, `R` rotation de
//! l'aperçu, `Échap` ou `Q` pour quitter. Fermer la fenêtre arrête le programme.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use tracing::{debug, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::platform::wayland::{ActiveEventLoopExtWayland, EventLoopBuilderExtWayland};
use winit::platform::x11::EventLoopBuilderExtX11;
use winit::window::{Window, WindowId};

use super::{Frame, HEIGHT, SharedStats, Sink, Stats, WIDTH};
use crate::text::{TextPainter, blit};

/// Réglages de la fenêtre d'aperçu.
#[derive(Debug, Clone)]
pub struct WindowOptions {
    /// Afficher le bandeau de debug.
    pub overlay: bool,
    /// Rotation simulée de l'aperçu : 0, 90, 180 ou 270.
    pub rotation: u16,
}

struct Shared {
    latest: Mutex<Option<Arc<Frame>>>,
    closing: AtomicBool,
}

pub struct WindowSink {
    shared: Arc<Shared>,
    proxy: EventLoopProxy<()>,
    thread: Option<JoinHandle<()>>,
}

impl WindowSink {
    /// Ouvre la fenêtre dans un fil dédié ; échoue s'il n'y a pas de session graphique.
    pub fn spawn(opts: WindowOptions, stats: SharedStats, stop: Arc<AtomicBool>) -> Result<Self> {
        let shared = Arc::new(Shared {
            latest: Mutex::new(None),
            closing: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::channel::<Result<EventLoopProxy<()>>>();
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("window".into())
                .spawn(move || {
                    let mut builder = EventLoop::<()>::with_user_event();
                    EventLoopBuilderExtWayland::with_any_thread(&mut builder, true);
                    EventLoopBuilderExtX11::with_any_thread(&mut builder, true);
                    let event_loop = match builder.build() {
                        Ok(event_loop) => event_loop,
                        Err(e) => {
                            let _ = tx.send(Err(anyhow!("session graphique indisponible : {e}")));
                            return;
                        }
                    };
                    let _ = tx.send(Ok(event_loop.create_proxy()));
                    let mut app = App {
                        opts,
                        shared,
                        stats,
                        stop,
                        window: None,
                        surface: None,
                        painter: TextPainter::new(),
                        shown_seq: 0,
                        presented: 0,
                    };
                    if let Err(e) = event_loop.run_app(&mut app) {
                        warn!("fenêtre d'aperçu : {e}");
                    }
                    debug!("fenêtre d'aperçu fermée");
                })
                .context("création du fil de la fenêtre")?
        };
        let proxy = rx
            .recv_timeout(Duration::from_secs(10))
            .context("la fenêtre d'aperçu n'a pas démarré")??;
        Ok(Self {
            shared,
            proxy,
            thread: Some(thread),
        })
    }
}

impl Sink for WindowSink {
    fn name(&self) -> &'static str {
        "window"
    }

    fn needs_pixels(&self) -> bool {
        true
    }

    fn present(&mut self, frame: &Arc<Frame>) {
        *self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame.clone());
        let _ = self.proxy.send_event(());
    }
}

impl Drop for WindowSink {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::SeqCst);
        let _ = self.proxy.send_event(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

type Surface = softbuffer::Surface<Arc<Window>, Arc<Window>>;

struct App {
    opts: WindowOptions,
    shared: Arc<Shared>,
    stats: SharedStats,
    stop: Arc<AtomicBool>,
    window: Option<Arc<Window>>,
    surface: Option<Surface>,
    painter: TextPainter,
    shown_seq: u64,
    presented: u64,
}

/// Lignes du bandeau de debug.
pub fn overlay_lines(stats: &Stats, rotation: u16) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{}  {:.1} img/s  rendu {:.1} ms  encodage {:.1} ms",
            stats.command, stats.fps, stats.render_ms, stats.encode_ms
        ),
        format!(
            "JPEG {} Ko  image n° {}  sorties : {}",
            stats.jpeg_bytes / 1024,
            stats.frames,
            stats.outputs.join(", ")
        ),
        format!(
            "écran : {}  ({} envoyées, {} erreurs, {} reconnexions)",
            stats.lcd.state.label(),
            stats.lcd.frames_sent,
            stats.lcd.errors,
            stats.lcd.reconnects
        ),
    ];
    if !stats.lcd.detail.is_empty() {
        lines.push(stats.lcd.detail.chars().take(90).collect());
    }
    if rotation != 0 {
        lines.push(format!("aperçu tourné de {rotation}°"));
    }
    lines
}

impl App {
    fn quit(&mut self, event_loop: &ActiveEventLoop, stop_program: bool) {
        if stop_program {
            self.stop.store(true, Ordering::SeqCst);
        }
        self.surface = None;
        self.window = None;
        event_loop.exit();
    }

    fn draw(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if let Err(e) = surface.resize(w, h) {
            warn!("fenêtre : redimensionnement impossible ({e})");
            return;
        }
        let mut buffer = match surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(e) => {
                warn!("fenêtre : tampon indisponible ({e})");
                return;
            }
        };
        let (dw, dh) = (size.width as usize, size.height as usize);
        let frame = self
            .shared
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match frame
            .as_ref()
            .and_then(|f| f.rgba.as_ref().map(|p| (f.seq, p)))
        {
            Some((seq, pixels)) => {
                blit(
                    pixels,
                    WIDTH as usize,
                    HEIGHT as usize,
                    &mut buffer,
                    dw,
                    dh,
                    self.opts.rotation,
                );
                self.shown_seq = seq;
            }
            None => {
                buffer.fill(0x0010_1216);
                let msg = "en attente d'une image…";
                let tw = self.painter.measure(msg, 22.0);
                self.painter.draw(
                    &mut buffer,
                    dw,
                    dh,
                    (dw as f32 - tw) / 2.0,
                    dh as f32 / 2.0,
                    22.0,
                    (140, 150, 168),
                    msg,
                );
            }
        }
        if self.opts.overlay {
            let lines = overlay_lines(&self.stats.snapshot(), self.opts.rotation);
            self.painter.overlay(&mut buffer, dw, dh, &lines);
        }
        window.pre_present_notify();
        match buffer.present() {
            Ok(()) => {
                self.presented += 1;
                if self.presented == 1 {
                    info!(
                        width = dw,
                        height = dh,
                        "fenêtre d'aperçu : première image présentée"
                    );
                }
            }
            Err(e) => warn!("fenêtre : présentation impossible ({e})"),
        }
    }
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Lightning LCD — aperçu")
            .with_inner_size(LogicalSize::new(960.0, 600.0))
            .with_min_inner_size(LogicalSize::new(320.0, 200.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(e) => {
                warn!("fenêtre d'aperçu non créée : {e}");
                event_loop.exit();
                return;
            }
        };
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|context| softbuffer::Surface::new(&context, window.clone()));
        match surface {
            Ok(surface) => self.surface = Some(surface),
            Err(e) => {
                warn!("fenêtre d'aperçu : surface logicielle indisponible ({e})");
                event_loop.exit();
                return;
            }
        }
        let backend = if event_loop.is_wayland() {
            "Wayland"
        } else {
            "X11"
        };
        info!(backend, "fenêtre d'aperçu ouverte");
        window.request_redraw();
        self.window = Some(window);
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if self.shared.closing.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
            self.quit(event_loop, false);
            return;
        }
        // Réveil périodique : rafraîchit le bandeau et surveille l'arrêt.
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: ()) {
        if self.shared.closing.load(Ordering::SeqCst) {
            self.quit(event_loop, false);
        } else if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                info!("fenêtre d'aperçu fermée par l'utilisateur : arrêt");
                self.quit(event_loop, true);
            }
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key.as_ref() {
                    Key::Character("o" | "O") => self.opts.overlay = !self.opts.overlay,
                    Key::Character("r" | "R") => {
                        self.opts.rotation = (self.opts.rotation + 90) % 360
                    }
                    Key::Character("q" | "Q") | Key::Named(NamedKey::Escape) => {
                        self.quit(event_loop, true);
                        return;
                    }
                    _ => return,
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(500),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{LcdState, LcdStatus};
    use super::*;

    #[test]
    fn overlay_reports_fps_sizes_and_lcd_state() {
        let stats = Stats {
            command: "dashboard".into(),
            outputs: vec!["lcd".into(), "window".into()],
            frames: 42,
            fps: 2.04,
            render_ms: 7.4,
            encode_ms: 3.1,
            jpeg_bytes: 102_400,
            lcd: LcdStatus {
                state: LcdState::Reconnecting,
                detail: "erreur USB : No device".into(),
                frames_sent: 40,
                errors: 1,
                reconnects: 0,
            },
            ..Default::default()
        };
        let lines = overlay_lines(&stats, 90);
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains("2.0 img/s") && lines[0].contains("rendu 7.4 ms"));
        assert!(lines[1].contains("JPEG 100 Ko") && lines[1].contains("lcd, window"));
        assert!(lines[2].contains("reconnexion") && lines[2].contains("1 erreurs"));
        assert!(lines[3].contains("No device"));
        assert!(lines[4].contains("90"));
        assert_eq!(overlay_lines(&Stats::default(), 0).len(), 3);
    }
}
