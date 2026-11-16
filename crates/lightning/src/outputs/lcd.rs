//! Sortie `lcd` : l'écran réel, piloté par un fil dédié.
//!
//! Le fil ne garde que la dernière image, se reconnecte seul et publie son état.
//! Un écran absent, occupé ou débranché ne bloque donc jamais les autres sorties.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use lightning_lcd::{Display, HidControl, OpenOptions};
use tracing::{debug, info, warn};

use super::{Frame, LcdState, SharedStats, Sink};

pub use lightning_ipc::OnExit;

/// Le canal HID ne supporte qu'un interlocuteur à la fois : verrou partagé par
/// tout ce qui parle au firmware dans ce processus (connexion, luminosité, retour
/// à l'animation d'origine).
pub static HID_LOCK: Mutex<()> = Mutex::new(());

fn restore_firmware() {
    let _hid = HID_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match HidControl::open().and_then(|mut hid| hid.restore()) {
        Ok(Some(st)) if !st.is_extended() => info!("écran rendu au firmware (animation d'origine)"),
        Ok(st) => warn!("le firmware n'a pas quitté le mode étendu : {st:?}"),
        Err(e) => warn!("retour au firmware impossible : {e}"),
    }
}

const RETRY_ABSENT: Duration = Duration::from_secs(5);
const RETRY_LOST: Duration = Duration::from_secs(3);

#[derive(Default)]
struct Slot {
    pending: Option<Arc<Frame>>,
    /// Numéro de la dernière image réellement envoyée à l'écran.
    delivered: u64,
    /// Erreur rencontrée depuis la dernière image envoyée.
    failed: Option<String>,
    closing: bool,
    /// Écran laissé au firmware : aucune connexion tant que c'est vrai.
    suspended: bool,
    /// Demande d'envoi de la commande de retour au firmware.
    restore_requested: bool,
    on_exit: OnExit,
}

/// Commande de la sortie `lcd` depuis un autre fil (démon).
#[derive(Clone)]
pub struct LcdControl {
    shared: Arc<Shared>,
}

impl LcdControl {
    /// Libère l'écran et le rend au firmware (animation MSI d'origine).
    pub fn suspend(&self) {
        let mut slot = self.shared.slot.lock().unwrap_or_else(|e| e.into_inner());
        slot.suspended = true;
        slot.restore_requested = true;
        slot.pending = None;
        self.shared.wake.notify_all();
    }

    /// Reprend la main sur l'écran (rebascule en mode étendu à la connexion).
    pub fn resume(&self) {
        let mut slot = self.shared.slot.lock().unwrap_or_else(|e| e.into_inner());
        slot.suspended = false;
        slot.restore_requested = false;
        self.shared.wake.notify_all();
    }

    pub fn set_on_exit(&self, on_exit: OnExit) {
        self.shared
            .slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .on_exit = on_exit;
    }
}

struct Shared {
    slot: Mutex<Slot>,
    wake: Condvar,
}

pub struct LcdSink {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    last_seq: u64,
    stop: Arc<AtomicBool>,
}

/// Réglages de la sortie `lcd`.
#[derive(Clone)]
pub struct LcdOptions {
    pub extend: bool,
    pub quality: u8,
    pub on_exit: OnExit,
    /// Image affichée à l'arrêt quand `on_exit = blank` (JPEG).
    pub blank_jpeg: Option<Vec<u8>>,
}

impl LcdSink {
    pub fn spawn(opts: LcdOptions, stats: SharedStats, stop: Arc<AtomicBool>) -> Self {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot {
                on_exit: opts.on_exit,
                ..Slot::default()
            }),
            wake: Condvar::new(),
        });
        stats.set_lcd(LcdState::Connecting, "");
        let worker = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("lcd".into())
                .spawn(move || worker(&shared, &opts, &stats))
                .expect("création du fil lcd")
        };
        Self {
            shared,
            worker: Some(worker),
            last_seq: 0,
            stop,
        }
    }
}

fn open(opts: &LcdOptions) -> lightning_lcd::error::Result<Display> {
    let _hid = HID_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    Display::open_with(OpenOptions {
        extend: opts.extend,
        jpeg_quality: opts.quality,
        wait: Duration::from_secs(3),
        retries: 2,
        auto_reconnect: false,
    })
}

