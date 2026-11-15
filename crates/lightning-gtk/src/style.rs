//! Identité visuelle : feuille de style embarquée, sombre par défaut, qui suit
//! aussi le mode clair du système. L'accent ambre rappelle l'éclair de l'icône
//! et les teintes des panneaux reprennent celles du tableau de bord.

use gtk::gdk;

/// Couleurs propres à chaque thème (noms de couleurs de libadwaita).
fn palette(dark: bool) -> &'static [(&'static str, &'static str)] {
    if dark {
        &[
            ("accent_bg_color", "#f2b441"),
            ("accent_fg_color", "#1c1403"),
            ("accent_color", "#f7c66a"),
            ("window_bg_color", "#0d1017"),
            ("window_fg_color", "#eceff5"),
            ("view_bg_color", "#11151e"),
            ("view_fg_color", "#eceff5"),
            ("headerbar_bg_color", "#0d1017"),
            ("headerbar_backdrop_color", "#0d1017"),
            ("headerbar_fg_color", "#eceff5"),
            ("sidebar_bg_color", "#090c12"),
            ("sidebar_backdrop_color", "#090c12"),
            ("sidebar_fg_color", "#eceff5"),
            ("card_bg_color", "#171c27"),
            ("card_fg_color", "#eceff5"),
            ("popover_bg_color", "#1b2130"),
            ("dialog_bg_color", "#141923"),
        ]
    } else {
        &[
            ("accent_bg_color", "#eaa72a"),
            ("accent_fg_color", "#1c1403"),
            ("accent_color", "#8a5a00"),
            ("window_bg_color", "#f3f4f8"),
            ("window_fg_color", "#161a22"),
            ("view_bg_color", "#ffffff"),
            ("view_fg_color", "#161a22"),
            ("headerbar_bg_color", "#f3f4f8"),
            ("headerbar_backdrop_color", "#f3f4f8"),
            ("headerbar_fg_color", "#161a22"),
            ("sidebar_bg_color", "#e8eaf0"),
            ("sidebar_backdrop_color", "#e8eaf0"),
            ("sidebar_fg_color", "#161a22"),
            ("card_bg_color", "#ffffff"),
            ("card_fg_color", "#161a22"),
            ("popover_bg_color", "#ffffff"),
            ("dialog_bg_color", "#fafbfd"),
        ]
    }
}

/// Feuille de couleurs du thème : couleurs nommées (libadwaita ≤ 1.5) et
/// variables CSS (libadwaita ≥ 1.6, GTK ≥ 4.16).
pub fn theme_css(dark: bool, css_variables: bool) -> String {
    let mut css = String::new();
    for (name, value) in palette(dark) {
        css.push_str(&format!("@define-color {name} {value};\n"));
    }
    if css_variables {
        css.push_str(":root {\n");
        for (name, value) in palette(dark) {
            css.push_str(&format!("  --{}: {value};\n", name.replace('_', "-")));
        }
        css.push_str("}\n");
    }
    css
}

/// Styles communs aux deux thèmes.
pub const COMMON_CSS: &str = r#"
/* ---------------------------------------------------------------- pages */
.page-title { font-size: 1.85em; font-weight: 800; letter-spacing: -0.02em; }
.page-subtitle { opacity: 0.62; }
.section-title { font-size: 1.05em; font-weight: 800; }
.section-hint { opacity: 0.6; font-size: 0.92em; }
.numeric { font-feature-settings: "tnum"; }

/* -------------------------------------------------------------- sidebar */
.brand-mark { -gtk-icon-size: 28px; }
.brand-name { font-weight: 800; font-size: 1.1em; letter-spacing: -0.01em; }
.nav-list row { border-radius: 10px; margin: 1px 0; }
.nav-list row .nav-label { font-weight: 600; }
.nav-list row:selected { background: alpha(@accent_bg_color, 0.16); }
.nav-list row:selected image, .nav-list row:selected .nav-label { color: @accent_color; }
.sidebar-status { padding: 10px 12px; border-radius: 12px; background: alpha(currentColor, 0.06); }
.sidebar-status .caption { opacity: 0.6; font-size: 0.82em; }
.sidebar-status .value { font-weight: 700; }

