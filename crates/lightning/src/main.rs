//! `lightning` : application Linux pour l'écran LCD de la MSI RTX 5090 Lightning Z.

mod config;
mod ctl;
mod daemon;
mod google;
mod heif;
mod media;
mod mjpeg;
mod mpris;
mod outputs;
mod secrets;
mod slideshow;
mod text;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use lightning_dashboard::{Collector, DashboardConfig, Renderer};
use lightning_i18n::{tr, trf};
use lightning_ipc::{Fit, OnExit};
use lightning_lcd::HidControl;
use lightning_lcd::display::encode_jpeg;
use tracing::{info, warn};

use crate::config::{Config, OutputKind};
use crate::media::{load_image, parse_color};
use crate::mjpeg::JpegSplitter;
use crate::outputs::http::HttpSink;
use crate::outputs::lcd::{LcdOptions, LcdSink};
use crate::outputs::{HEIGHT, Outputs, SharedStats, Sink, WIDTH};

#[derive(Parser)]
#[command(version, about = "Pilote l'écran LCD de la MSI RTX 5090 Lightning Z")]
struct Cli {
    /// Fichier de configuration (défaut : ~/.config/lightning/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Journalisation détaillée (-v : debug, -vv : trace)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Ne pas basculer l'écran en mode étendu via HID
    #[arg(long, global = true)]
    no_extend: bool,
    /// Sorties d'affichage, combinables : lcd,window,http (défaut : lcd)
    #[arg(short, long, global = true, value_enum, value_delimiter = ',')]
    output: Option<Vec<OutputKind>>,
    /// Données factices (machine « workstation ») : captures de la documentation
    #[arg(long, global = true, hide = true)]
    demo: bool,
    /// Ne pas utiliser l'écran réel (développement sans matériel)
    #[arg(long, global = true)]
    no_lcd: bool,
    /// Adresse d'écoute de la sortie http (défaut : 127.0.0.1:7881)
    #[arg(long, global = true, value_name = "ADRESSE:PORT")]
    http_addr: Option<String>,
    /// Masquer le bandeau de debug de la fenêtre d'aperçu
    #[arg(long, global = true)]
    no_overlay: bool,
    /// Rotation simulée dans la fenêtre d'aperçu (0, 90, 180, 270)
    #[arg(long, global = true, value_name = "DEGRÉS")]
    preview_rotation: Option<u16>,
    /// État de l'écran à l'arrêt : keep (dernière image), blank (écran noir),
    /// restore (animation MSI d'origine)
    #[arg(long, global = true, value_name = "COMPORTEMENT")]
    on_exit: Option<OnExit>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Affiche l'état renvoyé par le firmware (HID)
    Status,
    /// Remplit l'écran d'une couleur unie (RRGGBB)
    Color { rgb: String },
    /// Affiche une image (JPEG, PNG, WebP, BMP, GIF)
    Image {
        file: PathBuf,
        /// cover, contain ou stretch
        #[arg(long, default_value_t = Fit::Cover)]
        fit: Fit,
    },
    /// Lit une vidéo (fichier, URL ou périphérique lisible par ffmpeg)
    Video {
        file: String,
        #[arg(long, default_value_t = 30.0)]
        fps: f32,
        /// Reboucler indéfiniment
        #[arg(long = "loop")]
        looped: bool,
    },
    /// Affiche un flux MJPEG 1280x800 lu sur l'entrée standard
    Stream {
        /// Cadence maximale (défaut : 60 ; 0 = illimitée)
        #[arg(long, default_value_t = 60.0)]
        fps: f32,
    },
    /// Affiche en continu le tableau de bord de l'état du PC
    Dashboard {
        /// Images par seconde (défaut : configuration, sinon 2)
        #[arg(long)]
        fps: Option<f32>,
        /// Écrit une image PNG du tableau de bord et quitte, sans toucher à l'écran
        #[arg(long, value_name = "FICHIER")]
        png: Option<PathBuf>,
    },
    /// Règle la luminosité de l'écran (0 à 100)
    Brightness { level: u8 },
    /// Rend l'écran au firmware (animation MSI d'origine)
    Restore,
    /// Démon pilotable par D-Bus (service systemd, application de réglage)
    Daemon,
    /// Pilote le démon en cours d'exécution (client D-Bus)
    Ctl {
        #[command(subcommand)]
        command: ctl::CtlCmd,
    },
}

