//! Construction de l'interface : fenêtre adaptative à navigation latérale,
//! page d'accueil avec l'aperçu en direct, cartes de contenu, pages de réglage.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use adw::prelude::*;
use gtk::{gdk, gio};
use lightning_ipc::{Fit, Mode, OnExit};

use crate::remote::DaemonState;

pub const APP_ID: &str = "cloud.wizops.Lightning";

/// Pages de la navigation latérale : `(nom, titre, icône, sous-titre)`.
pub const PAGES: [(&str, &str, &str, &str); 6] = [
    (
        "accueil",
        "Accueil",
        "go-home-symbolic",
        "Ce que l'écran affiche en ce moment",
    ),
    (
        "contenu",
        "Contenu",
        "image-x-generic-symbolic",
        "Choisissez ce que l'écran affiche",
    ),
    (
        "diaporama",
        "Diaporama",
        "folder-pictures-symbolic",
        "Vos photos, depuis ce PC ou Google Photos",
    ),
    (
        "dashboard",
        "Tableau de bord",
        "view-grid-symbolic",
        "Panneaux affichés et leur ordre",
    ),
    (
        "reglages",
        "Réglages",
        "preferences-system-symbolic",
        "Écran, arrêt et aperçu dans un navigateur",
    ),
    (
        "service",
        "Service",
        "system-run-symbolic",
        "Le programme qui fait tourner l'écran en arrière-plan",
    ),
];

/// Contenus proposés : `(mode, titre, icône, description)`.
pub const MODES: [(Mode, &str, &str, &str); 8] = [
    (
        Mode::Dashboard,
        "Tableau de bord",
        "view-grid-symbolic",
        "État du PC en temps réel",
    ),
    (
        Mode::Image,
        "Image",
        "image-x-generic-symbolic",
        "Une image fixe",
    ),
    (
        Mode::Video,
        "Vidéo",
        "video-x-generic-symbolic",
        "Une vidéo, en boucle ou non",
    ),
    (
        Mode::Slideshow,
        "Diaporama",
        "folder-pictures-symbolic",
        "Vos photos qui défilent",
    ),
    (
        Mode::Clock,
        "Horloge",
        "preferences-system-time-symbolic",
        "L'heure en grand, lisible de loin",
    ),
    (
        Mode::NowPlaying,
        "En cours de lecture",
        "audio-x-generic-symbolic",
        "Le morceau joué sur ce PC",
    ),
    (
        Mode::Color,
        "Couleur",
        "color-select-symbolic",
        "Une couleur unie",
    ),
    (
        Mode::MsiAnimation,
        "Animation d'origine",
        "weather-storm-symbolic",
        "L'écran reprend son animation",
    ),
];

/// Panneaux du tableau de bord : `(nom, titre, description, rangée du haut)`.
pub const PANELS: [(&str, &str, &str, bool); 5] = [
    (
        "gpu",
        "Carte graphique",
        "Charge, température, mémoire vidéo, puissance",
        true,
    ),
    (
        "cpu",
        "Processeur",
        "Charge globale et par cœur, température, fréquence",
        true,
    ),
    (
        "memory",
        "Mémoire",
        "Mémoire vive et espace d'échange utilisés",
        false,
    ),
    (
        "disks",
        "Disques",
        "Occupation des points de montage",
        false,
    ),
    ("network", "Réseau", "Débits descendant et montant", false),
];

pub const ORDERS: [(&str, &str); 2] = [("random", "Aléatoire"), ("chronological", "Chronologique")];
pub const SLIDE_FITS: [(&str, &str); 2] = [
    ("contain", "Photo entière sur fond flou"),
    ("cover", "Remplir l'écran (rogner)"),
];
pub const ON_EXIT: [(OnExit, &str); 3] = [
    (OnExit::Keep, "Conserver la dernière image"),
    (OnExit::Blank, "Afficher un écran noir"),
    (OnExit::Restore, "Rendre l'écran à son animation"),
];
pub const FITS: [(Fit, &str); 3] = [
    (Fit::Cover, "Remplir (rogner)"),
    (Fit::Contain, "Adapter (bandes noires)"),
    (Fit::Stretch, "Étirer"),
];
/// Couleurs proposées d'un clic.
pub const SWATCHES: [&str; 8] = [
    "000000", "101418", "f2b441", "ff8a4c", "f25454", "b289ff", "54a6ff", "2dd4bf",
];

/// Carte illustrée d'un contenu (accueil et page Contenu).
pub struct Card {
    pub mode: Mode,
    pub button: gtk::ToggleButton,
    pub art: gtk::Stack,
    pub thumb: gtk::Picture,
    pub color: Rc<Cell<gdk::RGBA>>,
    pub swatch: gtk::DrawingArea,
    pub subtitle: gtk::Label,
    /// Fichier dont la vignette est affichée (ou demandée).
    pub thumb_key: RefCell<String>,
}

/// Pastille d'état de l'accueil.
pub struct Chip {
    pub root: gtk::Box,
    pub label: gtk::Label,
}

impl Chip {
    fn new(icon: Option<&str>, with_dot: bool, tooltip: &str) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        root.add_css_class("chip");
        root.set_halign(gtk::Align::Center);
        root.set_tooltip_text(Some(tooltip));
        if with_dot {
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("dot");
            dot.set_valign(gtk::Align::Center);
            root.append(&dot);
        }
        if let Some(icon) = icon {
            root.append(&gtk::Image::from_icon_name(icon));
        }
        let label = gtk::Label::new(Some("—"));
        label.add_css_class("numeric");
        root.append(&label);
        Self { root, label }
    }

    /// Change la tonalité : `ok`, `busy`, `bad` ou neutre (chaîne vide).
    pub fn set_tone(&self, tone: &str) {
        for class in ["ok", "busy", "bad"] {
            if class == tone {
                self.root.add_css_class(class);
            } else {
                self.root.remove_css_class(class);
            }
        }
    }
}

