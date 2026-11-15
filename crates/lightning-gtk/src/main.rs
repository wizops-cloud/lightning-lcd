//! `lightning-settings` : application GNOME « Lightning » de réglage de l'écran
//! LCD de la carte graphique.
//!
//! L'application ne touche jamais au matériel : elle pilote le démon
//! `lightning daemon` par D-Bus (`cloud.wizops.Lightning1`) et le service
//! systemd utilisateur par l'API D-Bus de systemd.

mod bind;
mod remote;
mod style;
mod ui;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::bind::{apply_daemon, apply_frame, apply_service, decode_jpeg};
use crate::remote::{Remote, ServiceState};
use crate::ui::{APP_ID, PAGES, Ui};

/// Options de lancement (les options de capture sont réservées aux tests).
#[derive(Debug, Clone, Default, PartialEq)]
struct Options {
    /// Rend chaque page en PNG dans ce dossier, puis quitte.
    screenshot: Option<PathBuf>,
    /// Suffixe des fichiers de capture.
    tag: String,
    /// `dark` ou `light` : force le thème au lieu de suivre le système.
    color_scheme: Option<String>,
    /// Taille initiale de la fenêtre.
    size: Option<(i32, i32)>,
    /// Identifiant d'application de rechange (essais à côté d'une instance ouverte).
    app_id: Option<String>,
}

fn parse_size(text: &str) -> Option<(i32, i32)> {
    let (w, h) = text.split_once('x')?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    (w >= 200 && h >= 200).then_some((w, h))
}

fn parse_options(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} : valeur manquante"));
        match arg.as_str() {
            "--screenshot" => options.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--tag" => options.tag = value("--tag")?,
            "--color-scheme" => {
                let scheme = value("--color-scheme")?;
                if !matches!(scheme.as_str(), "dark" | "light") {
                    return Err("--color-scheme : dark ou light".into());
                }
                options.color_scheme = Some(scheme);
            }
            "--size" => {
                options.size =
                    Some(parse_size(&value("--size")?).ok_or("--size : LARGEURxHAUTEUR attendu")?);
            }
            "--app-id" => options.app_id = Some(value("--app-id")?),
            other => return Err(format!("option inconnue : {other}")),
        }
    }
    Ok(options)
}

fn show_root(ui: &Ui, name: &str) {
    if ui.root.visible_child_name().as_deref() != Some(name) {
        ui.root.set_visible_child_name(name);
    }
}

fn about_dialog() -> adw::AboutDialog {
    adw::AboutDialog::builder()
        .application_name("Lightning")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("WizOps")
        .comments(
            "Réglage de l'écran LCD intégré à la carte graphique : tableau de bord de l'état \
             du PC, image, vidéo, diaporama.\n\nProjet indépendant, sans lien avec le \
             fabricant de la carte.",
        )
        .website("https://github.com/wizops-cloud/lightning-lcd")
        .issue_url("https://github.com/wizops-cloud/lightning-lcd/issues")
        .license_type(gtk::License::Gpl30Only)
        .copyright("© 2026 WizOps")
        .build()
}

/// Rend la fenêtre dans un PNG (rendu hors écran du contenu courant).
fn save_png(window: &adw::ApplicationWindow, path: &Path) -> Result<(), String> {
    let (w, h) = (window.width(), window.height());
    if w <= 0 || h <= 0 {
        return Err("fenêtre sans taille".into());
    }
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let node = snapshot.to_node().ok_or("rien à dessiner")?;
    let renderer = window.renderer().ok_or("fenêtre sans moteur de rendu")?;
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path).map_err(|e| e.to_string())
}