fn main() -> std::process::ExitCode {
    lightning_i18n::init();
    let cli = Cli::parse();
    // Le détail ne concerne que nos crates : winit et consorts restent discrets.
    let default_level = match cli.verbose {
        0 => "info",
        1 => "info,lightning=debug,lightning_lcd=debug,lightning_dashboard=debug",
        _ => "info,lightning=trace,lightning_lcd=trace,lightning_dashboard=trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();

    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "{}",
                trf("erreur : {error}", &[("error", &format!("{e:#}"))])
            );
            std::process::ExitCode::FAILURE
        }
    }
}

/// Applique les options de ligne de commande par-dessus la configuration.
fn merge_cli(config: &mut Config, cli: &Cli) -> Result<()> {
    let out = &mut config.output;
    if let Some(outputs) = &cli.output {
        out.outputs = outputs.clone();
    }
    if cli.no_lcd {
        out.outputs.retain(|o| *o != OutputKind::Lcd);
    }
    let mut unique = Vec::new();
    for o in &out.outputs {
        if !unique.contains(o) {
            unique.push(*o);
        }
    }
    out.outputs = unique;
    if let Some(addr) = &cli.http_addr {
        out.http_addr = addr.clone();
    }
    if cli.no_overlay {
        out.overlay = false;
    }
    if let Some(rotation) = cli.preview_rotation {
        anyhow::ensure!(
            matches!(rotation, 0 | 90 | 180 | 270),
            tr("--preview-rotation : 0, 90, 180 ou 270")
        );
        out.preview_rotation = rotation;
    }
    if let Some(on_exit) = cli.on_exit {
        out.on_exit = on_exit;
    }
    Ok(())
}

/// Construit les sorties demandées. L'écran réel absent n'est pas une erreur
/// ici : son état est publié et les autres sorties continuent.
fn build_outputs(
    config: &Config,
    extend: bool,
    command: &str,
    stop: &Arc<AtomicBool>,
) -> Result<Outputs> {
    let out = &config.output;
    if out.outputs.is_empty() {
        bail!(tr(
            "aucune sortie active : ajouter --output window ou --output http avec --no-lcd"
        ));
    }
    let quality = config.dashboard.quality;
    let stats = SharedStats::default();
    let mut sinks: Vec<Box<dyn Sink>> = Vec::new();
    for kind in &out.outputs {
        match kind {
            OutputKind::Lcd => {
                let blank_jpeg = match out.on_exit {
                    OnExit::Blank => {
                        let when = chrono::Local::now().format("%d/%m/%Y %H:%M").to_string();
                        let rgba = text::stopped_frame(WIDTH, HEIGHT, &format!("depuis le {when}"));
                        Some(encode_jpeg(&rgba, WIDTH, HEIGHT, 4, quality)?)
                    }
                    _ => None,
                };
                let opts = LcdOptions {
                    extend,
                    quality,
                    on_exit: out.on_exit,
                    blank_jpeg,
                };
                sinks.push(Box::new(LcdSink::spawn(opts, stats.clone(), stop.clone())));
            }
            OutputKind::Http => {
                sinks.push(Box::new(HttpSink::spawn(
                    &out.http_addr,
                    stats.clone(),
                    stop.clone(),
                )?));
            }
            OutputKind::Window => sinks.push(window_sink(config, &stats, stop)?),
        }
    }
    Ok(Outputs::new(sinks, stats, quality, command))
}

#[cfg(feature = "window")]
pub(crate) fn window_sink(
    config: &Config,
    stats: &SharedStats,
    stop: &Arc<AtomicBool>,
) -> Result<Box<dyn Sink>> {
    use crate::outputs::window::{WindowOptions, WindowSink};
    let opts = WindowOptions {
        overlay: config.output.overlay,
        rotation: config.output.preview_rotation,
    };
    Ok(Box::new(WindowSink::spawn(
        opts,
        stats.clone(),
        stop.clone(),
    )?))
}

#[cfg(not(feature = "window"))]
pub(crate) fn window_sink(
    _: &Config,
    _: &SharedStats,
    _: &Arc<AtomicBool>,
) -> Result<Box<dyn Sink>> {
    bail!(tr(
        "ce binaire a été compilé sans la fenêtre d'aperçu (feature `window`)"
    ))
}