/// Widgets de l'interface et état local.
pub struct Ui {
    pub window: adw::ApplicationWindow,
    pub toasts: adw::ToastOverlay,
    pub root: gtk::Stack,
    pub split: adw::NavigationSplitView,
    pub nav_list: gtk::ListBox,
    pub content_page: adw::NavigationPage,
    pub pages: gtk::Stack,
    pub offline: adw::StatusPage,
    pub offline_button: gtk::Button,
    pub sidebar_state: gtk::Label,
    pub sidebar_mode: gtk::Label,
    // Accueil
    pub hero_empty: gtk::Box,
    pub hero_picture: gtk::Picture,
    pub chip_lcd: Chip,
    pub chip_mode: Chip,
    pub chip_fps: Chip,
    pub chip_render: Chip,
    pub chip_brightness: Chip,
    pub hero_cards: Vec<Card>,
    pub brightness: gtk::Adjustment,
    pub brightness_caption: gtk::Label,
    // Contenu
    pub content_cards: Vec<Card>,
    pub details: gtk::Stack,
    pub image_row: adw::ActionRow,
    pub image_button: gtk::Button,
    pub image_fit: adw::ComboRow,
    pub video_row: adw::ActionRow,
    pub video_button: gtk::Button,
    pub video_loop: adw::SwitchRow,
    pub clock_seconds: adw::SwitchRow,
    pub clock_date: adw::SwitchRow,
    pub now_playing_row: adw::ActionRow,
    pub now_playing_player: adw::EntryRow,
    pub color: gtk::ColorDialogButton,
    pub swatches: Vec<(String, gtk::Button)>,
    pub slideshow_summary: adw::ActionRow,
    pub slideshow_open: gtk::Button,
    // Diaporama
    pub folders_flow: gtk::FlowBox,
    pub shown_folders: RefCell<Option<Vec<String>>>,
    pub add_folder: gtk::Button,
    pub slide_interval: adw::SpinRow,
    pub slide_order: adw::ComboRow,
    pub slide_fit: adw::ComboRow,
    pub slide_fade: adw::SwitchRow,
    pub slide_overlay: adw::SwitchRow,
    pub slide_clock: adw::SwitchRow,
    pub google_state: gtk::Label,
    pub google_button: gtk::Button,
    pub google_cache: gtk::Label,
    pub google_clear: gtk::Button,
    pub google_id: adw::EntryRow,
    pub google_secret: adw::PasswordEntryRow,
    pub google_save: gtk::Button,
    pub google_requested: Cell<bool>,
    pub google_opened: RefCell<String>,
    // Tableau de bord
    pub schematic: gtk::Box,
    pub panel_list: gtk::ListBox,
    pub shown_panels: RefCell<Option<Vec<String>>>,
    // Réglages
    pub rotation: adw::ComboRow,
    pub fps: adw::SpinRow,
    pub lcd_row: adw::ActionRow,
    pub stats_row: adw::ActionRow,
    pub on_exit: adw::ComboRow,
    pub http: adw::SwitchRow,
    pub http_addr: adw::EntryRow,
    pub browser_button: gtk::Button,
    // Service
    pub service_badge: gtk::Box,
    pub service_icon: gtk::Image,
    pub service_title: gtk::Label,
    pub service_hint: gtk::Label,
    pub service_active: adw::SwitchRow,
    pub service_enabled: adw::SwitchRow,
    pub restart_button: gtk::Button,
    pub version_row: adw::ActionRow,
    // État local
    /// Vrai pendant que l'interface recopie l'état du démon (pas d'appel D-Bus).
    pub updating: Cell<bool>,
    /// Après une action, l'état du démon n'écrase pas les contrôles avant cet instant.
    pub hold_until: Cell<Instant>,
    pub debounce: Cell<u32>,
    pub force_offline: Cell<bool>,
    pub state: RefCell<DaemonState>,
    pub last_frame: RefCell<Vec<u8>>,
    pub frame_shown: Cell<bool>,
    /// Vignettes déjà calculées, par fichier.
    pub thumbs: RefCell<HashMap<String, gdk::Texture>>,
    /// Dernière image vue du tableau de bord (vignette de sa carte).
    pub dashboard_thumb: RefCell<Option<gdk::Texture>>,
}

impl Ui {
    pub fn toast(&self, message: &str) {
        let toast = adw::Toast::new(message);
        toast.set_timeout(4);
        self.toasts.add_toast(toast);
    }

    /// Vrai si le changement vient de l'utilisateur ; arme alors la retenue.
    pub fn user_action(&self) -> bool {
        if self.updating.get() {
            return false;
        }
        self.hold_until
            .set(Instant::now() + std::time::Duration::from_millis(1500));
        true
    }

    /// Affiche une page de la navigation.
    pub fn show_page(&self, name: &str) {
        let Some(index) = PAGES.iter().position(|(n, ..)| *n == name) else {
            return;
        };
        self.pages.set_visible_child_name(name);
        self.content_page.set_title(PAGES[index].1);
        if let Some(row) = self.nav_list.row_at_index(index as i32)
            && self.nav_list.selected_row().as_ref() != Some(&row)
        {
            self.nav_list.select_row(Some(&row));
        }
    }
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    for class in classes {
        label.add_css_class(class);
    }
    label
}

/// Titre de section avec indication facultative.
fn section(title: &str, hint: Option<&str>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    b.add_css_class("section-head");
    b.append(&label(title, &["section-title"]));
    if let Some(hint) = hint {
        b.append(&label(hint, &["section-hint"]));
    }
    b
}

