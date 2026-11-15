//! Liaison de l'interface au démon : actions de l'utilisateur vers D-Bus, état
//! du démon vers les widgets, vignettes décodées hors du fil de l'interface.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio, glib};
use lightning_ipc::Mode;

use crate::remote::{DaemonState, Remote, ServiceState, error_text};
use crate::ui::{Card, FITS, MODES, ON_EXIT, ORDERS, PAGES, PANELS, SLIDE_FITS, Ui, flow_append};

// ---------------------------------------------------------------- images

/// Image décodée dans un fil de travail, prête à devenir une texture.
pub struct Decoded {
    width: i32,
    height: i32,
    stride: usize,
    has_alpha: bool,
    bytes: glib::Bytes,
}

impl Decoded {
    fn from_pixbuf(pixbuf: &gdk_pixbuf::Pixbuf) -> Self {
        Self {
            width: pixbuf.width(),
            height: pixbuf.height(),
            stride: pixbuf.rowstride() as usize,
            has_alpha: pixbuf.has_alpha(),
            bytes: pixbuf.read_pixel_bytes(),
        }
    }

    /// Création de la texture : simple enveloppe, sans décodage.
    pub fn into_texture(self) -> gdk::Texture {
        let format = if self.has_alpha {
            gdk::MemoryFormat::R8g8b8a8
        } else {
            gdk::MemoryFormat::R8g8b8
        };
        gdk::MemoryTexture::new(self.width, self.height, format, &self.bytes, self.stride).upcast()
    }
}

/// Décode un JPEG (appelé hors du fil de l'interface).
pub fn decode_jpeg(data: &[u8]) -> Option<Decoded> {
    let loader = gdk_pixbuf::PixbufLoader::new();
    let written = loader.write(data);
    let closed = loader.close();
    written.ok()?;
    closed.ok()?;
    Some(Decoded::from_pixbuf(&loader.pixbuf()?))
}

/// Vignette d'un fichier image, orientation EXIF appliquée (hors du fil de l'interface).
pub fn decode_thumbnail(path: &str, width: i32, height: i32) -> Option<Decoded> {
    let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_scale(path, width, height, true).ok()?;
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    Some(Decoded::from_pixbuf(&pixbuf))
}

const PHOTO_EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "webp", "bmp"];

/// Première photo et nombre de photos d'un dossier (parcours borné).
pub fn folder_summary(folder: &Path) -> (Option<PathBuf>, usize, bool) {
    const LIMIT: usize = 5000;
    fn walk(dir: &Path, depth: u32, first: &mut Option<PathBuf>, count: &mut usize) {
        if depth > 6 || *count >= LIMIT {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if *count >= LIMIT {
                return;
            }
            let hidden = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if path.is_dir() {
                walk(&path, depth + 1, first, count);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| PHOTO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            {
                *count += 1;
                if first.is_none() {
                    *first = Some(path);
                }
            }
        }
    }
    let (mut first, mut count) = (None, 0);
    walk(folder, 0, &mut first, &mut count);
    (first, count, count >= LIMIT)
}

/// Charge la vignette d'un fichier (avec cache) et la remet à `then`.
fn with_thumbnail(ui: &Rc<Ui>, path: &str, then: impl FnOnce(&gdk::Texture) + 'static) {
    if let Some(texture) = ui.thumbs.borrow().get(path) {
        then(texture);
        return;
    }
    let (ui, path) = (ui.clone(), path.to_owned());
    glib::spawn_future_local(async move {
        let file = path.clone();
        let decoded = gio::spawn_blocking(move || decode_thumbnail(&file, 480, 300)).await;
        if let Ok(Some(decoded)) = decoded {
            let texture = decoded.into_texture();
            then(&texture);
            let mut cache = ui.thumbs.borrow_mut();
            if cache.len() > 64 {
                cache.clear();
            }
            cache.insert(path, texture);
        }
    });
}

// --------------------------------------------------------------- actions

/// Lance un appel au démon ; une erreur s'affiche en notification.
pub fn call<F>(ui: &Rc<Ui>, future: F)
where
    F: Future<Output = zbus::Result<()>> + 'static,
{
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        if let Err(e) = future.await {
            ui.toast(&error_text(&e));
        }
    });
}

/// Comme [`call`], avec une confirmation quand l'appel réussit.
fn call_confirmed<F>(ui: &Rc<Ui>, message: &'static str, future: F)
where
    F: Future<Output = zbus::Result<()>> + 'static,
{
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        match future.await {
            Ok(()) => ui.toast(message),
            Err(e) => ui.toast(&error_text(&e)),
        }
    });
}

/// Exécute `action` si rien d'autre n'est demandé pendant `delay` (curseurs).
fn debounced(ui: &Rc<Ui>, delay: Duration, action: impl FnOnce() + 'static) {
    let ticket = ui.debounce.get().wrapping_add(1);
    ui.debounce.set(ticket);
    let ui = ui.clone();
    glib::timeout_add_local_once(delay, move || {
        if ui.debounce.get() == ticket {
            action();
        }
    });
}

pub fn hex_color(rgba: &gdk::RGBA) -> String {
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "{:02x}{:02x}{:02x}",
        channel(rgba.red()),
        channel(rgba.green()),
        channel(rgba.blue())
    )
}

pub fn parse_hex(hex: &str) -> Option<gdk::RGBA> {
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let c = |i: usize| {
        u8::from_str_radix(&hex[i..i + 2], 16)
            .ok()
            .map(|v| f32::from(v) / 255.0)
    };
    Some(gdk::RGBA::new(c(0)?, c(2)?, c(4)?, 1.0))
}