fn run(cli: Cli) -> Result<()> {
    // Le client D-Bus n'a besoin ni de la configuration ni des signaux.
    if let Cmd::Ctl { command } = cli.command {
        return ctl::main(command);
    }
    // Le démon crée lui-même son fichier de configuration au premier réglage.
    let missing_daemon_config =
        matches!(cli.command, Cmd::Daemon) && cli.config.as_deref().is_some_and(|p| !p.exists());
    let mut config = if missing_daemon_config {
        Config::default()
    } else {
        config::load(cli.config.as_deref())?
    };
    merge_cli(&mut config, &cli)?;
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))
            .context(tr("installation du gestionnaire de signaux"))?;
    }
    let extend = !cli.no_extend;

    match cli.command {
        Cmd::Status => status(),
        Cmd::Brightness { level } => brightness(level),
        Cmd::Restore => restore(),
        Cmd::Ctl { .. } => unreachable!("traité plus haut"),
        Cmd::Daemon => {
            let config_path = cli.config.clone().or_else(config::default_path);
            run_daemon(config, config_path, extend, cli.demo, &stop)
        }
        Cmd::Color { rgb } => {
            let [r, g, b] = parse_color(&rgb)?;
            let pixels = [r, g, b, 255].repeat((WIDTH * HEIGHT) as usize);
            show_still(
                build_outputs(&config, extend, "color", &stop)?,
                &pixels,
                &stop,
            )
        }
        Cmd::Image { file, fit } => {
            let pixels = load_image(&file, fit, WIDTH, HEIGHT)?;
            show_still(
                build_outputs(&config, extend, "image", &stop)?,
                &pixels,
                &stop,
            )
        }
        Cmd::Video { file, fps, looped } => {
            let out = build_outputs(&config, extend, "video", &stop)?;
            video(out, &file, fps, looped, &stop)
        }
        Cmd::Stream { fps } => {
            let mut out = build_outputs(&config, extend, "stream", &stop)?;
            let frames = pump(&mut out, std::io::stdin().lock(), fps, &stop)?;
            info!(frames, "flux terminé");
            finish_stream(out)
        }
        Cmd::Dashboard { fps, png } => {
            if let Some(fps) = fps {
                config.dashboard.fps = fps;
            }
            match png {
                Some(path) => dashboard_png(&config.dashboard, cli.demo, &path),
                None => {
                    let out = build_outputs(&config, extend, "dashboard", &stop)?;
                    dashboard(config.dashboard, cli.demo, out, &stop)
                }
            }
        }
    }
}

/// Affiche une image fixe. Avec une sortie d'aperçu, le programme reste ouvert
/// jusqu'à Ctrl-C (ou la fermeture de la fenêtre) pour qu'on puisse la regarder.
fn show_still(mut out: Outputs, rgba: &[u8], stop: &AtomicBool) -> Result<()> {
    out.present_rgba(rgba, Duration::ZERO, true)?;
    if out.has_preview() {
        info!(sorties = ?out.names(), "image affichée (Ctrl-C pour quitter)");
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    } else {
        out.finish()
    }
}

/// Fin d'un flux : sans aperçu, vérifie que l'écran a bien reçu la dernière image.
fn finish_stream(mut out: Outputs) -> Result<()> {
    if out.has_preview() {
        Ok(())
    } else {
        out.finish()
    }
}

fn restore() -> Result<()> {
    let mut hid = HidControl::open()?;
    match hid.restore()? {
        Some(st) if !st.is_extended() => {
            println!("{}", tr("écran rendu au firmware (mode autonome)"));
            Ok(())
        }
        Some(_) => bail!(tr("le firmware est resté en mode écran étendu")),
        None => bail!(tr("pas de réponse du firmware sur le canal HID")),
    }
}

fn status() -> Result<()> {
    let mut hid = HidControl::open()?;
    match hid.status()? {
        Some(st) => {
            let line = |label: &str, value: String| println!("{label:<14}: {value}");
            line(tr("modèle"), st.model.clone());
            line(tr("numéro série"), st.sn.clone());
            line(tr("firmware"), st.version.firmware.clone());
            line(
                tr("versions"),
                trf(
                    "app {app}, sdk {sdk}, matériel {hardware}",
                    &[
                        ("app", &st.version.app),
                        ("sdk", &st.version.sdk),
                        ("hardware", &st.version.hardware),
                    ],
                ),
            );
            line(tr("luminosité"), format!("{} %", st.brightness));
            line(tr("rotation"), format!("{}°", st.degree));
            let mode = if st.is_extended() {
                tr("écran étendu (affiche le flux USB)")
            } else {
                tr("autonome (animation interne)")
            };
            line(tr("mode"), mode.to_owned());
            Ok(())
        }
        None => bail!(tr("pas de réponse du firmware sur le canal HID")),
    }
}