/// Page défilante : titre, sous-titre, puis le contenu, dans une colonne bornée.
fn page(name: &str, children: &[&gtk::Widget]) -> gtk::ScrolledWindow {
    let (_, title, _, subtitle) = PAGES
        .iter()
        .find(|(n, ..)| *n == name)
        .expect("page connue");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 22);
    column.set_margin_top(20);
    column.set_margin_bottom(36);
    column.set_margin_start(24);
    column.set_margin_end(24);
    let heading = gtk::Box::new(gtk::Orientation::Vertical, 2);
    heading.append(&label(title, &["page-title"]));
    heading.append(&label(subtitle, &["page-subtitle"]));
    column.append(&heading);
    // Un titre de section reste proche du bloc qu'il introduit.
    let mut pending: Option<gtk::Box> = None;
    for child in children {
        if child.has_css_class("section-head") {
            let block = gtk::Box::new(gtk::Orientation::Vertical, 10);
            block.append(*child);
            column.append(&block);
            pending = Some(block);
        } else if let Some(block) = pending.take() {
            block.append(*child);
        } else {
            column.append(*child);
        }
    }
    let clamp = adw::Clamp::builder()
        .maximum_size(940)
        .tightening_threshold(700)
        .child(&column)
        .build();
    // Hauteur naturelle (et non minimale) pour le contenu : sans cela, l'aperçu,
    // qui sait rétrécir, serait écrasé dès que la page dépasse la fenêtre.
    let viewport = gtk::Viewport::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    viewport.set_vscroll_policy(gtk::ScrollablePolicy::Natural);
    viewport.set_child(Some(&clamp));
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&viewport)
        .build()
}

fn group(rows: &[&gtk::Widget]) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    for row in rows {
        group.add(*row);
    }
    group
}

fn info_row(title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle("—").build();
    row.add_css_class("property");
    row
}

fn string_list(labels: impl IntoIterator<Item = &'static str>) -> gtk::StringList {
    gtk::StringList::new(&labels.into_iter().collect::<Vec<_>>())
}

fn accessible(widget: &impl IsA<gtk::Accessible>, text: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(text)]);
}

fn flow(min: u32, max: u32) -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .column_spacing(14)
        .row_spacing(14)
        .min_children_per_line(min)
        .max_children_per_line(max)
        .build()
}

/// Ajoute un enfant à une grille sans que son conteneur prenne le focus à sa place.
pub fn flow_append(flow: &gtk::FlowBox, child: &impl IsA<gtk::Widget>) {
    flow.append(child);
    if let Some(wrapper) = child.parent() {
        wrapper.set_focusable(false);
    }
}

/// Cadre d'illustration de taille fixe (16:10) ; `content` y est superposé et
/// n'influence pas la taille demandée.
pub fn art_frame(content: &impl IsA<gtk::Widget>, size: (i32, i32)) -> gtk::Overlay {
    let base = gtk::Box::new(gtk::Orientation::Vertical, 0);
    base.set_size_request(size.0, size.1);
    let frame = gtk::Overlay::new();
    frame.add_css_class("art");
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.set_child(Some(&base));
    frame.add_overlay(content);
    frame
}

fn card(mode: Mode, title: &str, icon: &str, description: &str, large: bool) -> Card {
    // Les vignettes sont posées en superposition d'un fond de taille fixe : une
    // grande image ne dicte ainsi jamais la largeur de la carte.
    let art = gtk::Stack::new();
    art.set_transition_type(gtk::StackTransitionType::Crossfade);
    art.set_transition_duration(250);
    let image = gtk::Image::from_icon_name(icon);
    image.set_halign(gtk::Align::Center);
    image.set_valign(gtk::Align::Center);
    art.add_named(&image, Some("icon"));
    let thumb = gtk::Picture::new();
    thumb.set_content_fit(gtk::ContentFit::Cover);
    thumb.set_can_shrink(true);
    art.add_named(&thumb, Some("thumb"));
    let color = Rc::new(Cell::new(gdk::RGBA::new(0.06, 0.08, 0.1, 1.0)));
    let swatch = gtk::DrawingArea::new();
    swatch.set_draw_func({
        let color = color.clone();
        move |_, cr, w, h| {
            let c = color.get();
            cr.set_source_rgb(
                f64::from(c.red()),
                f64::from(c.green()),
                f64::from(c.blue()),
            );
            cr.rectangle(0.0, 0.0, f64::from(w), f64::from(h));
            let _ = cr.fill();
        }
    });
    art.add_named(&swatch, Some("color"));
    let art_frame = art_frame(&art, if large { (208, 130) } else { (128, 80) });

    let title_label = label(title, &["title"]);
    title_label.set_wrap(false);
    title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let subtitle = label(description, &["subtitle"]);
    subtitle.set_wrap(false);
    subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
    // Largeur naturelle bornée : un long sous-titre (nom de fichier, morceau en
    // cours) ne doit pas réduire le nombre de cartes par ligne.
    title_label.set_max_width_chars(18);
    subtitle.set_max_width_chars(18);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
    texts.set_hexpand(true);
    texts.append(&title_label);
    if large {
        texts.append(&subtitle);
    }
    let check = gtk::Image::from_icon_name("object-select-symbolic");
    check.add_css_class("check");
    check.set_valign(gtk::Align::Center);
    let caption = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    caption.set_margin_start(4);
    caption.set_margin_end(4);
    caption.set_margin_bottom(2);
    caption.append(&texts);
    caption.append(&check);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.append(&art_frame);
    body.append(&caption);

    let button = gtk::ToggleButton::new();
    button.set_child(Some(&body));
    button.add_css_class("mode-card");
    button.add_css_class("flat");
    if large {
        button.add_css_class("large");
    }
    button.set_tooltip_text(Some(description));
    accessible(&button, &format!("{title} : {description}"));
    Card {
        mode,
        button,
        art,
        thumb,
        color,
        swatch,
        subtitle,
        thumb_key: RefCell::new(String::new()),
    }
}