/// Capture chaque page puis quitte (option `--screenshot DOSSIER`).
async fn screenshots(ui: Rc<Ui>, dir: PathBuf, tag: String, daemon_running: bool) {
    let pause = |ms| glib::timeout_future(Duration::from_millis(ms));
    let shoot = |name: &str| {
        let file = if tag.is_empty() {
            format!("{name}.png")
        } else {
            format!("{name}-{tag}.png")
        };
        let path = dir.join(file);
        match save_png(&ui.window, &path) {
            Ok(()) => println!("capture : {}", path.display()),
            Err(e) => eprintln!("capture {name} impossible : {e}"),
        }
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("dossier {} : {e}", dir.display());
    }
    // Le temps de charger l'aperçu et les vignettes.
    pause(2500).await;
    if daemon_running {
        if ui.split.is_collapsed() {
            ui.split.set_show_content(false);
            pause(700).await;
            shoot("menu");
        }
        for (name, ..) in PAGES {
            ui.show_page(name);
            if ui.split.is_collapsed() {
                ui.split.set_show_content(true);
            }
            pause(1100).await;
            shoot(name);
        }
        ui.show_page("accueil");
        let about = about_dialog();
        about.present(Some(&ui.window));
        pause(900).await;
        shoot("a-propos");
        about.close();
        pause(500).await;
        // Écran « service arrêté », tel qu'il apparaît sans démon.
        ui.offline.set_title("Le service Lightning est arrêté");
        ui.offline.set_description(Some(
            "L'écran de la carte graphique n'est pas piloté pour l'instant.",
        ));
        ui.offline_button.set_visible(true);
        ui.force_offline.set(true);
        show_root(&ui, "offline");
        pause(900).await;
        shoot("service-arrete");
    } else {
        pause(600).await;
        shoot("service-arrete");
    }
    ui.window.close();
}

/// Boucle de rafraîchissement : état du démon et du service, aperçu en direct.
///
/// L'aperçu est décodé hors du fil de l'interface, deux fois par seconde au
/// plus, et n'est pas demandé quand la fenêtre est masquée ou réduite.
async fn refresh_loop(ui: Rc<Ui>, remote: Remote, options: Options) {
    let mut screenshot_dir = options.screenshot;
    let mut service = ServiceState::default();
    let mut live: Option<gdk::Texture> = None;
    let mut tick = 0u32;
    loop {
        if !ui.window.is_visible() {
            break;
        }
        let watching = !ui.window.is_suspended();
        if tick.is_multiple_of(4) {
            service = remote.service_state().await;
        }
        tick = tick.wrapping_add(1);
        let daemon = remote.daemon_state().await;
        let running = daemon.is_ok();
        match daemon {
            Ok(state) => {
                if watching
                    && let Ok(jpeg) = remote.daemon.get_frame_jpeg().await
                    && !jpeg.is_empty()
                    && *ui.last_frame.borrow() != jpeg
                {
                    let data = jpeg.clone();
                    if let Ok(Some(decoded)) = gio::spawn_blocking(move || decode_jpeg(&data)).await
                    {
                        *ui.last_frame.borrow_mut() = jpeg;
                        let texture = decoded.into_texture();
                        // L'état est appliqué d'abord : la vignette va à la bonne carte.
                        *ui.state.borrow_mut() = state.clone();
                        apply_frame(&ui, &texture);
                        live = Some(texture);
                    }
                }
                apply_daemon(&ui, &remote, &state, live.as_ref());
                if !ui.force_offline.get() {
                    show_root(&ui, "main");
                }
            }
            Err(_) => {
                live = None;
                show_root(&ui, "offline");
            }
        }
        apply_service(&ui, &service, running);
        if let Some(dir) = screenshot_dir.take() {
            glib::spawn_future_local(screenshots(ui.clone(), dir, options.tag.clone(), running));
        }
        // Fenêtre masquée : on ralentit, l'état suffit.
        glib::timeout_future(Duration::from_millis(if watching { 500 } else { 2000 })).await;
    }
}