pub fn file_name(path: &str) -> String {
    if path.is_empty() {
        return "Aucun fichier choisi".to_owned();
    }
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

pub fn mode_label(mode: &str) -> &'static str {
    MODES
        .iter()
        .find(|(m, ..)| m.as_str() == mode)
        .map_or("—", |(_, title, ..)| title)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn choose_file(
    ui: &Rc<Ui>,
    title: &str,
    mime: &str,
    filter_name: &str,
    then: impl Fn(String) + 'static,
) {
    let filter = gtk::FileFilter::new();
    filter.add_mime_type(mime);
    filter.set_name(Some(filter_name));
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title(title)
        .modal(true)
        .filters(&filters)
        .default_filter(&filter)
        .build();
    let ui_ = ui.clone();
    dialog.open(Some(&ui.window), gio::Cancellable::NONE, move |result| {
        // Une annulation n'est pas une erreur à signaler.
        let Ok(file) = result else { return };
        match file.path().and_then(|p| p.to_str().map(str::to_owned)) {
            Some(path) => then(path),
            None => ui_.toast("Ce fichier n'est pas accessible localement"),
        }
    });
}

fn pick_image(ui: &Rc<Ui>, remote: &Remote) {
    let (ui_, remote) = (ui.clone(), remote.clone());
    choose_file(ui, "Choisir une image", "image/*", "Images", move |path| {
        let fit = FITS[ui_.image_fit.selected().min(2) as usize].0;
        let remote = remote.clone();
        call(&ui_, async move {
            remote.daemon.show_image(&path, fit.as_str()).await
        });
    });
}

fn pick_video(ui: &Rc<Ui>, remote: &Remote) {
    let (ui_, remote) = (ui.clone(), remote.clone());
    choose_file(ui, "Choisir une vidéo", "video/*", "Vidéos", move |path| {
        let looped = ui_.video_loop.is_active();
        let remote = remote.clone();
        call(&ui_, async move {
            remote.daemon.play_video(&path, looped).await
        });
    });
}

/// Coche la carte du mode donné dans les deux jeux de cartes.
fn sync_cards(ui: &Ui, mode: &str) {
    let was = ui.updating.replace(true);
    for card in ui.hero_cards.iter().chain(&ui.content_cards) {
        card.button.set_active(card.mode.as_str() == mode);
    }
    ui.updating.set(was);
}

/// Active un contenu à la demande de l'utilisateur.
fn activate_mode(ui: &Rc<Ui>, remote: &Remote, mode: Mode) {
    let state = ui.state.borrow().clone();
    let remote_ = remote.clone();
    match mode {
        Mode::Image if state.image_path.is_empty() => {
            ui.show_page("contenu");
            pick_image(ui, remote);
        }
        Mode::Video if state.video_path.is_empty() => {
            ui.show_page("contenu");
            pick_video(ui, remote);
        }
        Mode::Slideshow if state.slideshow_folders.is_empty() => {
            ui.toast("Ajoutez d'abord un dossier de photos");
            ui.show_page("diaporama");
            call(ui, async move {
                remote_.daemon.set_mode(Mode::Slideshow.as_str()).await
            });
        }
        Mode::Color => {
            let hex = hex_color(&ui.color.rgba());
            call(ui, async move { remote_.daemon.show_color(&hex).await });
        }
        Mode::MsiAnimation => call(ui, async move { remote_.daemon.restore().await }),
        other => call(
            ui,
            async move { remote_.daemon.set_mode(other.as_str()).await },
        ),
    }
}

/// Envoie au démon tous les réglages du diaporama (`folders` : nouvelle liste, sinon l'actuelle).
fn send_slideshow(ui: &Rc<Ui>, remote: &Remote, folders: Option<Vec<String>>) {
    let folders = folders.unwrap_or_else(|| ui.state.borrow().slideshow_folders.clone());
    let interval = ui.slide_interval.value();
    let order = ORDERS[ui.slide_order.selected().min(1) as usize].0;
    let fit = SLIDE_FITS[ui.slide_fit.selected().min(1) as usize].0;
    // Le fondu garde sa durée si elle a été réglée à la main dans le fichier.
    let current = ui.state.borrow().slideshow_transition_ms;
    let transition = match (ui.slide_fade.is_active(), current) {
        (false, _) => 0,
        (true, 0) => 600,
        (true, ms) => ms,
    };
    let (overlay, clock) = (ui.slide_overlay.is_active(), ui.slide_clock.is_active());
    let remote = remote.clone();
    call(ui, async move {
        let refs: Vec<&str> = folders.iter().map(String::as_str).collect();
        remote
            .daemon
            .set_slideshow(&refs, interval, order, fit, transition, overlay, clock)
            .await
    });
}

fn send_widgets(ui: &Rc<Ui>, remote: &Remote, widgets: Vec<String>) {
    ui.user_action();
    // Affichage immédiat du nouvel ordre ; le démon confirmera.
    ui.state.borrow_mut().widgets = widgets.clone();
    apply_panels(ui, remote, &widgets);
    let remote = remote.clone();
    call(ui, async move {
        let refs: Vec<&str> = widgets.iter().map(String::as_str).collect();
        remote.daemon.set_widgets(&refs).await
    });
}

/// Branche les contrôles sur le démon et le service.
pub fn connect(ui: &Rc<Ui>, remote: &Remote) {
    // Navigation.
    ui.nav_list.connect_row_selected({
        let ui = ui.clone();
        move |_, row| {
            if let Some(row) = row
                && let Some((name, ..)) = PAGES.get(row.index() as usize)
            {
                ui.show_page(name);
            }
        }
    });
    ui.nav_list.connect_row_activated({
        let ui = ui.clone();
        move |_, _| {
            if ui.split.is_collapsed() {
                ui.split.set_show_content(true);
            }
        }
    });

    // Luminosité : un seul réglage partagé par l'accueil et les réglages.
    ui.brightness.connect_value_changed({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |adjustment| {
            if !ui.user_action() {
                return;
            }
            let level = adjustment.value().round().clamp(0.0, 100.0) as u8;
            ui.brightness_caption.set_label(&format!("{level} %"));
            let (ui_, remote) = (ui.clone(), remote.clone());
            debounced(&ui, Duration::from_millis(350), move || {
                call(
                    &ui_,
                    async move { remote.daemon.set_brightness(level).await },
                );
            });
        }
    });
    ui.rotation.connect_selected_notify({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |row| {
            if !ui.user_action() {
                return;
            }
            let degrees = if row.selected() == 1 { 180 } else { 0 };
            let remote = remote.clone();
            call(
                &ui,
                async move { remote.daemon.set_rotation(degrees).await },
            );
        }
    });
    ui.fps.connect_value_notify({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |row| {
            if !ui.user_action() {
                return;
            }
            let fps = row.value();
            let (ui_, remote) = (ui.clone(), remote.clone());
            debounced(&ui, Duration::from_millis(400), move || {
                call(&ui_, async move { remote.daemon.set_fps(fps).await });
            });
        }
    });

    // Cartes de contenu.
    for card in ui.hero_cards.iter().chain(&ui.content_cards) {
        let (ui_, remote, mode) = (ui.clone(), remote.clone(), card.mode);
        card.button.connect_toggled(move |button| {
            if ui_.updating.get() {
                return;
            }
            if !button.is_active() {
                // Clic sur la carte déjà active : elle le reste.
                sync_cards(&ui_, mode.as_str());
                return;
            }
            ui_.user_action();
            sync_cards(&ui_, mode.as_str());
            activate_mode(&ui_, &remote, mode);
        });
    }
    ui.image_button.connect_clicked({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |_| pick_image(&ui, &remote)
    });
    ui.video_button.connect_clicked({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |_| pick_video(&ui, &remote)
    });
    ui.slideshow_open.connect_clicked({
        let ui = ui.clone();
        move |_| ui.show_page("diaporama")
    });
    ui.color.connect_rgba_notify({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |button| {
            if !ui.user_action() {
                return;
            }
            let hex = hex_color(&button.rgba());
            let remote = remote.clone();
            call(&ui, async move { remote.daemon.show_color(&hex).await });
        }
    });
    for (hex, button) in &ui.swatches {
        let (ui_, remote, hex) = (ui.clone(), remote.clone(), hex.clone());
        button.connect_clicked(move |_| {
            ui_.user_action();
            let (remote, hex) = (remote.clone(), hex.clone());
            call(&ui_, async move { remote.daemon.show_color(&hex).await });
        });
    }
    ui.image_fit.connect_selected_notify({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |row| {
            if !ui.user_action() {
                return;
            }
            let path = ui.state.borrow().image_path.clone();
            if path.is_empty() {
                return;
            }
            let fit = FITS[row.selected().min(2) as usize].0;
            let remote = remote.clone();
            call(&ui, async move {
                remote.daemon.show_image(&path, fit.as_str()).await
            });
        }
    });
    ui.video_loop.connect_active_notify({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |row| {
            if !ui.user_action() {
                return;
            }
            let path = ui.state.borrow().video_path.clone();
            if path.is_empty() {
                return;
            }
            let looped = row.is_active();
            let remote = remote.clone();
            call(
                &ui,
                async move { remote.daemon.play_video(&path, looped).await },
            );
        }
    });

    // Diaporama : chaque changement renvoie l'ensemble des réglages.
    ui.slide_interval.connect_value_notify({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |_| {
            if ui.user_action() {
                let (ui_, remote) = (ui.clone(), remote.clone());
                debounced(&ui, Duration::from_millis(500), move || {
                    send_slideshow(&ui_, &remote, None)
                });
            }
        }
    });
    for row in [&ui.slide_order, &ui.slide_fit] {
        let (ui_, remote) = (ui.clone(), remote.clone());
        row.connect_selected_notify(move |_| {
            if ui_.user_action() {
                send_slideshow(&ui_, &remote, None);
            }
        });
    }
    for row in [&ui.slide_fade, &ui.slide_overlay, &ui.slide_clock] {
        let (ui_, remote) = (ui.clone(), remote.clone());
        row.connect_active_notify(move |_| {
            if ui_.user_action() {
                send_slideshow(&ui_, &remote, None);
            }
        });
    }
    ui.add_folder.connect_clicked({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Ajouter un dossier de photos")
                .modal(true)
                .build();
            let (ui_, remote) = (ui.clone(), remote.clone());
            dialog.select_folder(Some(&ui.window), gio::Cancellable::NONE, move |result| {
                let Ok(folder) = result else { return };
                let Some(path) = folder.path().and_then(|p| p.to_str().map(str::to_owned)) else {
                    ui_.toast("Ce dossier n'est pas accessible localement");
                    return;
                };
                let mut folders = ui_.state.borrow().slideshow_folders.clone();
                if folders.contains(&path) {
                    ui_.toast("Ce dossier fait déjà partie du diaporama");
                } else {
                    folders.push(path);
                    ui_.user_action();
                    send_slideshow(&ui_, &remote, Some(folders));
                    ui_.toast("Dossier ajouté au diaporama");
                }
            });
        }
    });

    // Google Photos.
    ui.google_button.connect_clicked({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |_| {
            let remote = remote.clone();
            if ui.state.borrow().google_busy() {
                call_confirmed(&ui, "Import annulé", async move {
                    remote.daemon.google_photos_cancel().await
                });
            } else {
                ui.google_requested.set(true);
                ui.google_opened.borrow_mut().clear();
                call(
                    &ui,
                    async move { remote.daemon.google_photos_start().await },
                );
            }
        }
    });
    ui.google_clear.connect_clicked({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |_| {
            let remote = remote.clone();
            call_confirmed(&ui, "Cache des photos Google vidé", async move {
                remote.daemon.google_photos_clear_cache().await
            });
        }
    });
    ui.google_save.connect_clicked({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |_| {
            let (id, secret) = (
                ui.google_id.text().to_string(),
                ui.google_secret.text().to_string(),
            );
            if id.trim().is_empty() || secret.trim().is_empty() {
                ui.toast("Renseignez l'identifiant client et le secret client");
                return;
            }
            let (ui_, remote) = (ui.clone(), remote.clone());
            glib::spawn_future_local(async move {
                match remote.daemon.set_google_credentials(&id, &secret).await {
                    Ok(()) => {
                        ui_.google_secret.set_text("");
                        ui_.toast("Identifiants enregistrés");
                    }
                    Err(e) => ui_.toast(&error_text(&e)),
                }
            });
        }
    });

    // Service systemd.
    ui.offline_button.connect_clicked({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |_| {
            let remote = remote.clone();
            call(&ui, async move { remote.start_service().await });
        }
    });
    ui.service_active.connect_active_notify({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |row| {
            if !ui.user_action() {
                return;
            }
            let (active, remote) = (row.is_active(), remote.clone());
            call(&ui, async move {
                if active {
                    remote.start_service().await
                } else {
                    remote.stop_service().await
                }
            });
        }
    });
    ui.service_enabled.connect_active_notify({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |row| {
            if !ui.user_action() {
                return;
            }
            let (enabled, remote) = (row.is_active(), remote.clone());
            let message = if enabled {
                "Le service se lancera à l'ouverture de session"
            } else {
                "Le service ne se lancera plus tout seul"
            };
            call_confirmed(&ui, message, async move {
                remote.set_service_enabled(enabled).await
            });
        }
    });
    ui.restart_button.connect_clicked({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |_| {
            let remote = remote.clone();
            call_confirmed(&ui, "Service redémarré", async move {
                remote.restart_service().await
            });
        }
    });

    // Arrêt et aperçu HTTP.
    ui.on_exit.connect_selected_notify({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |row| {
            if !ui.user_action() {
                return;
            }
            let behaviour = ON_EXIT[row.selected().min(2) as usize].0;
            let remote = remote.clone();
            call(&ui, async move {
                remote.daemon.set_on_exit(behaviour.as_str()).await
            });
        }
    });
    ui.http.connect_active_notify({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |row| {
            if !ui.user_action() {
                return;
            }
            let (enabled, remote) = (row.is_active(), remote.clone());
            let message = if enabled {
                "Aperçu HTTP activé"
            } else {
                "Aperçu HTTP coupé"
            };
            call_confirmed(&ui, message, async move {
                remote.daemon.set_http(enabled, "").await
            });
        }
    });
    ui.http_addr.connect_apply({
        let (ui, remote) = (ui.clone(), remote.clone());
        move |row| {
            ui.user_action();
            let (addr, enabled, remote) =
                (row.text().to_string(), ui.http.is_active(), remote.clone());
            call_confirmed(&ui, "Adresse de l'aperçu enregistrée", async move {
                remote.daemon.set_http(enabled, &addr).await
            });
        }
    });
    ui.browser_button.connect_clicked({
        let ui = ui.clone();
        move |_| {
            let addr = ui.state.borrow().http_addr.replace("0.0.0.0", "127.0.0.1");
            gtk::UriLauncher::new(&format!("http://{addr}/")).launch(
                Some(&ui.window),
                gio::Cancellable::NONE,
                |_| {},
            );
        }
    });
}