fn worker(shared: &Shared, opts: &LcdOptions, stats: &SharedStats) {
    let mut display: Option<Display> = None;
    let mut next_attempt = Instant::now();
    let mut ever_connected = false;
    let mut current: Option<Arc<Frame>> = None;
    let mut suspended = false;
    loop {
        // Attente d'une nouvelle image, d'une échéance de reconnexion ou de l'arrêt.
        {
            let mut slot = shared.slot.lock().unwrap_or_else(|e| e.into_inner());
            if slot.pending.is_none() && !slot.closing {
                slot = shared
                    .wake
                    .wait_timeout(slot, Duration::from_millis(200))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            if let Some(frame) = slot.pending.take() {
                current = Some(frame);
            }
            if slot.closing && (current.is_none() || display.is_none() || slot.suspended) {
                break;
            }
            suspended = slot.suspended;
            if suspended {
                current = None;
            }
            let restore = std::mem::take(&mut slot.restore_requested);
            drop(slot);
            if restore {
                display = None; // libère l'interface USB et le verrou avant le HID
                restore_firmware();
                stats.set_lcd(
                    LcdState::Disabled,
                    lightning_i18n::tr("écran rendu au firmware (animation MSI)"),
                );
            }
        }
        if suspended {
            continue;
        }

        if display.is_none() && Instant::now() >= next_attempt {
            match open(opts) {
                Ok(d) => {
                    info!(mode = ?d.mode(), "écran connecté");
                    if ever_connected {
                        stats.lock().lcd.reconnects += 1;
                    }
                    ever_connected = true;
                    stats.set_lcd(
                        LcdState::Connected,
                        lightning_i18n::trf(
                            "mode {mode}, {id}",
                            &[
                                ("mode", &format!("{:?}", d.mode())),
                                ("id", &crate::usb_label(d.pid())),
                            ],
                        ),
                    );
                    display = Some(d);
                }
                Err(e) => {
                    let delay = if ever_connected {
                        RETRY_LOST
                    } else {
                        RETRY_ABSENT
                    };
                    debug!("écran indisponible : {e}");
                    stats.set_lcd(LcdState::Absent, e.to_string());
                    shared.slot.lock().unwrap_or_else(|e| e.into_inner()).failed =
                        Some(e.to_string());
                    next_attempt = Instant::now() + delay;
                }
            }
        }

        let (Some(d), Some(frame)) = (display.as_mut(), current.as_ref()) else {
            continue;
        };
        let result = if frame.still {
            d.show_still(&frame.jpeg)
        } else {
            d.show_jpeg(&frame.jpeg)
        };
        match result {
            Ok(()) => {
                stats.lock().lcd.frames_sent += 1;
                let mut slot = shared.slot.lock().unwrap_or_else(|e| e.into_inner());
                slot.delivered = frame.seq;
                slot.failed = None;
                current = None;
                shared.wake.notify_all();
            }
            Err(e) => {
                warn!("écran : envoi interrompu ({e}), reconnexion");
                {
                    let mut s = stats.lock();
                    s.lcd.errors += 1;
                    s.lcd.state = LcdState::Reconnecting;
                    s.lcd.detail = e.to_string();
                }
                display = None; // libère l'interface USB et le verrou
                next_attempt = Instant::now() + RETRY_LOST;
                // `current` est conservée : elle sera renvoyée après reconnexion.
            }
        }
    }

    let on_exit = shared
        .slot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .on_exit;
    if suspended {
        // L'écran est déjà rendu au firmware : rien à faire.
    } else if let Some(mut d) = display.take() {
        match on_exit {
            OnExit::Keep => {}
            OnExit::Blank => {
                if let Some(jpeg) = &opts.blank_jpeg
                    && let Err(e) = d.show_jpeg(jpeg)
                {
                    warn!("image d'arrêt non affichée : {e}");
                }
            }
            OnExit::Restore => {
                drop(d);
                restore_firmware();
            }
        }
    }
    stats.set_lcd(LcdState::Disabled, lightning_i18n::tr("arrêté"));
    shared.wake.notify_all();
}

impl LcdSink {
    /// Poignée de commande utilisable depuis un autre fil.
    pub fn control(&self) -> LcdControl {
        LcdControl {
            shared: self.shared.clone(),
        }
    }
}

impl Sink for LcdSink {
    fn name(&self) -> &'static str {
        "lcd"
    }

    fn is_preview(&self) -> bool {
        false
    }

    fn present(&mut self, frame: &Arc<Frame>) {
        self.last_seq = frame.seq;
        let mut slot = self.shared.slot.lock().unwrap_or_else(|e| e.into_inner());
        if slot.suspended {
            return;
        }
        slot.pending = Some(frame.clone());
        self.shared.wake.notify_all();
    }

    /// Attend que la dernière image soit réellement partie vers l'écran.
    fn finish(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(45);
        let mut slot = self.shared.slot.lock().unwrap_or_else(|e| e.into_inner());
        while slot.delivered < self.last_seq {
            if let Some(err) = &slot.failed {
                bail!(lightning_i18n::trf("écran : {error}", &[("error", err)]));
            }
            if Instant::now() >= deadline || self.stop.load(Ordering::SeqCst) {
                bail!(lightning_i18n::tr(
                    "écran : image non affichée (délai dépassé ou arrêt demandé)"
                ));
            }
            slot = self
                .shared
                .wake
                .wait_timeout(slot, Duration::from_millis(200))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        Ok(())
    }
}

impl Drop for LcdSink {
    fn drop(&mut self) {
        {
            let mut slot = self.shared.slot.lock().unwrap_or_else(|e| e.into_inner());
            slot.closing = true;
            slot.pending = None;
            self.shared.wake.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