/* ----------------------------------------------------------------- héros */
.hero-bezel {
  background: #05070b;
  border-radius: 24px;
  padding: 10px;
  box-shadow: 0 0 0 1px alpha(white, 0.08),
              0 22px 70px alpha(@accent_bg_color, 0.16),
              0 10px 28px alpha(black, 0.45);
}
.hero-screen { border-radius: 15px; background: #090b10; }
.hero-empty { color: alpha(white, 0.55); }
.hero-empty image { -gtk-icon-size: 48px; opacity: 0.6; }

.chip {
  padding: 6px 13px;
  border-radius: 999px;
  background: alpha(currentColor, 0.07);
  font-weight: 600;
  font-size: 0.9em;
}
.chip .dot { min-width: 8px; min-height: 8px; border-radius: 8px; background: alpha(currentColor, 0.35); }
.chip.ok { background: alpha(#3fca6b, 0.16); }
.chip.ok .dot { background: #3fca6b; }
.chip.busy { background: alpha(#f2b441, 0.18); }
.chip.busy .dot { background: #f2b441; }
.chip.bad { background: alpha(#f25454, 0.18); }
.chip.bad .dot { background: #f25454; }
.chip image { opacity: 0.7; }

.quick-panel { border-radius: 18px; padding: 14px 18px; background: @card_bg_color; box-shadow: 0 0 0 1px alpha(currentColor, 0.07); }

/* ------------------------------------------------------ cartes de contenu */
.mode-card {
  padding: 8px;
  border-radius: 18px;
  background: @card_bg_color;
  box-shadow: 0 0 0 1px alpha(currentColor, 0.08);
  transition: box-shadow 200ms ease, background 200ms ease;
}
.mode-card:hover { box-shadow: 0 0 0 1px alpha(currentColor, 0.2), 0 8px 22px alpha(black, 0.22); }
.mode-card:checked {
  background: alpha(@accent_bg_color, 0.10);
  box-shadow: 0 0 0 2px @accent_bg_color, 0 10px 28px alpha(@accent_bg_color, 0.20);
}
.mode-card:focus-visible { outline: 2px solid alpha(@accent_color, 0.8); outline-offset: 2px; }
.mode-card .art {
  border-radius: 12px;
  background: linear-gradient(135deg, #1d2536, #0b0e15);
  color: #f7c66a;
}
.mode-card .art image { -gtk-icon-size: 34px; }
.mode-card.large .art image { -gtk-icon-size: 46px; }
.mode-card .title { font-weight: 700; }
.mode-card .subtitle { font-size: 0.84em; opacity: 0.62; }
.mode-card .check { color: @accent_color; opacity: 0; transition: opacity 200ms ease; }
.mode-card:checked .check { opacity: 1; }

.swatch { min-width: 30px; min-height: 30px; border-radius: 999px; padding: 0; box-shadow: 0 0 0 1px alpha(currentColor, 0.18); }
.swatch:hover { box-shadow: 0 0 0 2px alpha(currentColor, 0.45); }

/* -------------------------------------------------------------- diaporama */
.folder-tile { padding: 8px; border-radius: 18px; background: @card_bg_color; box-shadow: 0 0 0 1px alpha(currentColor, 0.08); }
.folder-tile .cover { border-radius: 12px; background: linear-gradient(135deg, #1d2536, #0b0e15); color: alpha(white, 0.5); }
.folder-tile .cover image { -gtk-icon-size: 36px; }
.folder-tile .title { font-weight: 700; }
.folder-tile .subtitle { font-size: 0.84em; opacity: 0.62; }
.add-tile {
  border-radius: 18px;
  border: 2px dashed alpha(currentColor, 0.22);
  background: transparent;
  box-shadow: none;
  transition: border-color 200ms ease, background 200ms ease;
}
.add-tile:hover { border-color: @accent_color; background: alpha(@accent_bg_color, 0.07); }
.add-tile image { -gtk-icon-size: 28px; opacity: 0.8; }

.feature-card {
  border-radius: 20px;
  padding: 22px;
  background: linear-gradient(120deg, alpha(@accent_bg_color, 0.22), alpha(#b289ff, 0.16) 55%, alpha(#54a6ff, 0.16));
  box-shadow: 0 0 0 1px alpha(currentColor, 0.08);
}
.feature-card .feature-title { font-size: 1.25em; font-weight: 800; }
.feature-card .feature-icon { -gtk-icon-size: 40px; color: @accent_color; }
.feature-card .feature-state { font-weight: 600; }

/* -------------------------------------------------------- tableau de bord */
.schematic { background: #090b10; border-radius: 16px; padding: 12px; box-shadow: 0 0 0 1px alpha(white, 0.06); }
.schematic .tile {
  border-radius: 9px;
  padding: 16px 10px;
  background: #141923;
  color: #eceff5;
  font-weight: 700;
  border-left: 4px solid #8c96a8;
}
.schematic .tile.small { padding: 11px 10px; }
.schematic .empty { color: alpha(white, 0.55); padding: 26px; }
.schematic .tile.tile-gpu { border-left-color: #76e05b; }
.schematic .tile.tile-cpu { border-left-color: #ff8a4c; }
.schematic .tile.tile-memory { border-left-color: #54a6ff; }
.schematic .tile.tile-disks { border-left-color: #b289ff; }
.schematic .tile.tile-network { border-left-color: #2dd4bf; }
.panel-dot { min-width: 10px; min-height: 10px; border-radius: 10px; }
.dot-gpu { background: #76e05b; }
.dot-cpu { background: #ff8a4c; }
.dot-memory { background: #54a6ff; }
.dot-disks { background: #b289ff; }
.dot-network { background: #2dd4bf; }

/* ----------------------------------------------------------------- service */
.service-badge { border-radius: 999px; padding: 22px; background: alpha(currentColor, 0.07); }
.service-badge image { -gtk-icon-size: 44px; }
.service-badge.ok { background: alpha(#3fca6b, 0.16); color: #3fca6b; }
.service-badge.bad { background: alpha(#f25454, 0.16); color: #f25454; }
.service-title { font-size: 1.35em; font-weight: 800; }
"#;

/// Installe les feuilles de style et les tient à jour quand le thème change.
pub fn install() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let css_variables = gtk::major_version() > 4 || gtk::minor_version() >= 16;
    let common = gtk::CssProvider::new();
    common.load_from_string(COMMON_CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &common,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
    let theme = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &theme,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let manager = adw::StyleManager::default();
    theme.load_from_string(&theme_css(manager.is_dark(), css_variables));
    manager.connect_dark_notify(move |manager| {
        theme.load_from_string(&theme_css(manager.is_dark(), css_variables));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(hex: &str) -> f64 {
        let c = |i: usize| {
            let v = f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap()) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * c(1) + 0.7152 * c(3) + 0.0722 * c(5)
    }

    fn contrast(a: &str, b: &str) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    fn color(dark: bool, name: &str) -> &'static str {
        palette(dark).iter().find(|(n, _)| *n == name).unwrap().1
    }

    #[test]
    fn both_palettes_define_the_same_colors() {
        let names = |dark| palette(dark).iter().map(|(n, _)| *n).collect::<Vec<_>>();
        assert_eq!(names(true), names(false));
        for dark in [true, false] {
            for (name, value) in palette(dark) {
                assert!(
                    value.starts_with('#') && value.len() == 7,
                    "{name} = {value}"
                );
            }
        }
    }

    #[test]
    fn text_contrast_meets_wcag_aa() {
        for dark in [true, false] {
            for (fg, bg) in [
                ("window_fg_color", "window_bg_color"),
                ("view_fg_color", "view_bg_color"),
                ("card_fg_color", "card_bg_color"),
                ("sidebar_fg_color", "sidebar_bg_color"),
                ("accent_fg_color", "accent_bg_color"),
                ("accent_color", "window_bg_color"),
                ("accent_color", "card_bg_color"),
            ] {
                let ratio = contrast(color(dark, fg), color(dark, bg));
                assert!(ratio >= 4.5, "{fg} sur {bg} (sombre={dark}) : {ratio:.2}");
            }
        }
    }

    #[test]
    fn theme_css_declares_named_colors_and_optionally_variables() {
        let legacy = theme_css(true, false);
        assert!(legacy.contains("@define-color accent_bg_color #f2b441;"));
        assert!(!legacy.contains(":root"));
        let modern = theme_css(false, true);
        assert!(modern.contains("--accent-bg-color: #eaa72a;"));
        assert!(modern.contains("--window-bg-color: #f3f4f8;"));
    }
}