// ------------------------------------------------------- état → interface

/// Illustration d'une carte : image en direct, vignette de fichier, couleur ou icône.
fn apply_card_art(ui: &Rc<Ui>, card: &Card, st: &DaemonState, live: Option<&gdk::Texture>) {
    let active = card.mode.as_str() == st.mode;
    let show = |name: &str| {
        if card.art.visible_child_name().as_deref() != Some(name) {
            card.art.set_visible_child_name(name);
        }
    };
    match card.mode {
        Mode::Color => {
            if let Some(rgba) = parse_hex(&st.color) {
                card.color.set(rgba);
                card.swatch.queue_draw();
            }
            show("color");
        }
        Mode::MsiAnimation => show("icon"),
        Mode::Image if !active || live.is_none() => {
            if st.image_path.is_empty() {
                show("icon");
            } else if *card.thumb_key.borrow() != st.image_path {
                *card.thumb_key.borrow_mut() = st.image_path.clone();
                let (thumb, art) = (card.thumb.clone(), card.art.clone());
                with_thumbnail(ui, &st.image_path, move |texture| {
                    thumb.set_paintable(Some(texture));
                    art.set_visible_child_name("thumb");
                });
            }
        }
        Mode::Slideshow if !active || live.is_none() => {
            // Couverture : la première photo du premier dossier.
            match st.slideshow_folders.first() {
                None => {
                    card.thumb_key.borrow_mut().clear();
                    show("icon");
                }
                Some(folder) if *card.thumb_key.borrow() != *folder => {
                    *card.thumb_key.borrow_mut() = folder.clone();
                    show("icon");
                    let (ui_, folder) = (ui.clone(), PathBuf::from(folder));
                    let (thumb, art) = (card.thumb.clone(), card.art.clone());
                    glib::spawn_future_local(async move {
                        let summary = gio::spawn_blocking(move || folder_summary(&folder)).await;
                        if let Ok((Some(first), ..)) = summary
                            && let Some(path) = first.to_str()
                        {
                            with_thumbnail(&ui_, path, move |texture| {
                                thumb.set_paintable(Some(texture));
                                art.set_visible_child_name("thumb");
                            });
                        }
                    });
                }
                Some(_) => {}
            }
        }
        Mode::Dashboard if !active => match ui.dashboard_thumb.borrow().as_ref() {
            Some(texture) => {
                card.thumb.set_paintable(Some(texture));
                show("thumb");
            }
            None => show("icon"),
        },
        _ if active => {
            if let Some(texture) = live {
                card.thumb.set_paintable(Some(texture));
                card.thumb_key.borrow_mut().clear();
                show("thumb");
            }
        }
        _ => show("icon"),
    }
}

