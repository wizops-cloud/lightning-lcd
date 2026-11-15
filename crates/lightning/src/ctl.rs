//! `lightning ctl` : client D-Bus du démon, pour les scripts et les tests.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::Subcommand;
use lightning_ipc::{Fit, LightningProxyBlocking, Mode, OnExit, SYSTEMD_UNIT};

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum CtlCmd {
    /// Affiche l'état du démon
    Status,
    /// Change le contenu : dashboard, image, video, color, msi_animation
    Mode { mode: Mode },
    /// Affiche une image
    Image {
        file: PathBuf,
        /// cover, contain ou stretch
        #[arg(long, default_value_t = Fit::Cover)]
        fit: Fit,
    },
    /// Lit une vidéo (fichier ou URL)
    Video {
        file: String,
        /// Ne pas reboucler
        #[arg(long)]
        once: bool,
    },
    /// Affiche une couleur unie (RRGGBB)
    Color { rgb: String },
    /// Rend l'écran au firmware (animation MSI d'origine)
    Restore,
    /// Luminosité de l'écran (0 à 100)
    Brightness { level: u8 },
    /// Cadence du tableau de bord (images par seconde)
    Fps { fps: f64 },
    /// Rotation logicielle : 0 ou 180
    Rotation { degrees: u16 },
    /// Panneaux du tableau de bord, dans l'ordre : gpu,cpu,memory,disks,network
    Widgets {
        #[arg(value_delimiter = ',')]
        widgets: Vec<String>,
    },
    /// Comportement à l'arrêt : keep, blank ou restore
    OnExit { behaviour: OnExit },
    /// Aperçu HTTP : on ou off
    Http {
        #[arg(value_parser = ["on", "off"])]
        state: String,
        /// Adresse d'écoute (IP:port) ; inchangée si absente
        #[arg(long)]
        addr: Option<String>,
    },
    /// Lance le diaporama sur des dossiers de photos (ou règle ses options)
    Slideshow {
        /// Dossiers parcourus récursivement ; aucun = dossiers déjà réglés
        folders: Vec<PathBuf>,
        /// Durée par photo, en secondes
        #[arg(long)]
        interval: Option<f64>,
        /// random ou chronological
        #[arg(long)]
        order: Option<String>,
        /// cover ou contain (photo entière sur fond flou)
        #[arg(long)]
        fit: Option<String>,
        /// Durée du fondu enchaîné en millisecondes (0 = aucun)
        #[arg(long)]
        transition_ms: Option<u32>,
        /// Masquer la légende (date et album)
        #[arg(long)]
        no_overlay: bool,
        /// Masquer l'horloge
        #[arg(long)]
        no_clock: bool,
    },
    /// Photo suivante du diaporama
    Next,
    /// Import Google Photos : start, cancel, clear ou status
    Google {
        #[arg(value_parser = ["start", "cancel", "clear", "status"])]
        action: String,
    },
    /// Enregistre la dernière image diffusée (JPEG)
    Frame { file: PathBuf },
    /// Écrit la configuration courante dans le fichier TOML
    Save,
    /// Relit le fichier TOML et applique ses réglages
    Reload,
}

/// Chemin absolu : le démon n'a pas le même répertoire courant que l'appelant.
fn absolute(path: &Path) -> Result<String> {
    let abs = std::path::absolute(path).with_context(|| format!("chemin {}", path.display()))?;
    abs.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("chemin non UTF-8 : {}", abs.display()))
}

/// Message d'erreur lisible pour un appel D-Bus raté.
fn explain(err: zbus::Error) -> anyhow::Error {
    match &err {
        zbus::Error::MethodError(name, detail, _) => {
            if name.as_str().ends_with("ServiceUnknown")
                || name.as_str().ends_with("NameHasNoOwner")
            {
                anyhow!("le démon ne tourne pas (systemctl --user start {SYSTEMD_UNIT})")
            } else {
                anyhow!("{}", detail.clone().unwrap_or_else(|| name.to_string()))
            }
        }
        zbus::Error::FDO(e) => match e.as_ref() {
            zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_) => {
                anyhow!("le démon ne tourne pas (systemctl --user start {SYSTEMD_UNIT})")
            }
            other => anyhow!("{other}"),
        },
        _ => anyhow!("{err}"),
    }
}

fn brightness_text(value: i32) -> String {
    if value < 0 {
        "inconnue".into()
    } else {
        format!("{value} %")
    }
}