fn brightness(level: u8) -> Result<()> {
    let mut hid = HidControl::open()?;
    match hid.set_brightness(level)? {
        Some(st) => {
            println!("{} : {} %", tr("luminosité"), st.brightness);
            if st.brightness != u32::from(level) {
                warn!(
                    "le firmware annonce {} % au lieu de {level} %",
                    st.brightness
                );
            }
            Ok(())
        }
        None => bail!(tr("pas de réponse du firmware sur le canal HID")),
    }
}

/// Diffuse les images d'un flux MJPEG à la cadence demandée (0 = sans limite).
fn pump(out: &mut Outputs, reader: impl Read, fps: f32, stop: &AtomicBool) -> Result<u64> {
    let start = Instant::now();
    let mut frames = 0u64;
    for jpeg in JpegSplitter::new(reader) {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        out.present_jpeg(jpeg?, false)?;
        frames += 1;
        if fps > 0.0 {
            let due = start + Duration::from_secs_f64(frames as f64 / f64::from(fps));
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    }
    Ok(frames)
}

fn video(mut out: Outputs, file: &str, fps: f32, looped: bool, stop: &AtomicBool) -> Result<()> {
    let mut child = media::spawn_ffmpeg(file, fps, looped, false, WIDTH, HEIGHT)?;
    let stdout = child
        .stdout
        .take()
        .context(tr("sortie de ffmpeg indisponible"))?;
    // ffmpeg cadence déjà le flux (-re) : pas de limitation supplémentaire.
    let result = pump(&mut out, stdout, 0.0, stop);
    let _ = child.kill();
    let status = child.wait()?;
    let frames = result?;
    info!(frames, "vidéo terminée");
    if frames == 0 && !status.success() {
        bail!(trf(
            "ffmpeg n'a produit aucune image ({status})",
            &[("status", &status)]
        ));
    }
    finish_stream(out)
}

/// Démon : moteur d'affichage + interface D-Bus sur le bus de session.
fn run_daemon(
    config: Config,
    config_path: Option<std::path::PathBuf>,
    extend: bool,
    demo: bool,
    stop: &Arc<AtomicBool>,
) -> Result<()> {
    let (mut engine, handle) = daemon::Engine::new(
        daemon::EngineOptions {
            google: daemon::GoogleRuntime {
                // Jamais le trousseau réel en démonstration.
                keyring: !demo,
                ..daemon::GoogleRuntime::standard(config_path.as_deref())
            },
            demo,
            config,
            config_path,
            extend,
        },
        stop.clone(),
    )?;
    // Sans bus de session, le démon reste un simple afficheur.
    let _connection = match daemon::dbus::serve(handle, None) {
        Ok(connection) => {
            info!(nom = lightning_ipc::BUS_NAME, "interface D-Bus publiée");
            engine.set_notifier(daemon::dbus::notifier(connection.clone()));
            Some(connection)
        }
        Err(zbus::Error::NameTaken) => {
            bail!(trf(
                "un autre démon lightning tient déjà le nom D-Bus {name}",
                &[("name", &lightning_ipc::BUS_NAME)]
            ))
        }
        Err(e) => {
            warn!("interface D-Bus indisponible ({e}) : démon non pilotable");
            None
        }
    };
    engine.run();
    Ok(())
}

/// Attend `duration` par tranches courtes pour réagir vite à Ctrl-C.
fn sleep_interruptible(duration: Duration, stop: &AtomicBool) {
    let end = Instant::now() + duration;
    while !stop.load(Ordering::SeqCst) {
        let Some(left) = end.checked_duration_since(Instant::now()) else {
            break;
        };
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
}

fn collector(cfg: &DashboardConfig, demo: bool) -> Collector {
    if demo {
        Collector::new_demo(cfg.clone())
    } else {
        Collector::new(cfg.clone())
    }
}

fn dashboard_png(cfg: &DashboardConfig, demo: bool, path: &Path) -> Result<()> {
    let mut collector = collector(cfg, demo);
    let mut renderer = Renderer::new();
    // Quelques relevés espacés pour disposer de taux et d'un début d'historique.
    for _ in 0..8 {
        collector.sample();
        std::thread::sleep(Duration::from_millis(250));
    }
    let snapshot = collector.sample();
    let rgba = renderer.render(&snapshot, collector.history(), cfg);
    image::save_buffer(
        path,
        rgba,
        lightning_dashboard::render::WIDTH,
        lightning_dashboard::render::HEIGHT,
        image::ExtendedColorType::Rgba8,
    )
    .with_context(|| trf("écriture de {path}", &[("path", &path.display())]))?;
    println!(
        "{}",
        trf("capture écrite : {path}", &[("path", &path.display())])
    );
    Ok(())
}

fn dashboard(cfg: DashboardConfig, demo: bool, mut out: Outputs, stop: &AtomicBool) -> Result<()> {
    if let Some(level) = cfg.brightness
        && out.names().contains(&"lcd")
    {
        match HidControl::open().and_then(|mut hid| hid.set_brightness(level)) {
            Ok(_) => info!(level, "luminosité appliquée"),
            Err(e) => warn!("luminosité non appliquée : {e}"),
        }
    }
    let mut collector = collector(&cfg, demo);
    let mut renderer = Renderer::new();
    let interval = cfg.frame_interval();
    info!(
        sorties = ?out.names(),
        fps = 1.0 / interval.as_secs_f32(),
        "tableau de bord démarré (Ctrl-C pour arrêter)"
    );

    let (mut frames, mut work) = (0u64, Duration::ZERO);
    let mut report = Instant::now();
    let mut next = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        let t0 = Instant::now();
        let snapshot = collector.sample();
        let rgba = renderer.render(&snapshot, collector.history(), &cfg);
        let render = t0.elapsed();
        out.present_rgba(rgba, render, false)?;
        work += t0.elapsed();
        frames += 1;
        // Sans aperçu, un écran inutilisable au démarrage est une erreur franche
        // (absent, déjà piloté par un autre processus…). Ensuite, on réessaie.
        if frames == 1 && !out.has_preview() {
            out.finish()?;
        }
        if report.elapsed() >= Duration::from_secs(60) {
            let lcd = out.stats().lcd;
            info!(
                frames,
                work_ms = work.as_secs_f64() * 1000.0 / frames as f64,
                lcd = lcd.state.label(),
                lcd_sent = lcd.frames_sent,
                lcd_errors = lcd.errors,
                lcd_reconnects = lcd.reconnects,
                "tableau de bord actif"
            );
            report = Instant::now();
        }
        next += interval;
        let now = Instant::now();
        if next < now {
            next = now; // en retard : on ne rattrape pas les images manquées
        }
        sleep_interruptible(next - now, stop);
    }
    let lcd = out.stats().lcd;
    info!(
        frames,
        lcd_sent = lcd.frames_sent,
        lcd_errors = lcd.errors,
        lcd_reconnects = lcd.reconnects,
        "tableau de bord arrêté"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subcommand_parses_with_global_options() {
        // Détecte les collisions d'identifiants entre options globales et arguments.
        for args in [
            "lightning --output lcd,http --on-exit blank dashboard --fps 1",
            "lightning --no-lcd --output window image a.png --fit contain",
            "lightning ctl status",
            "lightning ctl mode msi_animation",
            "lightning ctl image a.png --fit stretch",
            "lightning ctl video a.mp4 --once",
            "lightning ctl widgets cpu,gpu",
            "lightning ctl on-exit restore",
            "lightning ctl http on --addr 127.0.0.1:9000",
            "lightning ctl frame out.jpg",
            "lightning ctl slideshow /a /b --interval 20 --order chronological --fit cover --no-clock",
            "lightning ctl slideshow",
            "lightning ctl next",
            "lightning ctl google status",
            "lightning ctl brightness 50",
            "lightning daemon",
            "lightning --demo --no-lcd daemon",
            "lightning --demo dashboard --png out.png",
        ] {
            let cli =
                Cli::try_parse_from(args.split(' ')).unwrap_or_else(|e| panic!("{args} : {e}"));
            if let Cmd::Ctl { command } = cli.command {
                let _ = format!("{command:?}");
            }
        }
        assert!(Cli::try_parse_from("lightning ctl mode tv".split(' ')).is_err());
        assert!(Cli::try_parse_from("lightning ctl http maybe".split(' ')).is_err());
    }

    #[test]
    fn cli_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