fn card_subtitle(mode: Mode, st: &DaemonState, default: &str) -> String {
    match mode {
        Mode::Image if !st.image_path.is_empty() => file_name(&st.image_path),
        Mode::Video if !st.video_path.is_empty() => file_name(&st.video_path),
        Mode::Slideshow => match st.slideshow_folders.len() {
            0 => "Aucun dossier choisi".to_owned(),
            n if st.mode == "slideshow" => {
                format!(
                    "{} · {}",
                    plural(st.slideshow_count as usize, "photo", "photos"),
                    plural(n, "dossier", "dossiers")
                )
            }
            n => plural(n, "dossier", "dossiers"),
        },
        Mode::Color => format!("#{}", st.color),
        _ => default.to_owned(),
    }
}

/// Recopie l'état du démon dans l'interface.
pub fn apply_daemon(ui: &Rc<Ui>, remote: &Remote, st: &DaemonState, live: Option<&gdk::Texture>) {
    ui.updating.set(true);
    *ui.state.borrow_mut() = st.clone();

    // Pastilles et barre latérale.
    ui.chip_lcd.label.set_label(st.lcd_label());
    ui.chip_lcd.set_tone(match st.lcd_state.as_str() {
        "connected" => "ok",
        "connecting" | "reconnecting" => "busy",
        "absent" => "bad",
        _ => "",
    });
    ui.chip_lcd
        .root
        .set_tooltip_text(Some(if st.lcd_detail.is_empty() {
            "État de la liaison avec l'écran"
        } else {
            st.lcd_detail.as_str()
        }));
    ui.chip_mode.label.set_label(mode_label(&st.mode));
    ui.chip_fps
        .label
        .set_label(&format!("{:.1} img/s", st.measured_fps).replace('.', ","));
    ui.chip_render
        .label
        .set_label(&format!("{:.1} ms", st.render_ms).replace('.', ","));
    ui.chip_render.root.set_visible(st.mode == "dashboard");
    ui.chip_brightness
        .label
        .set_label(&if st.firmware_brightness >= 0 {
            format!("{} %", st.firmware_brightness)
        } else {
            "—".to_owned()
        });
    ui.sidebar_state.set_label(st.lcd_label());
    ui.sidebar_mode.set_label(mode_label(&st.mode));

    // Cartes.
    for card in ui.hero_cards.iter().chain(&ui.content_cards) {
        let default = MODES
            .iter()
            .find(|(m, ..)| *m == card.mode)
            .map_or("", |m| m.3);
        card.subtitle
            .set_label(&card_subtitle(card.mode, st, default));
        apply_card_art(ui, card, st, live);
    }
    if ui.details.child_by_name(&st.mode).is_some() {
        ui.details.set_visible_child_name(&st.mode);
    }

    // Lecture seule.
    ui.lcd_row.set_subtitle(&glib::markup_escape_text(&format!(
        "{}{}",
        st.lcd_label(),
        if st.lcd_detail.is_empty() {
            String::new()
        } else {
            format!(" — {}", st.lcd_detail)
        }
    )));
    ui.stats_row.set_subtitle(&format!(
        "{:.1} images/s · rendu {:.1} ms · {} images envoyées",
        st.measured_fps, st.render_ms, st.frames_sent
    ));
    ui.image_row
        .set_subtitle(&glib::markup_escape_text(&file_name(&st.image_path)));
    ui.video_row
        .set_subtitle(&glib::markup_escape_text(&file_name(&st.video_path)));
    ui.version_row.set_subtitle(&st.version);
    ui.slideshow_summary
        .set_subtitle(&match st.slideshow_folders.len() {
            0 => "Aucun dossier choisi".to_owned(),
            n if st.mode == "slideshow" => format!(
                "{} dans {}",
                plural(st.slideshow_count as usize, "photo", "photos"),
                plural(n, "dossier", "dossiers")
            ),
            n => plural(n, "dossier de photos", "dossiers de photos"),
        });

    // Google Photos.
    ui.google_state
        .set_label(&format!("État : {}", st.google_label()));
    ui.google_button.set_label(if st.google_busy() {
        "Annuler l'import"
    } else {
        "Choisir des photos Google…"
    });
    ui.google_button
        .set_sensitive(st.google_state != "unconfigured");
    ui.google_cache.set_label(&match st.google_cached_count {
        0 => "Aucune photo Google sur ce PC".to_owned(),
        n => format!(
            "{} sur ce PC",
            plural(n as usize, "photo Google", "photos Google")
        ),
    });
    ui.google_clear
        .set_visible(st.google_cached_count > 0 && !st.google_busy());
    // Pages Google à ouvrir pendant un import lancé d'ici : autorisation, puis sélection.
    if ui.google_requested.get() {
        if !st.google_url.is_empty() && *ui.google_opened.borrow() != st.google_url {
            *ui.google_opened.borrow_mut() = st.google_url.clone();
            gtk::UriLauncher::new(&st.google_url).launch(
                Some(&ui.window),
                gio::Cancellable::NONE,
                |_| {},
            );
        }
        if !st.google_busy() && !ui.google_opened.borrow().is_empty() {
            ui.google_requested.set(false);
            ui.toast(&st.google_label());
        }
    }

    // Contrôles : laissés tranquilles juste après une action de l'utilisateur.
    if Instant::now() >= ui.hold_until.get() {
        let level = if st.brightness >= 0 {
            st.brightness
        } else {
            st.firmware_brightness
        };
        if level >= 0 {
            ui.brightness.set_value(f64::from(level));
            ui.brightness_caption.set_label(&format!("{level} %"));
        }
        ui.rotation.set_selected(u32::from(st.rotation == 180));
        ui.fps.set_value(st.fps);
        for card in ui.hero_cards.iter().chain(&ui.content_cards) {
            card.button.set_active(card.mode.as_str() == st.mode);
        }
        if let Some(i) = FITS
            .iter()
            .position(|(fit, _)| fit.as_str() == st.image_fit)
        {
            ui.image_fit.set_selected(i as u32);
        }
        ui.video_loop.set_active(st.video_loop);
        if let Some(rgba) = parse_hex(&st.color) {
            ui.color.set_rgba(&rgba);
        }
        if let Some(i) = ON_EXIT.iter().position(|(b, _)| b.as_str() == st.on_exit) {
            ui.on_exit.set_selected(i as u32);
        }
        ui.http.set_active(st.http_enabled);
        if !ui.http_addr.has_focus() && ui.http_addr.text() != st.http_addr {
            ui.http_addr.set_text(&st.http_addr);
        }
        ui.slide_interval.set_value(st.slideshow_interval);
        if let Some(i) = ORDERS.iter().position(|(o, _)| *o == st.slideshow_order) {
            ui.slide_order.set_selected(i as u32);
        }
        if let Some(i) = SLIDE_FITS.iter().position(|(f, _)| *f == st.slideshow_fit) {
            ui.slide_fit.set_selected(i as u32);
        }
        ui.slide_fade.set_active(st.slideshow_transition_ms > 0);
        ui.slide_overlay.set_active(st.slideshow_overlay);
        ui.slide_clock.set_active(st.slideshow_clock);
        if !ui.google_id.has_focus() && ui.google_id.text().is_empty() {
            ui.google_id.set_text(&st.google_client_id);
        }
        apply_panels(ui, remote, &st.widgets);
    }
    apply_folders(ui, remote, &st.slideshow_folders);
    ui.updating.set(false);
}