/// Exécute une commande sur le démon ; le texte destiné à l'utilisateur va dans `out`.
pub fn run(proxy: &LightningProxyBlocking<'_>, cmd: CtlCmd, out: &mut impl Write) -> Result<()> {
    match cmd {
        CtlCmd::Status => {
            let p = |r: zbus::Result<String>| r.map_err(explain);
            writeln!(out, "démon          : version {}", p(proxy.version())?)?;
            let detail = p(proxy.lcd_detail())?;
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(" ({detail})")
            };
            writeln!(out, "écran          : {}{detail}", p(proxy.lcd_state())?)?;
            writeln!(out, "contenu        : {}", p(proxy.mode())?)?;
            writeln!(
                out,
                "luminosité     : demandée {}, firmware {}",
                brightness_text(proxy.brightness().map_err(explain)?),
                brightness_text(proxy.firmware_brightness().map_err(explain)?)
            )?;
            writeln!(
                out,
                "cadence        : {} img/s (mesurée {}), rendu {} ms",
                proxy.fps().map_err(explain)?,
                proxy.measured_fps().map_err(explain)?,
                proxy.render_ms().map_err(explain)?
            )?;
            writeln!(
                out,
                "rotation       : {}°",
                proxy.rotation().map_err(explain)?
            )?;
            writeln!(
                out,
                "panneaux       : {}",
                proxy.widgets().map_err(explain)?.join(", ")
            )?;
            writeln!(
                out,
                "image          : {} ({})",
                p(proxy.image_path())?,
                p(proxy.image_fit())?
            )?;
            writeln!(
                out,
                "vidéo          : {} ({})",
                p(proxy.video_path())?,
                if proxy.video_loop().map_err(explain)? {
                    "en boucle"
                } else {
                    "une fois"
                }
            )?;
            writeln!(out, "couleur        : {}", p(proxy.color())?)?;
            writeln!(
                out,
                "diaporama      : {} dossier(s), {} photos, {} s par photo, ordre {}",
                proxy.slideshow_folders().map_err(explain)?.len(),
                proxy.slideshow_count().map_err(explain)?,
                proxy.slideshow_interval().map_err(explain)?,
                p(proxy.slideshow_order())?
            )?;
            writeln!(out, "à l'arrêt      : {}", p(proxy.on_exit())?)?;
            let http = if proxy.http_enabled().map_err(explain)? {
                "actif"
            } else {
                "coupé"
            };
            writeln!(out, "aperçu HTTP    : {http} ({})", p(proxy.http_addr())?)?;
            writeln!(
                out,
                "images envoyées: {}",
                proxy.frames_sent().map_err(explain)?
            )?;
        }
        CtlCmd::Mode { mode } => proxy.set_mode(mode.as_str()).map_err(explain)?,
        CtlCmd::Image { file, fit } => {
            proxy
                .show_image(&absolute(&file)?, fit.as_str())
                .map_err(explain)?;
        }
        CtlCmd::Video { file, once } => {
            let target = if file.contains("://") {
                file
            } else {
                absolute(Path::new(&file))?
            };
            proxy.play_video(&target, !once).map_err(explain)?;
        }
        CtlCmd::Color { rgb } => proxy.show_color(&rgb).map_err(explain)?,
        CtlCmd::Restore => proxy.restore().map_err(explain)?,
        CtlCmd::Brightness { level } => proxy.set_brightness(level).map_err(explain)?,
        CtlCmd::Fps { fps } => proxy.set_fps(fps).map_err(explain)?,
        CtlCmd::Rotation { degrees } => proxy.set_rotation(degrees).map_err(explain)?,
        CtlCmd::Widgets { widgets } => {
            let refs: Vec<&str> = widgets.iter().map(String::as_str).collect();
            proxy.set_widgets(&refs).map_err(explain)?;
        }
        CtlCmd::OnExit { behaviour } => proxy.set_on_exit(behaviour.as_str()).map_err(explain)?,
        CtlCmd::Http { state, addr } => {
            proxy
                .set_http(state == "on", addr.as_deref().unwrap_or(""))
                .map_err(explain)?;
        }
        CtlCmd::Slideshow {
            folders,
            interval,
            order,
            fit,
            transition_ms,
            no_overlay,
            no_clock,
        } => {
            // Les options absentes gardent la valeur courante du démon.
            let folders: Vec<String> = if folders.is_empty() {
                proxy.slideshow_folders().map_err(explain)?
            } else {
                folders.iter().map(|f| absolute(f)).collect::<Result<_>>()?
            };
            let refs: Vec<&str> = folders.iter().map(String::as_str).collect();
            let interval = match interval {
                Some(v) => v,
                None => proxy.slideshow_interval().map_err(explain)?,
            };
            let order = match order {
                Some(v) => v,
                None => proxy.slideshow_order().map_err(explain)?,
            };
            let fit = match fit {
                Some(v) => v,
                None => proxy.slideshow_fit().map_err(explain)?,
            };
            let transition = match transition_ms {
                Some(v) => v,
                None => proxy.slideshow_transition_ms().map_err(explain)?,
            };
            let overlay = !no_overlay && proxy.slideshow_overlay().map_err(explain)?;
            let clock = !no_clock && proxy.slideshow_clock().map_err(explain)?;
            proxy
                .set_slideshow(&refs, interval, &order, &fit, transition, overlay, clock)
                .map_err(explain)?;
            proxy.set_mode(Mode::Slideshow.as_str()).map_err(explain)?;
        }
        CtlCmd::Next => proxy.slideshow_next().map_err(explain)?,
        CtlCmd::Google { action } => match action.as_str() {
            "start" => {
                proxy.google_photos_start().map_err(explain)?;
                writeln!(
                    out,
                    "import démarré : suivre « lightning ctl google status » et ouvrir l'adresse indiquée"
                )?;
            }
            "cancel" => proxy.google_photos_cancel().map_err(explain)?,
            "clear" => proxy.google_photos_clear_cache().map_err(explain)?,
            _ => {
                writeln!(out, "état    : {}", proxy.google_state().map_err(explain)?)?;
                writeln!(out, "détail  : {}", proxy.google_detail().map_err(explain)?)?;
                writeln!(out, "adresse : {}", proxy.google_url().map_err(explain)?)?;
                writeln!(
                    out,
                    "cache   : {} photos dans {}",
                    proxy.google_cached_count().map_err(explain)?,
                    proxy.google_cache_dir().map_err(explain)?
                )?;
            }
        },
        CtlCmd::Frame { file: output } => {
            let jpeg = proxy.get_frame_jpeg().map_err(explain)?;
            if jpeg.is_empty() {
                bail!("le démon n'a encore diffusé aucune image");
            }
            std::fs::write(&output, &jpeg)
                .with_context(|| format!("écriture de {}", output.display()))?;
            writeln!(out, "{} ({} octets)", output.display(), jpeg.len())?;
        }
        CtlCmd::Save => proxy.save_config().map_err(explain)?,
        CtlCmd::Reload => proxy.reload_config().map_err(explain)?,
    }
    Ok(())
}