fn cards(large: bool) -> (gtk::FlowBox, Vec<Card>) {
    // Une seule grande carte par ligne reste possible en fenêtre étroite.
    let flow = if large { flow(1, 3) } else { flow(2, 6) };
    let cards: Vec<Card> = MODES
        .iter()
        .map(|(mode, title, icon, description)| card(*mode, title, icon, description, large))
        .collect();
    for card in &cards {
        flow_append(&flow, &card.button);
    }
    (flow, cards)
}

/// Construit toute l'interface (sans la brancher).
pub fn build(app: &adw::Application) -> Rc<Ui> {
    // ------------------------------------------------------------ accueil
    let hero_picture = gtk::Picture::new();
    hero_picture.set_content_fit(gtk::ContentFit::Cover);
    hero_picture.set_can_shrink(true);
    accessible(&hero_picture, "Aperçu en direct de l'écran");
    let hero_empty = gtk::Box::new(gtk::Orientation::Vertical, 10);
    hero_empty.add_css_class("hero-empty");
    hero_empty.set_valign(gtk::Align::Center);
    hero_empty.set_halign(gtk::Align::Center);
    hero_empty.append(&gtk::Image::from_icon_name("video-display-symbolic"));
    hero_empty.append(&gtk::Label::new(Some("En attente d'une image…")));
    // L'image donne la hauteur selon la largeur (16:10) ; avant la première
    // image du démon, une texture sombre aux mêmes proportions tient la place.
    let blank = gdk::MemoryTexture::new(
        16,
        10,
        gdk::MemoryFormat::R8g8b8,
        &gtk::glib::Bytes::from_owned(vec![9u8; 16 * 10 * 3]),
        16 * 3,
    );
    hero_picture.set_paintable(Some(&blank));
    hero_picture.set_content_fit(gtk::ContentFit::Contain);
    let hero = gtk::Overlay::new();
    hero.add_css_class("hero-screen");
    hero.set_overflow(gtk::Overflow::Hidden);
    hero.set_child(Some(&hero_picture));
    hero.add_overlay(&hero_empty);
    let bezel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bezel.add_css_class("hero-bezel");
    bezel.append(&hero);
    // Largeur bornée : à peu près la taille réelle de la dalle de 9,6 pouces.
    let hero_clamp = adw::Clamp::builder()
        .maximum_size(680)
        .tightening_threshold(480)
        .child(&bezel)
        .build();
    hero_clamp.set_margin_top(6);
    hero_clamp.set_margin_bottom(10);

    let chip_lcd = Chip::new(None, true, "État de la liaison avec l'écran");
    let chip_mode = Chip::new(Some("image-x-generic-symbolic"), false, "Contenu affiché");
    let chip_fps = Chip::new(
        Some("media-playback-start-symbolic"),
        false,
        "Images envoyées par seconde",
    );
    let chip_render = Chip::new(
        Some("preferences-system-time-symbolic"),
        false,
        "Temps de rendu de la dernière image",
    );
    let chip_brightness = Chip::new(
        Some("display-brightness-symbolic"),
        false,
        "Luminosité confirmée par l'écran",
    );
    let chips = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    chips.set_halign(gtk::Align::Center);
    let chips_flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(8)
        .row_spacing(8)
        .min_children_per_line(2)
        .max_children_per_line(5)
        .halign(gtk::Align::Center)
        .build();
    for chip in [
        &chip_lcd,
        &chip_mode,
        &chip_fps,
        &chip_render,
        &chip_brightness,
    ] {
        flow_append(&chips_flow, &chip.root);
    }
    chips.append(&chips_flow);

    let (hero_flow, hero_cards) = cards(false);
    let brightness = gtk::Adjustment::new(100.0, 0.0, 100.0, 1.0, 10.0, 0.0);
    let quick_scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&brightness));
    quick_scale.set_hexpand(true);
    quick_scale.set_draw_value(false);
    accessible(&quick_scale, "Luminosité de l'écran");
    let brightness_caption = gtk::Label::new(Some("—"));
    brightness_caption.add_css_class("numeric");
    brightness_caption.set_width_chars(6);
    brightness_caption.set_xalign(1.0);
    let quick = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    quick.add_css_class("quick-panel");
    quick.append(&gtk::Image::from_icon_name("display-brightness-symbolic"));
    let quick_title = gtk::Label::new(Some("Luminosité"));
    quick_title.add_css_class("heading");
    quick.append(&quick_title);
    quick.append(&quick_scale);
    quick.append(&brightness_caption);

    let page_home = page(
        "accueil",
        &[
            hero_clamp.upcast_ref(),
            chips.upcast_ref(),
            section("Afficher", Some("Changez de contenu d'un clic")).upcast_ref(),
            hero_flow.upcast_ref(),
            quick.upcast_ref(),
        ],
    );

    // ------------------------------------------------------------ contenu
    let (content_flow, content_cards) = cards(true);
    let details = gtk::Stack::new();
    details.set_transition_type(gtk::StackTransitionType::Crossfade);
    details.set_vhomogeneous(false);
    details.set_interpolate_size(true);

    let dashboard_hint = adw::ActionRow::builder()
        .title("Panneaux et ordre")
        .subtitle("Réglables dans la page « Tableau de bord »")
        .build();
    details.add_named(&group(&[dashboard_hint.upcast_ref()]), Some("dashboard"));

    let image_row = adw::ActionRow::builder()
        .title("Fichier")
        .subtitle("Aucun fichier choisi")
        .build();
    let image_button = gtk::Button::with_label("Choisir une image…");
    image_button.set_valign(gtk::Align::Center);
    image_row.add_suffix(&image_button);
    let image_fit = adw::ComboRow::builder()
        .title("Ajustement")
        .model(&string_list(FITS.map(|(_, label)| label)))
        .build();
    details.add_named(
        &group(&[image_row.upcast_ref(), image_fit.upcast_ref()]),
        Some("image"),
    );

    let video_row = adw::ActionRow::builder()
        .title("Fichier")
        .subtitle("Aucun fichier choisi")
        .build();
    let video_button = gtk::Button::with_label("Choisir une vidéo…");
    video_button.set_valign(gtk::Align::Center);
    video_row.add_suffix(&video_button);
    let video_loop = adw::SwitchRow::builder().title("Lire en boucle").build();
    details.add_named(
        &group(&[video_row.upcast_ref(), video_loop.upcast_ref()]),
        Some("video"),
    );

    let slideshow_summary = adw::ActionRow::builder()
        .title("Photos")
        .subtitle("—")
        .build();
    let slideshow_open = gtk::Button::with_label("Régler le diaporama");
    slideshow_open.set_valign(gtk::Align::Center);
    slideshow_summary.add_suffix(&slideshow_open);
    details.add_named(&group(&[slideshow_summary.upcast_ref()]), Some("slideshow"));

    let clock_seconds = adw::SwitchRow::builder()
        .title("Secondes")
        .subtitle("Affichées en plus petit à côté de l'heure")
        .build();
    let clock_date = adw::SwitchRow::builder()
        .title("Date")
        .subtitle("Jour, mois et année sous l'heure")
        .build();
    details.add_named(
        &group(&[clock_seconds.upcast_ref(), clock_date.upcast_ref()]),
        Some("clock"),
    );

    let now_playing_row = adw::ActionRow::builder()
        .title("Morceau")
        .subtitle("Aucune lecture en cours")
        .build();
    now_playing_row.add_css_class("property");
    let now_playing_player = adw::EntryRow::builder()
        .title("Lecteur préféré (ex. spotify ; vide = celui qui joue)")
        .show_apply_button(true)
        .build();
    details.add_named(
        &group(&[
            now_playing_row.upcast_ref(),
            now_playing_player.upcast_ref(),
        ]),
        Some("now_playing"),
    );

    let color = gtk::ColorDialogButton::new(Some(
        gtk::ColorDialog::builder()
            .with_alpha(false)
            .title("Couleur de l'écran")
            .build(),
    ));
    color.set_valign(gtk::Align::Center);
    let color_row = adw::ActionRow::builder()
        .title("Couleur")
        .subtitle("Choisissez une teinte ou ouvrez le sélecteur")
        .build();
    let swatch_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    swatch_box.set_valign(gtk::Align::Center);
    let swatches: Vec<(String, gtk::Button)> = SWATCHES
        .iter()
        .map(|hex| {
            let button = gtk::Button::new();
            button.add_css_class("swatch");
            button.set_tooltip_text(Some(&format!("#{hex}")));
            accessible(&button, &format!("Couleur #{hex}"));
            let (r, g, b) = (
                u8::from_str_radix(&hex[0..2], 16).unwrap_or(0),
                u8::from_str_radix(&hex[2..4], 16).unwrap_or(0),
                u8::from_str_radix(&hex[4..6], 16).unwrap_or(0),
            );
            let area = gtk::DrawingArea::new();
            area.set_content_width(30);
            area.set_content_height(30);
            area.set_draw_func(move |_, cr, w, h| {
                let radius = f64::from(w.min(h)) / 2.0;
                cr.arc(
                    f64::from(w) / 2.0,
                    f64::from(h) / 2.0,
                    radius,
                    0.0,
                    std::f64::consts::TAU,
                );
                cr.set_source_rgb(
                    f64::from(r) / 255.0,
                    f64::from(g) / 255.0,
                    f64::from(b) / 255.0,
                );
                let _ = cr.fill();
            });
            button.set_child(Some(&area));
            swatch_box.append(&button);
            ((*hex).to_owned(), button)
        })
        .collect();
    let swatch_row = adw::ActionRow::builder().title("Teintes").build();
    swatch_row.add_suffix(&swatch_box);
    color_row.add_suffix(&color);
    details.add_named(
        &group(&[swatch_row.upcast_ref(), color_row.upcast_ref()]),
        Some("color"),
    );

    let msi_hint = adw::ActionRow::builder()
        .title("Écran rendu à son firmware")
        .subtitle(
            "L'animation d'origine reprend ; choisissez un autre contenu pour reprendre la main",
        )
        .build();
    details.add_named(&group(&[msi_hint.upcast_ref()]), Some("msi_animation"));

    let page_content = page(
        "contenu",
        &[
            content_flow.upcast_ref(),
            section("Options", None).upcast_ref(),
            details.upcast_ref(),
        ],
    );

    // ---------------------------------------------------------- diaporama
    let folders_flow = flow(1, 4);
    let add_folder = gtk::Button::new();
    add_folder.add_css_class("add-tile");
    add_folder.add_css_class("flat");
    let add_body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    add_body.set_valign(gtk::Align::Center);
    add_body.set_margin_top(28);
    add_body.set_margin_bottom(28);
    add_body.append(&gtk::Image::from_icon_name("list-add-symbolic"));
    add_body.append(&gtk::Label::new(Some("Ajouter un dossier")));
    add_folder.set_child(Some(&add_body));
    add_folder.set_tooltip_text(Some("Ajouter un dossier de photos au diaporama"));

    let google_icon = gtk::Image::from_icon_name("emblem-photos-symbolic");
    google_icon.add_css_class("feature-icon");
    google_icon.set_valign(gtk::Align::Start);
    let google_title = label("Vos photos Google", &["feature-title"]);
    let google_text = label(
        "Choisissez des photos ou des albums dans Google Photos : ils sont copiés sur ce PC et \
         rejoignent le diaporama. Rien n'est envoyé ailleurs que chez Google.",
        &["section-hint"],
    );
    let google_state = label("—", &["feature-state"]);
    let google_cache = label("—", &["section-hint"]);
    let google_button = gtk::Button::with_label("Choisir des photos Google…");
    google_button.add_css_class("suggested-action");
    google_button.add_css_class("pill");
    let google_clear = gtk::Button::with_label("Vider le cache");
    google_clear.add_css_class("pill");
    // Les deux boutons passent l'un sous l'autre quand la place manque.
    let google_actions = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(10)
        .row_spacing(8)
        .min_children_per_line(1)
        .max_children_per_line(2)
        .halign(gtk::Align::Start)
        .margin_top(6)
        .build();
    flow_append(&google_actions, &google_button);
    flow_append(&google_actions, &google_clear);
    let google_column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    google_column.set_hexpand(true);
    google_column.append(&google_title);
    google_column.append(&google_text);
    google_column.append(&google_state);
    google_column.append(&google_cache);
    google_column.append(&google_actions);
    let google_card = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    google_card.add_css_class("feature-card");
    google_card.append(&google_icon);
    google_card.append(&google_column);

    let google_creds = adw::ExpanderRow::builder()
        .title("Identifiants OAuth")
        .subtitle("Client « application de bureau » créé dans votre Google Cloud Console")
        .build();
    let google_id = adw::EntryRow::builder().title("Identifiant client").build();
    let google_secret = adw::PasswordEntryRow::builder()
        .title("Secret client")
        .build();
    let google_save = gtk::Button::with_label("Enregistrer");
    google_save.set_valign(gtk::Align::Center);
    let google_save_row = adw::ActionRow::builder()
        .title("Enregistrer les identifiants")
        .subtitle("Conservés dans ~/.config/lightning/config.toml")
        .build();
    google_save_row.add_suffix(&google_save);
    google_creds.add_row(&google_id);
    google_creds.add_row(&google_secret);
    google_creds.add_row(&google_save_row);

    let slide_interval = adw::SpinRow::with_range(1.0, 3600.0, 1.0);
    slide_interval.set_title("Durée par photo");
    slide_interval.set_subtitle("En secondes");
    let slide_order = adw::ComboRow::builder()
        .title("Ordre")
        .subtitle("L'ordre chronologique suit la date de prise de vue")
        .model(&string_list(ORDERS.map(|(_, label)| label)))
        .build();
    let slide_fit = adw::ComboRow::builder()
        .title("Ajustement")
        .model(&string_list(SLIDE_FITS.map(|(_, label)| label)))
        .build();
    let slide_fade = adw::SwitchRow::builder()
        .title("Fondu enchaîné")
        .subtitle("Transition douce entre deux photos")
        .build();
    let slide_overlay = adw::SwitchRow::builder()
        .title("Légende")
        .subtitle("Date de la photo et nom de son dossier")
        .build();
    let slide_clock = adw::SwitchRow::builder().title("Horloge").build();

    let page_slideshow = page(
        "diaporama",
        &[
            section(
                "Dossiers",
                Some("Parcourus avec leurs sous-dossiers (JPEG, PNG, WebP)"),
            )
            .upcast_ref(),
            folders_flow.upcast_ref(),
            google_card.upcast_ref(),
            group(&[google_creds.upcast_ref()]).upcast_ref(),
            section("Lecture", None).upcast_ref(),
            group(&[
                slide_interval.upcast_ref(),
                slide_order.upcast_ref(),
                slide_fit.upcast_ref(),
                slide_fade.upcast_ref(),
                slide_overlay.upcast_ref(),
                slide_clock.upcast_ref(),
            ])
            .upcast_ref(),
        ],
    );

    // ---------------------------------------------------- tableau de bord
    let schematic = gtk::Box::new(gtk::Orientation::Vertical, 8);
    schematic.add_css_class("schematic");
    accessible(&schematic, "Schéma de la disposition des panneaux");
    let schematic_clamp = adw::Clamp::builder()
        .maximum_size(560)
        .child(&schematic)
        .build();
    let panel_list = gtk::ListBox::new();
    panel_list.set_selection_mode(gtk::SelectionMode::None);
    panel_list.add_css_class("boxed-list");
    let page_dashboard = page(
        "dashboard",
        &[
            section(
                "Disposition",
                Some("Aperçu de l'ordre des panneaux sur l'écran"),
            )
            .upcast_ref(),
            schematic_clamp.upcast_ref(),
            section(
                "Panneaux",
                Some(
                    "Activez-les, puis glissez-les ou utilisez les flèches ; l'ordre s'applique au sein de chaque rangée",
                ),
            )
            .upcast_ref(),
            panel_list.upcast_ref(),
        ],
    );

    // ----------------------------------------------------------- réglages
    let settings_scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&brightness));
    settings_scale.set_width_request(170);
    settings_scale.set_hexpand(true);
    settings_scale.set_draw_value(true);
    settings_scale.set_digits(0);
    settings_scale.set_value_pos(gtk::PositionType::Right);
    settings_scale.set_valign(gtk::Align::Center);
    accessible(&settings_scale, "Luminosité de l'écran");
    let brightness_row = adw::ActionRow::builder().title("Luminosité").build();
    brightness_row.add_suffix(&settings_scale);
    let rotation = adw::ComboRow::builder()
        .title("Orientation")
        .subtitle("Retourne l'image si l'écran est monté tête en bas")
        .model(&string_list(["Normale", "Retournée (180°)"]))
        .build();
    let fps = adw::SpinRow::with_range(0.5, 30.0, 0.5);
    fps.set_title("Images par seconde");
    fps.set_subtitle("Cadence de rafraîchissement du tableau de bord");
    fps.set_digits(1);
    let lcd_row = info_row("Écran");
    let stats_row = info_row("Performances");
    let on_exit = adw::ComboRow::builder()
        .title("À l'arrêt du service")
        .subtitle("Ce que l'écran affiche une fois le service arrêté")
        .model(&string_list(ON_EXIT.map(|(_, label)| label)))
        .build();
    let http = adw::SwitchRow::builder()
        .title("Aperçu HTTP")
        .subtitle("Page, flux MJPEG, dernière image et état JSON — sans authentification")
        .build();
    let http_addr = adw::EntryRow::builder()
        .title("Adresse d'écoute (IP:port)")
        .show_apply_button(true)
        .build();
    let browser_button = gtk::Button::from_icon_name("web-browser-symbolic");
    browser_button.set_tooltip_text(Some("Ouvrir l'aperçu dans le navigateur"));
    accessible(&browser_button, "Ouvrir l'aperçu dans le navigateur");
    browser_button.set_valign(gtk::Align::Center);
    browser_button.add_css_class("flat");
    http_addr.add_suffix(&browser_button);
    let page_settings = page(
        "reglages",
        &[
            section("Écran", None).upcast_ref(),
            group(&[
                brightness_row.upcast_ref(),
                rotation.upcast_ref(),
                fps.upcast_ref(),
            ])
            .upcast_ref(),
            section("État", None).upcast_ref(),
            group(&[lcd_row.upcast_ref(), stats_row.upcast_ref()]).upcast_ref(),
            section("Arrêt", None).upcast_ref(),
            group(&[on_exit.upcast_ref()]).upcast_ref(),
            section("Aperçu dans un navigateur", None).upcast_ref(),
            group(&[http.upcast_ref(), http_addr.upcast_ref()]).upcast_ref(),
        ],
    );

    // ------------------------------------------------------------ service
    let service_icon = gtk::Image::from_icon_name("system-run-symbolic");
    let service_badge = gtk::Box::new(gtk::Orientation::Vertical, 0);
    service_badge.add_css_class("service-badge");
    service_badge.set_halign(gtk::Align::Center);
    service_badge.append(&service_icon);
    let service_title = gtk::Label::new(Some("—"));
    service_title.add_css_class("service-title");
    let service_hint = gtk::Label::new(None);
    service_hint.add_css_class("section-hint");
    service_hint.set_wrap(true);
    service_hint.set_justify(gtk::Justification::Center);
    let restart_button = gtk::Button::with_label("Redémarrer le service");
    restart_button.add_css_class("pill");
    restart_button.set_halign(gtk::Align::Center);
    let service_head = gtk::Box::new(gtk::Orientation::Vertical, 10);
    service_head.set_margin_top(8);
    service_head.append(&service_badge);
    service_head.append(&service_title);
    service_head.append(&service_hint);
    service_head.append(&restart_button);
    let service_active = adw::SwitchRow::builder()
        .title("Service en marche")
        .subtitle("Arrêter le service libère l'écran")
        .build();
    let service_enabled = adw::SwitchRow::builder()
        .title("Lancer à l'ouverture de session")
        .build();
    let version_row = info_row("Version du démon");
    let page_service = page(
        "service",
        &[
            service_head.upcast_ref(),
            group(&[
                service_active.upcast_ref(),
                service_enabled.upcast_ref(),
                version_row.upcast_ref(),
            ])
            .upcast_ref(),
        ],
    );

    // --------------------------------------------------------- navigation
    let pages = gtk::Stack::new();
    pages.set_transition_type(gtk::StackTransitionType::Crossfade);
    pages.set_transition_duration(180);
    for (name, widget) in [
        ("accueil", &page_home),
        ("contenu", &page_content),
        ("diaporama", &page_slideshow),
        ("dashboard", &page_dashboard),
        ("reglages", &page_settings),
        ("service", &page_service),
    ] {
        pages.add_named(widget, Some(name));
    }
    let content_view = adw::ToolbarView::new();
    // Le titre est déjà en grand dans la page.
    let content_header = adw::HeaderBar::new();
    content_header.set_show_title(false);
    content_view.add_top_bar(&content_header);
    content_view.set_content(Some(&pages));
    let content_page = adw::NavigationPage::builder()
        .title("Accueil")
        .tag("content")
        .child(&content_view)
        .build();

    let nav_list = gtk::ListBox::new();
    nav_list.add_css_class("navigation-sidebar");
    nav_list.add_css_class("nav-list");
    nav_list.set_selection_mode(gtk::SelectionMode::Single);
    accessible(&nav_list, "Pages");
    for (_, title, icon, _) in PAGES {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_margin_top(7);
        row.set_margin_bottom(7);
        row.set_margin_start(6);
        row.set_margin_end(6);
        row.append(&gtk::Image::from_icon_name(icon));
        let text = gtk::Label::new(Some(title));
        text.add_css_class("nav-label");
        text.set_xalign(0.0);
        row.append(&text);
        nav_list.append(&row);
    }
    let sidebar_state = gtk::Label::new(Some("—"));
    sidebar_state.add_css_class("value");
    sidebar_state.set_xalign(0.0);
    let sidebar_mode = gtk::Label::new(Some("—"));
    sidebar_mode.add_css_class("caption");
    sidebar_mode.set_xalign(0.0);
    let sidebar_caption = gtk::Label::new(Some("Écran"));
    sidebar_caption.add_css_class("caption");
    sidebar_caption.set_xalign(0.0);
    let sidebar_status = gtk::Box::new(gtk::Orientation::Vertical, 1);
    sidebar_status.add_css_class("sidebar-status");
    sidebar_status.set_margin_top(8);
    sidebar_status.set_margin_bottom(12);
    sidebar_status.set_margin_start(12);
    sidebar_status.set_margin_end(12);
    sidebar_status.append(&sidebar_caption);
    sidebar_status.append(&sidebar_state);
    sidebar_status.append(&sidebar_mode);
    let nav_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&nav_list)
        .build();
    let sidebar_body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_body.append(&nav_scroll);
    sidebar_body.append(&sidebar_status);

    let menu = gio::Menu::new();
    menu.append(Some("À propos de Lightning"), Some("app.about"));
    menu.append(Some("Quitter"), Some("app.quit"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Menu principal")
        .primary(true)
        .build();
    let brand_icon = gtk::Image::from_icon_name(APP_ID);
    brand_icon.add_css_class("brand-mark");
    let brand_name = gtk::Label::new(Some("Lightning"));
    brand_name.add_css_class("brand-name");
    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    brand.append(&brand_icon);
    brand.append(&brand_name);
    let sidebar_header = adw::HeaderBar::new();
    sidebar_header.set_title_widget(Some(&brand));
    sidebar_header.pack_end(&menu_button);
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&sidebar_header);
    sidebar_view.set_content(Some(&sidebar_body));
    let sidebar_page = adw::NavigationPage::builder()
        .title("Lightning")
        .tag("sidebar")
        .child(&sidebar_view)
        .build();

    let split = adw::NavigationSplitView::new();
    split.set_min_sidebar_width(210.0);
    split.set_max_sidebar_width(250.0);
    split.set_sidebar(Some(&sidebar_page));
    split.set_content(Some(&content_page));

    // ---------------------------------------------------- états et fenêtre
    let offline_button = gtk::Button::with_label("Démarrer le service");
    offline_button.add_css_class("pill");
    offline_button.add_css_class("suggested-action");
    offline_button.set_halign(gtk::Align::Center);
    let offline = adw::StatusPage::builder()
        .icon_name(APP_ID)
        .title("Le service Lightning est arrêté")
        .description("L'écran de la carte graphique n'est pas piloté pour l'instant.")
        .child(&offline_button)
        .build();
    let offline_view = adw::ToolbarView::new();
    offline_view.add_top_bar(&adw::HeaderBar::new());
    offline_view.set_content(Some(&offline));
    let loading = gtk::Spinner::new();
    loading.set_spinning(true);
    loading.set_halign(gtk::Align::Center);
    loading.set_valign(gtk::Align::Center);
    loading.set_size_request(32, 32);
    let loading_view = adw::ToolbarView::new();
    loading_view.add_top_bar(&adw::HeaderBar::new());
    loading_view.set_content(Some(&loading));

    let root = gtk::Stack::new();
    root.set_transition_type(gtk::StackTransitionType::Crossfade);
    root.set_transition_duration(250);
    root.add_named(&loading_view, Some("loading"));
    root.add_named(&offline_view, Some("offline"));
    root.add_named(&split, Some("main"));
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&root));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Lightning")
        .icon_name(APP_ID)
        .default_width(1180)
        .default_height(820)
        .content(&toasts)
        .build();
    window.set_size_request(360, 480);
    // Fenêtre étroite : la navigation passe en une seule colonne.
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        780.0,
        adw::LengthUnit::Sp,
    ));
    narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(narrow);

    Rc::new(Ui {
        window,
        toasts,
        root,
        split,
        nav_list,
        content_page,
        pages,
        offline,
        offline_button,
        sidebar_state,
        sidebar_mode,
        hero_empty,
        hero_picture,
        chip_lcd,
        chip_mode,
        chip_fps,
        chip_render,
        chip_brightness,
        hero_cards,
        brightness,
        brightness_caption,
        content_cards,
        details,
        image_row,
        image_button,
        image_fit,
        video_row,
        video_button,
        video_loop,
        clock_seconds,
        clock_date,
        now_playing_row,
        now_playing_player,
        color,
        swatches,
        slideshow_summary,
        slideshow_open,
        folders_flow,
        shown_folders: RefCell::new(None),
        add_folder,
        slide_interval,
        slide_order,
        slide_fit,
        slide_fade,
        slide_overlay,
        slide_clock,
        google_state,
        google_button,
        google_cache,
        google_clear,
        google_id,
        google_secret,
        google_save,
        google_requested: Cell::new(false),
        google_opened: RefCell::new(String::new()),
        schematic,
        panel_list,
        shown_panels: RefCell::new(None),
        rotation,
        fps,
        lcd_row,
        stats_row,
        on_exit,
        http,
        http_addr,
        browser_button,
        service_badge,
        service_icon,
        service_title,
        service_hint,
        service_active,
        service_enabled,
        restart_button,
        version_row,
        updating: Cell::new(false),
        hold_until: Cell::new(Instant::now()),
        debounce: Cell::new(0),
        force_offline: Cell::new(false),
        state: RefCell::new(DaemonState::default()),
        last_frame: RefCell::new(Vec::new()),
        frame_shown: Cell::new(false),
        thumbs: RefCell::new(HashMap::new()),
        dashboard_thumb: RefCell::new(None),
    })
}