/// Tuiles des dossiers du diaporama, reconstruites quand la liste change.
fn apply_folders(ui: &Rc<Ui>, remote: &Remote, folders: &[String]) {
    if ui.shown_folders.borrow().as_deref() == Some(folders) {
        return;
    }
    ui.folders_flow.remove_all();
    for folder in folders {
        let cover = gtk::Stack::new();
        cover.set_transition_type(gtk::StackTransitionType::Crossfade);
        let icon = gtk::Image::from_icon_name("folder-pictures-symbolic");
        cover.add_named(&icon, Some("icon"));
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Cover);
        picture.set_can_shrink(true);
        cover.add_named(&picture, Some("thumb"));
        let cover_frame = crate::ui::art_frame(&cover, (176, 110));
        cover_frame.remove_css_class("art");
        cover_frame.add_css_class("cover");

        let name = Path::new(folder)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder.clone());
        let title = gtk::Label::new(Some(&name));
        title.add_css_class("title");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let subtitle = gtk::Label::new(Some("…"));
        subtitle.add_css_class("subtitle");
        subtitle.set_xalign(0.0);
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
        texts.set_hexpand(true);
        texts.append(&title);
        texts.append(&subtitle);
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Retirer ce dossier du diaporama")
            .valign(gtk::Align::Center)
            .build();
        remove.add_css_class("flat");
        remove.add_css_class("circular");
        remove.update_property(&[gtk::accessible::Property::Label(&format!(
            "Retirer le dossier {name} du diaporama"
        ))]);
        remove.connect_clicked({
            let (ui, remote, folder) = (ui.clone(), remote.clone(), folder.clone());
            move |_| {
                let folders: Vec<String> = ui
                    .state
                    .borrow()
                    .slideshow_folders
                    .iter()
                    .filter(|f| **f != folder)
                    .cloned()
                    .collect();
                ui.user_action();
                send_slideshow(&ui, &remote, Some(folders));
                ui.toast("Dossier retiré du diaporama");
            }
        });
        let caption = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        caption.set_margin_start(4);
        caption.append(&texts);
        caption.append(&remove);
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 8);
        tile.add_css_class("folder-tile");
        tile.set_tooltip_text(Some(folder));
        tile.append(&cover_frame);
        tile.append(&caption);
        flow_append(&ui.folders_flow, &tile);

        // Couverture et nombre de photos, calculés hors du fil de l'interface.
        let (ui_, folder) = (ui.clone(), PathBuf::from(folder));
        glib::spawn_future_local(async move {
            let Ok((first, count, capped)) =
                gio::spawn_blocking(move || folder_summary(&folder)).await
            else {
                return;
            };
            subtitle.set_label(&match (count, capped) {
                (0, _) => "Aucune photo".to_owned(),
                (n, true) => format!("plus de {n} photos"),
                (n, false) => plural(n, "photo", "photos"),
            });
            if let Some(path) = first.and_then(|p| p.to_str().map(str::to_owned)) {
                with_thumbnail(&ui_, &path, move |texture| {
                    picture.set_paintable(Some(texture));
                    cover.set_visible_child_name("thumb");
                });
            }
        });
    }
    flow_append(&ui.folders_flow, &ui.add_folder);
    *ui.shown_folders.borrow_mut() = Some(folders.to_vec());
}