fn activate(app: &adw::Application, options: &Options) {
    // Une seule fenêtre : un second lancement la ramène au premier plan.
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    style::install();
    let ui = ui::build(app);
    if let Some((w, h)) = options.size {
        ui.window.set_default_size(w, h);
    }
    ui.window.present();
    let options = options.clone();
    glib::spawn_future_local(async move {
        match Remote::connect().await {
            Ok(remote) => {
                bind::connect(&ui, &remote);
                ui.show_page("accueil");
                refresh_loop(ui, remote, options).await;
            }
            Err(e) => {
                ui.offline.set_title("Bus de session inaccessible");
                ui.offline.set_description(Some(&e.to_string()));
                ui.offline_button.set_visible(false);
                show_root(&ui, "offline");
            }
        }
    });
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("lightning-settings {}", env!("CARGO_PKG_VERSION"));
        return glib::ExitCode::SUCCESS;
    }
    let options = match parse_options(args) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("{e}");
            return glib::ExitCode::FAILURE;
        }
    };
    let app = adw::Application::builder()
        .application_id(options.app_id.as_deref().unwrap_or(APP_ID))
        .build();
    // En mode capture, l'instance reste indépendante d'une application déjà ouverte.
    if options.screenshot.is_some() {
        app.set_flags(gio::ApplicationFlags::NON_UNIQUE);
    }
    app.connect_startup({
        let scheme = options.color_scheme.clone();
        move |app| {
            // Sombre par défaut ; un système réglé sur « clair » est respecté.
            adw::StyleManager::default().set_color_scheme(match scheme.as_deref() {
                Some("light") => adw::ColorScheme::ForceLight,
                Some("dark") => adw::ColorScheme::ForceDark,
                _ => adw::ColorScheme::PreferDark,
            });
            let about = gio::ActionEntry::builder("about")
                .activate(|app: &adw::Application, _, _| {
                    about_dialog().present(app.active_window().as_ref());
                })
                .build();
            let quit = gio::ActionEntry::builder("quit")
                .activate(|app: &adw::Application, _, _| app.quit())
                .build();
            app.add_action_entries([about, quit]);
            app.set_accels_for_action("app.quit", &["<Control>q"]);
            app.set_accels_for_action("app.about", &["F1"]);
        }
    });
    app.connect_activate(move |app| activate(app, &options));
    app.run_with_args::<&str>(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn options_parsing() {
        assert_eq!(parse_options(args("")).unwrap(), Options::default());
        let o = parse_options(args(
            "--screenshot /tmp/x --tag sombre --color-scheme dark --size 420x760 --app-id a.b.C",
        ))
        .unwrap();
        assert_eq!(o.screenshot.as_deref(), Some(Path::new("/tmp/x")));
        assert_eq!(o.tag, "sombre");
        assert_eq!(o.color_scheme.as_deref(), Some("dark"));
        assert_eq!(o.size, Some((420, 760)));
        assert_eq!(o.app_id.as_deref(), Some("a.b.C"));
        assert!(parse_options(args("--color-scheme rose")).is_err());
        assert!(parse_options(args("--size 10x10")).is_err());
        assert!(parse_options(args("--size large")).is_err());
        assert!(parse_options(args("--screenshot")).is_err());
        assert!(parse_options(args("--inconnu")).is_err());
    }

    #[test]
    fn pages_and_modes_are_complete() {
        let names: Vec<&str> = PAGES.iter().map(|(n, ..)| *n).collect();
        assert_eq!(
            names,
            [
                "accueil",
                "contenu",
                "diaporama",
                "dashboard",
                "reglages",
                "service"
            ]
        );
        let modes: Vec<lightning_ipc::Mode> = ui::MODES.iter().map(|(m, ..)| *m).collect();
        assert_eq!(modes.len(), lightning_ipc::Mode::ALL.len());
        for mode in lightning_ipc::Mode::ALL {
            assert!(modes.contains(mode), "{mode} sans carte");
        }
        assert_ne!(
            APP_ID,
            lightning_ipc::BUS_NAME,
            "l'application et le démon ont des noms de bus distincts"
        );
    }
}