/// Proxy sans cache de propriétés : chaque lecture interroge le démon, même
/// s'il a démarré après le client.
pub fn proxy(connection: &zbus::blocking::Connection) -> Result<LightningProxyBlocking<'static>> {
    LightningProxyBlocking::builder(connection)
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .map_err(explain)
}

/// Point d'entrée de `lightning ctl` : se connecte au bus de session.
pub fn main(cmd: CtlCmd) -> Result<()> {
    let connection =
        zbus::blocking::Connection::session().context("connexion au bus de session")?;
    match run(&proxy(&connection)?, cmd, &mut std::io::stdout().lock()) {
        // Sortie redirigée vers un `head` déjà fermé : ce n'est pas une erreur.
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::dbus::tests::PrivateBus;
    use crate::daemon::dbus::{notifier, serve};
    use crate::daemon::tests::{start, wait_for, write_png};
    use std::sync::atomic::Ordering;

    #[test]
    fn ctl_drives_a_daemon_on_a_private_bus() {
        let Some(bus) = PrivateBus::start() else {
            eprintln!("dbus-daemon introuvable : test ctl sauté");
            return;
        };
        let client = bus.connect();
        let proxy = proxy(&client).unwrap();

        // Sans démon : message qui dit quoi faire.
        let mut out = Vec::new();
        let err = run(&proxy, CtlCmd::Status, &mut out)
            .unwrap_err()
            .to_string();
        assert!(err.contains("le démon ne tourne pas"), "{err}");

        let run_ = start("ctl", |_| {}, None);
        let service = serve(run_.handle.clone(), Some(&bus.address)).unwrap();
        let mut notify = notifier(service);
        let (handle, stop) = (run_.handle.clone(), run_.stop.clone());
        let pump = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                notify(&handle.state());
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        let h = &run_.handle;
        let ctl = |cmd: CtlCmd| -> Result<String> {
            let mut out = Vec::new();
            run(&proxy, cmd, &mut out)?;
            Ok(String::from_utf8(out).unwrap())
        };

        ctl(CtlCmd::Color {
            rgb: "ff8800".into(),
        })
        .unwrap();
        assert_eq!(h.state().mode, "color");
        ctl(CtlCmd::Fps { fps: 5.0 }).unwrap();
        ctl(CtlCmd::Brightness { level: 40 }).unwrap();
        ctl(CtlCmd::Widgets {
            widgets: vec!["cpu".into(), "memory".into()],
        })
        .unwrap();
        ctl(CtlCmd::OnExit {
            behaviour: OnExit::Restore,
        })
        .unwrap();
        let st = h.state();
        assert_eq!(
            (st.fps, st.brightness, st.on_exit.as_str()),
            (5.0, 40, "restore")
        );
        assert_eq!(st.widgets, ["cpu", "memory"]);

        // Chemin relatif rendu absolu avant d'être envoyé au démon.
        let img = run_.dir.join("photo.png");
        write_png(&img, 32, 20, [10, 200, 10], [10, 200, 10]);
        ctl(CtlCmd::Image {
            file: img.clone(),
            fit: Fit::Contain,
        })
        .unwrap();
        assert_eq!(h.state().image_path, img.to_str().unwrap());
        assert!(Path::new(&absolute(Path::new("rel/x.png")).unwrap()).is_absolute());

        wait_for("statut à jour", || proxy.mode().unwrap() == "image");
        let status = ctl(CtlCmd::Status).unwrap();
        assert!(status.contains("contenu        : image"), "{status}");
        assert!(
            status.contains("demandée 40 %, firmware inconnue"),
            "{status}"
        );
        assert!(status.contains("panneaux       : cpu, memory"), "{status}");
        assert!(status.contains("écran          : disabled"), "{status}");

        let frame = run_.dir.join("frame.jpg");
        let text = ctl(CtlCmd::Frame {
            file: frame.clone(),
        })
        .unwrap();
        assert!(text.contains("frame.jpg"));
        assert_eq!(&std::fs::read(&frame).unwrap()[..2], &[0xFF, 0xD8]);

        let err = ctl(CtlCmd::Rotation { degrees: 90 })
            .unwrap_err()
            .to_string();
        assert!(err.contains("rotation"), "{err}");
        let err = ctl(CtlCmd::Video {
            file: "/nonexistent.mp4".into(),
            once: true,
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("/nonexistent.mp4"), "{err}");

        ctl(CtlCmd::Mode {
            mode: Mode::Dashboard,
        })
        .unwrap();
        // Diaporama : options partielles conservées.
        ctl(CtlCmd::Slideshow {
            folders: vec![run_.dir.clone()],
            interval: Some(3.0),
            order: Some("chronological".into()),
            fit: None,
            transition_ms: Some(0),
            no_overlay: true,
            no_clock: false,
        })
        .unwrap();
        let st = h.state();
        assert_eq!(st.mode, "slideshow");
        assert_eq!(st.slideshow_folders, [run_.dir.to_str().unwrap()]);
        assert_eq!(
            (st.slideshow_interval, st.slideshow_order.as_str()),
            (3.0, "chronological")
        );
        assert_eq!(
            st.slideshow_fit, "contain",
            "option absente : valeur conservée"
        );
        assert!(!st.slideshow_overlay && st.slideshow_clock);
        ctl(CtlCmd::Next).unwrap();
        let google = ctl(CtlCmd::Google {
            action: "status".into(),
        })
        .unwrap();
        assert!(google.contains("état    : unconfigured"), "{google}");
        let err = ctl(CtlCmd::Google {
            action: "start".into(),
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("client_id"), "{err}");
        ctl(CtlCmd::Mode {
            mode: Mode::Dashboard,
        })
        .unwrap();
        ctl(CtlCmd::Save).unwrap();
        ctl(CtlCmd::Reload).unwrap();
        assert_eq!(h.state().mode, "dashboard");

        run_.stop.store(true, Ordering::SeqCst);
        pump.join().unwrap();
    }
}