/// Ordre d'affichage des lignes : panneaux activés dans l'ordre, puis les autres.
pub fn panel_rows(enabled: &[String]) -> Vec<(&'static str, bool)> {
    let mut rows: Vec<(&'static str, bool)> = enabled
        .iter()
        .filter_map(|w| PANELS.iter().find(|(name, ..)| name == w))
        .map(|(name, ..)| (*name, true))
        .collect();
    for (name, ..) in PANELS {
        if !rows.iter().any(|(n, _)| *n == name) {
            rows.push((name, false));
        }
    }
    rows
}

/// Liste après déplacement d'un panneau activé d'un cran (`delta` = -1 ou +1).
pub fn moved(enabled: &[String], name: &str, delta: i32) -> Vec<String> {
    let mut list = enabled.to_vec();
    if let Some(i) = list.iter().position(|w| w == name) {
        let j = i as i32 + delta;
        if j >= 0 && (j as usize) < list.len() {
            list.swap(i, j as usize);
        }
    }
    list
}

/// Liste après activation ou désactivation d'un panneau.
pub fn toggled(enabled: &[String], name: &str, on: bool) -> Vec<String> {
    let mut list: Vec<String> = enabled.iter().filter(|w| *w != name).cloned().collect();
    if on {
        list.push(name.to_owned());
    }
    list
}

/// Schéma et lignes des panneaux, reconstruits quand la liste change.
fn apply_panels(ui: &Rc<Ui>, remote: &Remote, widgets: &[String]) {
    if ui.shown_panels.borrow().as_deref() == Some(widgets) {
        return;
    }
    let was = ui.updating.replace(true);

    // Schéma : rangée du haut (carte graphique, processeur) et rangée du bas.
    while let Some(child) = ui.schematic.first_child() {
        ui.schematic.remove(&child);
    }
    let short = |name: &str| match name {
        "gpu" => "GPU",
        "cpu" => "CPU",
        "memory" => "RAM",
        "disks" => "Disques",
        _ => "Réseau",
    };
    let mut any = false;
    for top in [true, false] {
        let names: Vec<&str> = widgets
            .iter()
            .filter_map(|w| PANELS.iter().find(|(n, _, _, t)| n == w && *t == top))
            .map(|(n, ..)| *n)
            .collect();
        if names.is_empty() {
            continue;
        }
        any = true;
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_homogeneous(true);
        for name in names {
            let tile = gtk::Label::new(Some(short(name)));
            tile.add_css_class("tile");
            tile.add_css_class(&format!("tile-{name}"));
            if !top {
                tile.add_css_class("small");
            }
            tile.set_xalign(0.0);
            tile.set_hexpand(true);
            row.append(&tile);
        }
        ui.schematic.append(&row);
    }
    if !any {
        let empty = gtk::Label::new(Some("Aucun panneau activé : l'écran reste vide"));
        empty.add_css_class("empty");
        ui.schematic.append(&empty);
    }

    // Lignes : pastille de couleur, monter, descendre, interrupteur.
    ui.panel_list.remove_all();
    let rows = panel_rows(widgets);
    let enabled_count = rows.iter().filter(|(_, on)| *on).count();
    for (index, (name, on)) in rows.iter().enumerate() {
        let (_, title, subtitle, _) = PANELS
            .iter()
            .find(|(n, ..)| n == name)
            .expect("panneau connu");
        let row = adw::ActionRow::builder()
            .title(*title)
            .subtitle(*subtitle)
            .build();
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("panel-dot");
        dot.add_css_class(&format!("dot-{name}"));
        dot.set_valign(gtk::Align::Center);
        row.add_prefix(&dot);
        for (icon, delta, tip, sensitive) in [
            ("go-up-symbolic", -1, "Monter", *on && index > 0),
            (
                "go-down-symbolic",
                1,
                "Descendre",
                *on && index + 1 < enabled_count,
            ),
        ] {
            let button = gtk::Button::from_icon_name(icon);
            button.add_css_class("flat");
            button.set_valign(gtk::Align::Center);
            button.set_sensitive(sensitive);
            button.set_tooltip_text(Some(tip));
            button.update_property(&[gtk::accessible::Property::Label(&format!(
                "{tip} le panneau {title}"
            ))]);
            let (ui_, remote, name) = (ui.clone(), remote.clone(), *name);
            button.connect_clicked(move |_| {
                let current = ui_.state.borrow().widgets.clone();
                send_widgets(&ui_, &remote, moved(&current, name, delta));
            });
            row.add_suffix(&button);
        }
        let switch = gtk::Switch::new();
        switch.set_active(*on);
        switch.set_valign(gtk::Align::Center);
        switch.update_property(&[gtk::accessible::Property::Label(&format!(
            "Afficher le panneau {title}"
        ))]);
        let (ui_, remote, name) = (ui.clone(), remote.clone(), *name);
        switch.connect_active_notify(move |switch| {
            if ui_.updating.get() {
                return;
            }
            let current = ui_.state.borrow().widgets.clone();
            send_widgets(&ui_, &remote, toggled(&current, name, switch.is_active()));
        });
        row.add_suffix(&switch);
        row.set_activatable_widget(Some(&switch));
        ui.panel_list.append(&row);
    }
    *ui.shown_panels.borrow_mut() = Some(widgets.to_vec());
    ui.updating.set(was);
}

/// État du service : page Service et écran « hors service ».
pub fn apply_service(ui: &Ui, service: &ServiceState, daemon_running: bool) {
    ui.updating.set(true);
    let (tone, icon) = match service.active.as_str() {
        "active" => ("ok", "object-select-symbolic"),
        "failed" => ("bad", "dialog-warning-symbolic"),
        _ => ("", "system-run-symbolic"),
    };
    for class in ["ok", "bad"] {
        if class == tone {
            ui.service_badge.add_css_class(class);
        } else {
            ui.service_badge.remove_css_class(class);
        }
    }
    ui.service_icon.set_icon_name(Some(icon));
    ui.service_title.set_label(service.active_label());
    ui.service_hint.set_label(match service.active.as_str() {
        "active" => {
            "Le service pilote l'écran en arrière-plan, même quand cette fenêtre est fermée."
        }
        "failed" => {
            "Consultez « journalctl --user -u lightning-dashboard », puis redémarrez le service."
        }
        _ => "L'écran n'est pas piloté tant que le service est arrêté.",
    });
    if Instant::now() >= ui.hold_until.get() {
        ui.service_active.set_active(service.is_active());
        ui.service_enabled.set_active(service.is_enabled());
    }
    ui.service_active.set_sensitive(service.is_installed());
    ui.service_enabled.set_sensitive(service.is_installed());
    ui.restart_button.set_sensitive(service.is_installed());
    if !daemon_running {
        let (title, description, can_start) = if !service.is_installed() {
            (
                "Le service Lightning n'est pas installé",
                "Installez l'unité lightning-dashboard.service (voir le README), puis relancez l'application.",
                false,
            )
        } else if service.active == "failed" {
            (
                "Le service Lightning a échoué",
                "Consultez « journalctl --user -u lightning-dashboard », puis relancez le service.",
                true,
            )
        } else if service.is_active() {
            ("Le service démarre…", "Connexion au démon en cours.", false)
        } else {
            (
                "Le service Lightning est arrêté",
                "L'écran de la carte graphique n'est pas piloté pour l'instant.",
                true,
            )
        };
        ui.offline.set_title(title);
        ui.offline.set_description(Some(description));
        ui.offline_button.set_visible(can_start);
    }
    ui.updating.set(false);
}

/// Affiche une nouvelle image de l'aperçu (fondu à la première).
pub fn apply_frame(ui: &Ui, texture: &gdk::Texture) {
    ui.hero_picture.set_paintable(Some(texture));
    if !ui.frame_shown.replace(true) {
        ui.hero_empty.set_visible(false);
        // Apparition douce de la première image.
        let picture = ui.hero_picture.clone();
        let target = adw::CallbackAnimationTarget::new(move |value| picture.set_opacity(value));
        adw::TimedAnimation::new(&ui.hero_picture, 0.0, 1.0, 450, target).play();
    }
    if ui.state.borrow().mode == "dashboard" {
        *ui.dashboard_thumb.borrow_mut() = Some(texture.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn colors_roundtrip() {
        let rgba = parse_hex("ff8000").unwrap();
        assert_eq!(hex_color(&rgba), "ff8000");
        assert_eq!(hex_color(&gdk::RGBA::new(0.0, 1.0, 0.5, 1.0)), "00ff80");
        assert!(parse_hex("fff").is_none());
        assert!(parse_hex("zzzzzz").is_none());
        assert!(parse_hex("ééé").is_none());
    }

    #[test]
    fn labels_and_tables_are_consistent() {
        assert_eq!(file_name(""), "Aucun fichier choisi");
        assert_eq!(file_name("/home/moi/Images/été.jpg"), "été.jpg");
        assert_eq!(mode_label("msi_animation"), "Animation d'origine");
        for mode in Mode::ALL {
            assert_ne!(mode_label(mode.as_str()), "—", "{mode}");
        }
        assert_eq!(mode_label("inconnu"), "—");
        // Chaque panneau connu du démon a sa ligne, dans le même ordre.
        let names: Vec<&str> = PANELS.iter().map(|(n, ..)| *n).collect();
        assert_eq!(names, lightning_ipc::WIDGETS);
        assert_eq!(plural(1, "photo", "photos"), "1 photo");
        assert_eq!(plural(3, "photo", "photos"), "3 photos");
        for hex in crate::ui::SWATCHES {
            assert!(parse_hex(hex).is_some(), "{hex}");
        }
    }

    #[test]
    fn panel_order_logic() {
        let enabled = list(&["cpu", "network", "gpu"]);
        assert_eq!(
            panel_rows(&enabled),
            [
                ("cpu", true),
                ("network", true),
                ("gpu", true),
                ("memory", false),
                ("disks", false)
            ]
        );
        assert_eq!(panel_rows(&[]).iter().filter(|(_, on)| *on).count(), 0);
        assert_eq!(panel_rows(&list(&["inconnu", "gpu"]))[0], ("gpu", true));

        assert_eq!(
            moved(&enabled, "network", -1),
            list(&["network", "cpu", "gpu"])
        );
        assert_eq!(
            moved(&enabled, "network", 1),
            list(&["cpu", "gpu", "network"])
        );
        assert_eq!(moved(&enabled, "cpu", -1), enabled, "déjà en tête");
        assert_eq!(moved(&enabled, "gpu", 1), enabled, "déjà en queue");
        assert_eq!(moved(&enabled, "memory", 1), enabled, "panneau désactivé");

        assert_eq!(
            toggled(&enabled, "memory", true),
            list(&["cpu", "network", "gpu", "memory"])
        );
        assert_eq!(toggled(&enabled, "network", false), list(&["cpu", "gpu"]));
        assert_eq!(
            toggled(&enabled, "cpu", true),
            list(&["network", "gpu", "cpu"])
        );
    }

    #[test]
    fn card_subtitles() {
        let mut st = DaemonState {
            mode: "dashboard".into(),
            color: "ff8800".into(),
            ..Default::default()
        };
        assert_eq!(
            card_subtitle(Mode::Image, &st, "Une image fixe"),
            "Une image fixe"
        );
        assert_eq!(card_subtitle(Mode::Color, &st, ""), "#ff8800");
        assert_eq!(
            card_subtitle(Mode::Slideshow, &st, ""),
            "Aucun dossier choisi"
        );
        st.image_path = "/a/b/photo.png".into();
        st.slideshow_folders = vec!["/a".into(), "/b".into()];
        assert_eq!(card_subtitle(Mode::Image, &st, ""), "photo.png");
        assert_eq!(card_subtitle(Mode::Slideshow, &st, ""), "2 dossiers");
        st.mode = "slideshow".into();
        st.slideshow_count = 14;
        assert_eq!(
            card_subtitle(Mode::Slideshow, &st, ""),
            "14 photos · 2 dossiers"
        );
    }

    #[test]
    fn folder_summary_counts_photos_and_finds_a_cover() {
        let dir = std::env::temp_dir().join(format!("lightning-gtk-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("b/sous")).unwrap();
        std::fs::create_dir_all(dir.join(".cache")).unwrap();
        for name in [
            "b/sous/2.JPG",
            "b/1.png",
            "notes.txt",
            ".cache/x.jpg",
            "a.webp",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let (first, count, capped) = folder_summary(&dir);
        assert_eq!(count, 3);
        assert!(!capped);
        assert_eq!(first.unwrap(), dir.join("a.webp"));
        assert_eq!(folder_summary(&dir.join("absent")), (None, 0, false));
        let _ = std::fs::remove_dir_all(dir);
    }
}
