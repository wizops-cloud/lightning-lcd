//! Rendu du tableau de bord dans une image 1280×800 (tiny-skia + fontdue).

use std::collections::{HashMap, VecDeque};

use fontdue::{Font, FontSettings};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect, Stroke,
    Transform,
};

mod screens;

pub use screens::{
    Artwork, ClockConfig, Track, day_date, demo_artwork, demo_track, fmt_track_time, fr_long_date,
    long_date, wrap_text,
};

use lightning_i18n::{Lang, lang, tr, trf};

use crate::config::DashboardConfig;
use crate::metrics::{History, Snapshot};
use crate::rotate;

/// Largeur de l'image produite.
pub const WIDTH: u32 = 1280;
/// Hauteur de l'image produite.
pub const HEIGHT: u32 = 800;

/// Police embarquée (JetBrains Mono Regular, OFL 1.1).
pub static FONT_REGULAR: &[u8] = include_bytes!("../assets/JetBrainsMono-Regular.otf");
static FONT_BOLD: &[u8] = include_bytes!("../assets/JetBrainsMono-Bold.otf");

type Rgb = (u8, u8, u8);

const BG: Rgb = (9, 11, 16);
const PANEL: Rgb = (20, 25, 35);
const TRACK: Rgb = (40, 48, 64);
const TEXT: Rgb = (236, 240, 247);
const MUTED: Rgb = (140, 150, 168);
const GPU: Rgb = (118, 224, 91);
const CPU: Rgb = (255, 138, 76);
const MEM: Rgb = (84, 166, 255);
const DISK: Rgb = (178, 137, 255);
const NET_RX: Rgb = (45, 212, 191);
const NET_TX: Rgb = (250, 204, 21);
const HOT: Rgb = (248, 84, 84);
const WARM: Rgb = (250, 190, 40);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Weight {
    Regular,
    Bold,
}

#[derive(Clone, Copy)]
struct Area {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Area {
    fn inset(self, d: f32) -> Area {
        Area {
            x: self.x + d,
            y: self.y + d,
            w: self.w - 2.0 * d,
            h: self.h - 2.0 * d,
        }
    }

    fn right(self) -> f32 {
        self.x + self.w
    }

    fn bottom(self) -> f32 {
        self.y + self.h
    }
}

struct Glyph {
    width: usize,
    height: usize,
    xmin: i32,
    ymin: i32,
    advance: f32,
    coverage: Vec<u8>,
}

/// Moteur de rendu ; conserve les polices et un cache de glyphes entre les images.
pub struct Renderer {
    regular: Font,
    bold: Font,
    glyphs: HashMap<(char, u32, Weight), Glyph>,
    /// Canevas courant (paysage, ou portrait pour 90 et 270°).
    pixmap: Pixmap,
    /// Canevas de l'autre orientation, gardé pour la prochaine image.
    spare: Option<Pixmap>,
    /// Image tournée d'un quart de tour.
    turned: Vec<u8>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        let load = |bytes: &[u8]| {
            Font::from_bytes(bytes, FontSettings::default()).expect("police embarquée valide")
        };
        Self {
            regular: load(FONT_REGULAR),
            bold: load(FONT_BOLD),
            glyphs: HashMap::new(),
            pixmap: Pixmap::new(WIDTH, HEIGHT).expect("dimensions non nulles"),
            spare: None,
            turned: Vec::new(),
        }
    }

    /// Dessine le tableau de bord et renvoie l'image en RGBA (4 octets par pixel,
    /// opaque), de taille [`WIDTH`] × [`HEIGHT`].
    ///
    /// À 90 et 270°, la disposition est en portrait (800 × 1280) puis tournée.
    pub fn render(&mut self, s: &Snapshot, h: &History, cfg: &DashboardConfig) -> &[u8] {
        self.begin(cfg.rotation);
        let portrait = rotate::is_portrait(cfg.rotation);
        let (cw, ch) = (self.w(), self.h());
        let top_y = if portrait {
            self.header_portrait(s)
        } else {
            self.header(s);
            76.0
        };

        // L'ordre de la configuration fixe l'ordre des panneaux dans chaque rangée.
        let in_row = |row: &[&'static str]| -> Vec<&'static str> {
            let mut names: Vec<&'static str> = Vec::new();
            for w in &cfg.widgets {
                if let Some(name) = row.iter().find(|n| n.eq_ignore_ascii_case(w))
                    && !names.contains(name)
                {
                    names.push(name);
                }
            }
            names
        };
        let top = in_row(&["gpu", "cpu"]);
        let bottom = in_row(&["memory", "disks", "network"]);
        let panels = if portrait {
            portrait_layout(&top, &bottom, cw as f32, ch as f32, top_y)
        } else {
            landscape_layout(&top, &bottom, cw as f32, ch as f32, top_y)
        };
        for (name, area) in panels {
            self.panel(area);
            let inner = area.inset(20.0);
            match name.as_str() {
                "gpu" => self.gpu_panel(inner, s, h),
                "cpu" => self.cpu_panel(inner, s, h),
                "memory" => self.memory_panel(inner, s, h),
                "disks" => self.disks_panel(inner, s),
                _ => self.network_panel(inner, s, h),
            }
        }
        self.finish(cfg.rotation)
    }

    /// Largeur du canevas courant.
    fn w(&self) -> u32 {
        self.pixmap.width()
    }

    /// Hauteur du canevas courant.
    fn h(&self) -> u32 {
        self.pixmap.height()
    }

    /// Prépare un canevas vide dans l'orientation demandée.
    fn begin(&mut self, rotation: u16) {
        let (w, h) = rotate::canvas(rotation);
        if (self.pixmap.width(), self.pixmap.height()) != (w, h) {
            let other = self
                .spare
                .take()
                .unwrap_or_else(|| Pixmap::new(w, h).expect("dimensions non nulles"));
            self.spare = Some(std::mem::replace(&mut self.pixmap, other));
        }
        self.pixmap.fill(color(BG, 255));
    }

    /// Applique la rotation et renvoie l'image de l'écran (1280 × 800, RGBA).
    fn finish(&mut self, rotation: u16) -> &[u8] {
        match rotation {
            180 => {
                self.pixmap.pixels_mut().reverse();
                self.pixmap.data()
            }
            90 | 270 => {
                self.turned =
                    rotate::quarter_turn(self.pixmap.data(), self.w(), self.h(), rotation == 90);
                &self.turned
            }
            _ => self.pixmap.data(),
        }
    }

    // ---------------------------------------------------------------- panneaux

    /// En-tête sur deux lignes (portrait) ; renvoie le haut de la zone des panneaux.
    fn header_portrait(&mut self, s: &Snapshot) -> f32 {
        let host = if s.hostname.is_empty() {
            "PC"
        } else {
            &s.hostname
        };
        let right = self.w() as f32 - 24.0;
        let clock = s.time.format("%H:%M:%S").to_string();
        let clock_w = self.measure(44.0, Weight::Bold, &clock);
        self.text(right - clock_w, 56.0, 44.0, Weight::Bold, TEXT, &clock);
        let host_line = screens::wrap_text(host, right - clock_w - 48.0, 1, |t| {
            self.measure(34.0, Weight::Bold, t)
        });
        if let Some(line) = host_line.first() {
            self.text(24.0, 52.0, 34.0, Weight::Bold, TEXT, line);
        }
        let up = trf(
            "allumé depuis {uptime}",
            &[("uptime", &fmt_uptime(s.uptime.as_secs()))],
        );
        self.text(24.0, 86.0, 22.0, Weight::Regular, MUTED, &up);
        let date = s.time.format(date_format(lang())).to_string();
        let date_w = self.measure(22.0, Weight::Regular, &date);
        self.text(right - date_w, 86.0, 22.0, Weight::Regular, MUTED, &date);
        104.0
    }
}

/// Paysage : rangée du haut (carte graphique, processeur), rangée du bas.
fn landscape_layout(
    top: &[&str],
    bottom: &[&str],
    width: f32,
    height: f32,
    top_y: f32,
) -> Vec<(String, Area)> {
    {
        let (margin, gap) = (16.0, 16.0);
        let avail = height - top_y - margin;
        let (top_h, bottom_h) = match (top.is_empty(), bottom.is_empty()) {
            (false, false) => (avail - gap - 236.0, 236.0),
            (false, true) => (avail, 0.0),
            (true, _) => (0.0, avail),
        };
        let row = |names: &[&str], y: f32, height: f32| -> Vec<(String, Area)> {
            let n = names.len() as f32;
            let w = (width - 2.0 * margin - gap * (n - 1.0)) / n;
            names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let x = margin + i as f32 * (w + gap);
                    ((*name).to_owned(), Area { x, y, w, h: height })
                })
                .collect()
        };
        let mut panels = Vec::new();
        if !top.is_empty() {
            panels.extend(row(top, top_y, top_h));
        }
        if !bottom.is_empty() {
            let y = if top.is_empty() {
                top_y
            } else {
                top_y + top_h + gap
            };
            panels.extend(row(bottom, y, bottom_h));
        }
        panels
    }
}

/// Portrait : carte graphique et processeur en pleine largeur, l'un sous
/// l'autre, puis les autres panneaux deux par deux (le dernier seul s'étend).
fn portrait_layout(
    top: &[&str],
    bottom: &[&str],
    width: f32,
    height: f32,
    top_y: f32,
) -> Vec<(String, Area)> {
    let (margin, gap) = (16.0, 16.0);
    let full = width - 2.0 * margin;
    let rows_bottom = bottom.len().div_ceil(2);
    let rows = top.len() + rows_bottom;
    if rows == 0 {
        return Vec::new();
    }
    let avail = height - top_y - margin - gap * (rows as f32 - 1.0);
    let (top_h, bottom_h) = match (top.is_empty(), rows_bottom) {
        (true, n) => (0.0, avail / n as f32),
        (false, 0) => (avail / top.len() as f32, 0.0),
        (false, n) => {
            let bottom_h = 190.0_f32.min(avail / rows as f32);
            ((avail - bottom_h * n as f32) / top.len() as f32, bottom_h)
        }
    };
    let mut panels = Vec::new();
    let mut y = top_y;
    for name in top {
        panels.push((
            (*name).to_owned(),
            Area {
                x: margin,
                y,
                w: full,
                h: top_h,
            },
        ));
        y += top_h + gap;
    }
    for pair in bottom.chunks(2) {
        let w = (full - gap * (pair.len() as f32 - 1.0)) / pair.len() as f32;
        for (i, name) in pair.iter().enumerate() {
            panels.push((
                (*name).to_owned(),
                Area {
                    x: margin + i as f32 * (w + gap),
                    y,
                    w,
                    h: bottom_h,
                },
            ));
        }
        y += bottom_h + gap;
    }
    panels
}

impl Renderer {
    fn header(&mut self, s: &Snapshot) {
        let host = if s.hostname.is_empty() {
            "PC"
        } else {
            &s.hostname
        };
        self.text(24.0, 52.0, 34.0, Weight::Bold, TEXT, host);
        let host_w = self.measure(34.0, Weight::Bold, host);
        let up = trf(
            "allumé depuis {uptime}",
            &[("uptime", &fmt_uptime(s.uptime.as_secs()))],
        );
        self.text(
            24.0 + host_w + 24.0,
            50.0,
            22.0,
            Weight::Regular,
            MUTED,
            &up,
        );

        let clock = s.time.format("%H:%M:%S").to_string();
        let clock_w = self.measure(44.0, Weight::Bold, &clock);
        let right = self.w() as f32 - 24.0;
        self.text(right - clock_w, 56.0, 44.0, Weight::Bold, TEXT, &clock);
        let date = s.time.format(date_format(lang())).to_string();
        let date_w = self.measure(22.0, Weight::Regular, &date);
        self.text(
            right - clock_w - 24.0 - date_w,
            50.0,
            22.0,
            Weight::Regular,
            MUTED,
            &date,
        );
    }

    fn title(&mut self, a: Area, accent: Rgb, label: &str, detail: &str) -> f32 {
        self.rounded(
            Area {
                x: a.x,
                y: a.y + 2.0,
                w: 8.0,
                h: 30.0,
            },
            4.0,
            accent,
            255,
        );
        self.text(a.x + 20.0, a.y + 28.0, 30.0, Weight::Bold, TEXT, label);
        let w = self.measure(30.0, Weight::Bold, label);
        self.text(
            a.x + 20.0 + w + 16.0,
            a.y + 27.0,
            22.0,
            Weight::Regular,
            MUTED,
            detail,
        );
        a.y + 52.0
    }

    fn gpu_panel(&mut self, a: Area, s: &Snapshot, h: &History) {
        let Some(gpu) = &s.gpu else {
            let y = self.title(a, GPU, "GPU", "");
            self.text(
                a.x,
                y + 40.0,
                26.0,
                Weight::Regular,
                MUTED,
                tr("NVML indisponible"),
            );
            return;
        };
        let y = self.title(a, GPU, "GPU", &gpu.name);
        let r = 76.0;
        let cy = y + r + 14.0;
        let usage = gpu.usage.unwrap_or(0.0);
        self.gauge(
            a.x + r + 6.0,
            cy,
            r,
            usage / 100.0,
            GPU,
            &format!("{usage:.0}"),
            "%",
            tr("charge"),
        );
        let temp = gpu.temp_c.unwrap_or(0.0);
        self.gauge(
            a.x + 3.0 * r + 40.0,
            cy,
            r,
            temp / 100.0,
            temp_color(temp, 70.0, 85.0),
            &format!("{temp:.0}"),
            "°C",
            tr("temp."),
        );

        // Colonne de valeurs à droite des jauges.
        let col = a.x + 4.0 * r + 70.0;
        let col_w = a.right() - col;
        let mut ly = y + 20.0;
        let power = gpu.power_w.map_or("—".into(), |p| format!("{p:.0} W"));
        self.stat(col, ly, col_w, tr("puissance"), &power);
        ly += 44.0;
        let fan = match gpu.fans.as_slice() {
            [] => "—".to_owned(),
            fans => fans
                .iter()
                .map(|f| format!("{f} %"))
                .collect::<Vec<_>>()
                .join(" "),
        };
        self.stat(col, ly, col_w, tr("ventilo"), &fan);
        ly += 44.0;
        let clock = gpu.clock_mhz.map_or("—".into(), |c| format!("{c} MHz"));
        self.stat(col, ly, col_w, tr("fréq."), &clock);
        ly += 44.0;
        let mem_clock = gpu.mem_clock_mhz.map_or("—".into(), |c| format!("{c} MHz"));
        self.stat(col, ly, col_w, tr("mém."), &mem_clock);

        // VRAM
        let by = cy + r + 40.0;
        let frac = ratio(gpu.mem_used, gpu.mem_total);
        let label = format!("{} / {}", fmt_bytes(gpu.mem_used), fmt_bytes(gpu.mem_total));
        self.bar(
            Area {
                x: a.x,
                y: by,
                w: a.w,
                h: 22.0,
            },
            frac,
            GPU,
            "VRAM",
            &label,
        );

        let sy = by + 38.0;
        let spark = Area {
            x: a.x,
            y: sy,
            w: a.w,
            h: a.bottom() - sy,
        };
        self.sparkline(spark, &h.gpu_usage, Some(100.0), GPU, tr("charge"));
    }

    fn cpu_panel(&mut self, a: Area, s: &Snapshot, h: &History) {
        let cpu = &s.cpu;
        let y = self.title(a, CPU, "CPU", &cpu.model);
        let r = 76.0;
        let cy = y + r + 14.0;
        self.gauge(
            a.x + r + 6.0,
            cy,
            r,
            cpu.usage / 100.0,
            CPU,
            &format!("{:.0}", cpu.usage),
            "%",
            tr("charge"),
        );
        let temp = cpu.temp_c.unwrap_or(0.0);
        let temp_text = cpu.temp_c.map_or("—".into(), |t| format!("{t:.0}"));
        self.gauge(
            a.x + 3.0 * r + 40.0,
            cy,
            r,
            temp / 100.0,
            temp_color(temp, 75.0, 90.0),
            &temp_text,
            "°C",
            tr("temp."),
        );

        let col = a.x + 4.0 * r + 70.0;
        let col_w = a.right() - col;
        let ghz = |mhz: Option<f32>| {
            mhz.map_or("—".into(), |f| {
                format!("{} GHz", lightning_i18n::decimal(f64::from(f) / 1000.0, 2))
            })
        };
        self.stat(col, y + 20.0, col_w, tr("moy."), &ghz(cpu.freq_mhz));
        self.stat(col, y + 64.0, col_w, "max", &ghz(cpu.freq_max_mhz));
        self.stat(
            col,
            y + 108.0,
            col_w,
            "threads",
            &cpu.per_core.len().to_string(),
        );
        let busy = cpu.per_core.iter().filter(|u| **u > 50.0).count();
        self.stat(col, y + 152.0, col_w, "> 50 %", &busy.to_string());

        // Barres par cœur logique.
        let by = cy + r + 18.0;
        let bars = Area {
            x: a.x,
            y: by,
            w: a.w,
            h: 46.0,
        };
        self.core_bars(bars, &cpu.per_core);

        let sy = by + 46.0 + 12.0;
        let spark = Area {
            x: a.x,
            y: sy,
            w: a.w,
            h: a.bottom() - sy,
        };
        self.sparkline(spark, &h.cpu_usage, Some(100.0), CPU, tr("charge"));
    }

    fn memory_panel(&mut self, a: Area, s: &Snapshot, h: &History) {
        let m = &s.memory;
        let y = self.title(a, MEM, "RAM", &fmt_bytes(m.total));
        let frac = ratio(m.used, m.total);
        self.text(
            a.x,
            y + 40.0,
            50.0,
            Weight::Bold,
            TEXT,
            &format!("{:.0} %", frac * 100.0),
        );
        // Panneau étroit (portrait) : forme courte, ou rien plutôt qu'un chevauchement.
        let pct_w = self.measure(50.0, Weight::Bold, &format!("{:.0} %", frac * 100.0));
        let long = trf("{size} utilisés", &[("size", &fmt_bytes(m.used))]);
        let short = fmt_bytes(m.used);
        for used in [long, short] {
            let w = self.measure(22.0, Weight::Regular, &used);
            if pct_w + 16.0 + w <= a.w {
                self.text(a.right() - w, y + 36.0, 22.0, Weight::Regular, MUTED, &used);
                break;
            }
        }
        self.bar(
            Area {
                x: a.x,
                y: y + 58.0,
                w: a.w,
                h: 18.0,
            },
            frac,
            MEM,
            "",
            "",
        );
        let swap = if m.swap_total > 0 {
            trf(
                "swap {used} / {total}",
                &[
                    ("used", &fmt_bytes(m.swap_used)),
                    ("total", &fmt_bytes(m.swap_total)),
                ],
            )
        } else {
            tr("pas de swap").to_owned()
        };
        self.text(a.x, y + 104.0, 20.0, Weight::Regular, MUTED, &swap);
        let sy = y + 114.0;
        if a.bottom() - sy > 24.0 {
            let spark = Area {
                x: a.x,
                y: sy,
                w: a.w,
                h: a.bottom() - sy,
            };
            self.sparkline(spark, &h.mem_usage, Some(100.0), MEM, "");
        }
    }

    fn disks_panel(&mut self, a: Area, s: &Snapshot) {
        let mut y = self.title(a, DISK, tr("Disques"), "");
        if s.disks.is_empty() {
            self.text(
                a.x,
                y + 30.0,
                22.0,
                Weight::Regular,
                MUTED,
                tr("aucun point de montage"),
            );
        }
        // Pas resserré quand la place manque (portrait), dans la limite de la lisibilité.
        let room = a.bottom() + 4.0 - y - 56.0;
        let step = if s.disks.len() > 1 {
            (room / (s.disks.len() - 1) as f32).clamp(46.0, 64.0)
        } else {
            64.0
        };
        for d in &s.disks {
            if y + 56.0 > a.bottom() + 4.0 {
                break;
            }
            let frac = ratio(d.used, d.total);
            let label = format!("{} / {}", fmt_bytes(d.used), fmt_bytes(d.total));
            let accent = if frac > 0.9 { HOT } else { DISK };
            self.bar(
                Area {
                    x: a.x,
                    y: y + 30.0,
                    w: a.w,
                    h: 18.0,
                },
                frac,
                accent,
                &d.mount,
                &label,
            );
            y += step;
        }
    }

    fn network_panel(&mut self, a: Area, s: &Snapshot, h: &History) {
        let n = &s.network;
        let y = self.title(a, NET_RX, tr("Réseau"), &n.interface);
        let half = (a.w - 16.0) / 2.0;
        for (i, (label, value, accent, hist)) in [
            (tr("↓ reçu"), n.rx_bps, NET_RX, &h.net_rx),
            (tr("↑ émis"), n.tx_bps, NET_TX, &h.net_tx),
        ]
        .into_iter()
        .enumerate()
        {
            let x = a.x + i as f32 * (half + 16.0);
            self.text(x, y + 14.0, 20.0, Weight::Regular, MUTED, label);
            self.text(x, y + 52.0, 32.0, Weight::Bold, accent, &fmt_rate(value));
            let sy = y + 64.0;
            let spark = Area {
                x,
                y: sy,
                w: half,
                h: a.bottom() - sy,
            };
            self.sparkline(spark, hist, None, accent, "");
        }
    }

    // ------------------------------------------------------------- primitives

    fn panel(&mut self, a: Area) {
        self.rounded(a, 18.0, PANEL, 255);
    }

    fn stat(&mut self, x: f32, y: f32, w: f32, label: &str, value: &str) {
        self.text(x, y, 18.0, Weight::Regular, MUTED, label);
        let vw = self.measure(24.0, Weight::Bold, value);
        self.text(x + w - vw, y + 2.0, 24.0, Weight::Bold, TEXT, value);
    }

    /// Jauge circulaire sur 270° ; `frac` entre 0 et 1.
    #[allow(clippy::too_many_arguments)]
    fn gauge(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        frac: f32,
        accent: Rgb,
        value: &str,
        unit: &str,
        label: &str,
    ) {
        let frac = if frac.is_finite() {
            frac.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (start, sweep) = (135.0_f32, 270.0_f32);
        self.arc(cx, cy, r, start, sweep, 14.0, TRACK);
        if frac > 0.004 {
            self.arc(cx, cy, r, start, sweep * frac, 14.0, accent);
        }
        let vw = self.measure(46.0, Weight::Bold, value);
        let uw = self.measure(20.0, Weight::Regular, unit);
        let x = cx - (vw + 4.0 + uw) / 2.0;
        self.text(x, cy + 14.0, 46.0, Weight::Bold, TEXT, value);
        self.text(x + vw + 4.0, cy + 14.0, 20.0, Weight::Regular, MUTED, unit);
        let lw = self.measure(18.0, Weight::Regular, label);
        self.text(
            cx - lw / 2.0,
            cy + r * 0.72 + 6.0,
            18.0,
            Weight::Regular,
            MUTED,
            label,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn arc(&mut self, cx: f32, cy: f32, r: f32, start: f32, sweep: f32, width: f32, c: Rgb) {
        let steps = (sweep.abs() / 3.0).ceil().max(1.0) as usize;
        let mut pb = PathBuilder::new();
        for i in 0..=steps {
            let angle = (start + sweep * i as f32 / steps as f32).to_radians();
            let (x, y) = (cx + r * angle.cos(), cy + r * angle.sin());
            if i == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        if let Some(path) = pb.finish() {
            let stroke = Stroke {
                width,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Default::default()
            };
            let paint = paint(c, 255);
            self.pixmap
                .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    /// Barre horizontale avec libellé à gauche et valeur à droite au-dessus.
    fn bar(&mut self, a: Area, frac: f32, accent: Rgb, label: &str, value: &str) {
        if !label.is_empty() {
            self.text(a.x, a.y - 8.0, 20.0, Weight::Regular, MUTED, label);
        }
        if !value.is_empty() {
            let w = self.measure(20.0, Weight::Bold, value);
            self.text(a.right() - w, a.y - 8.0, 20.0, Weight::Bold, TEXT, value);
        }
        let radius = a.h / 2.0;
        self.rounded(a, radius, TRACK, 255);
        let frac = if frac.is_finite() {
            frac.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if frac > 0.0 {
            let w = (a.w * frac).max(a.h);
            self.rounded(Area { w, ..a }, radius, accent, 255);
        }
    }

    fn core_bars(&mut self, a: Area, cores: &[f32]) {
        if cores.is_empty() {
            return;
        }
        let n = cores.len() as f32;
        let gap = if cores.len() > 48 { 1.0 } else { 3.0 };
        let w = ((a.w - gap * (n - 1.0)) / n).max(1.0);
        for (i, usage) in cores.iter().enumerate() {
            let x = a.x + i as f32 * (w + gap);
            self.rect(
                Area {
                    x,
                    y: a.y,
                    w,
                    h: a.h,
                },
                TRACK,
                255,
            );
            let frac = (usage / 100.0).clamp(0.0, 1.0);
            let h = (a.h * frac).max(2.0);
            let c = temp_color(*usage, 70.0, 90.0);
            let c = if *usage < 70.0 { CPU } else { c };
            self.rect(
                Area {
                    x,
                    y: a.bottom() - h,
                    w,
                    h,
                },
                c,
                255,
            );
        }
    }

    /// Mini-historique ; `max` fixe l'échelle, sinon elle suit le maximum observé.
    fn sparkline(
        &mut self,
        a: Area,
        values: &VecDeque<f32>,
        max: Option<f32>,
        c: Rgb,
        label: &str,
    ) {
        if a.h < 12.0 || a.w < 12.0 {
            return;
        }
        self.rounded(a, 8.0, BG, 255);
        // Légende seulement si la courbe a la place de passer dessous.
        if !label.is_empty() && a.h >= 56.0 {
            self.text(a.x + 10.0, a.y + 22.0, 16.0, Weight::Regular, MUTED, label);
        }
        if values.len() < 2 {
            return;
        }
        let peak = values.iter().copied().fold(0.0_f32, f32::max);
        let scale = max.unwrap_or(peak * 1.15).max(1.0);
        let pad = 5.0;
        let (w, h) = (a.w - 2.0 * pad, a.h - 2.0 * pad);
        // Les points les plus récents sont collés au bord droit.
        let capacity = values.len().max(2) as f32 - 1.0;
        let point = |i: usize, v: f32| {
            let x = a.x + pad + w * i as f32 / capacity;
            let v = if v.is_finite() { v } else { 0.0 };
            let y = a.y + pad + h * (1.0 - (v / scale).clamp(0.0, 1.0));
            (x, y)
        };
        let mut line = PathBuilder::new();
        let mut fill = PathBuilder::new();
        fill.move_to(a.x + pad, a.y + pad + h);
        for (i, v) in values.iter().enumerate() {
            let (x, y) = point(i, *v);
            if i == 0 {
                line.move_to(x, y);
            } else {
                line.line_to(x, y);
            }
            fill.line_to(x, y);
        }
        fill.line_to(a.x + pad + w, a.y + pad + h);
        fill.close();
        // Une série entièrement nulle donnerait un polygone plat, refusé par tiny-skia.
        if peak > 0.0
            && let Some(path) = fill.finish()
        {
            self.pixmap.fill_path(
                &path,
                &paint(c, 46),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if let Some(path) = line.finish() {
            let stroke = Stroke {
                width: 2.5,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Default::default()
            };
            self.pixmap
                .stroke_path(&path, &paint(c, 255), &stroke, Transform::identity(), None);
        }
    }

    fn rect(&mut self, a: Area, c: Rgb, alpha: u8) {
        if let Some(rect) = Rect::from_xywh(a.x, a.y, a.w.max(0.5), a.h.max(0.5)) {
            self.pixmap
                .fill_rect(rect, &paint(c, alpha), Transform::identity(), None);
        }
    }

    fn rounded(&mut self, a: Area, radius: f32, c: Rgb, alpha: u8) {
        let r = radius.min(a.w / 2.0).min(a.h / 2.0).max(0.0);
        let (x0, y0, x1, y1) = (a.x, a.y, a.right(), a.bottom());
        let mut pb = PathBuilder::new();
        pb.move_to(x0 + r, y0);
        pb.line_to(x1 - r, y0);
        pb.quad_to(x1, y0, x1, y0 + r);
        pb.line_to(x1, y1 - r);
        pb.quad_to(x1, y1, x1 - r, y1);
        pb.line_to(x0 + r, y1);
        pb.quad_to(x0, y1, x0, y1 - r);
        pb.line_to(x0, y0 + r);
        pb.quad_to(x0, y0, x0 + r, y0);
        pb.close();
        if let Some(path) = pb.finish() {
            self.pixmap.fill_path(
                &path,
                &paint(c, alpha),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    // ------------------------------------------------------------------ texte

    fn glyph(&mut self, ch: char, size: f32, weight: Weight) -> &Glyph {
        let key = (ch, (size * 4.0) as u32, weight);
        let (regular, bold) = (&self.regular, &self.bold);
        self.glyphs.entry(key).or_insert_with(|| {
            let font = match weight {
                Weight::Regular => regular,
                Weight::Bold => bold,
            };
            let (m, coverage) = font.rasterize(ch, size);
            Glyph {
                width: m.width,
                height: m.height,
                xmin: m.xmin,
                ymin: m.ymin,
                advance: m.advance_width,
                coverage,
            }
        })
    }

    fn measure(&mut self, size: f32, weight: Weight, text: &str) -> f32 {
        text.chars()
            .map(|ch| self.glyph(ch, size, weight).advance)
            .sum()
    }

    /// Dessine `text` avec `y` comme ligne de base.
    fn text(&mut self, x: f32, y: f32, size: f32, weight: Weight, c: Rgb, text: &str) {
        let mut pen = x;
        for ch in text.chars() {
            let g = self.glyph(ch, size, weight);
            let (gw, gh, advance) = (g.width, g.height, g.advance);
            let left = pen.round() as i32 + g.xmin;
            let top = y.round() as i32 - gh as i32 - g.ymin;
            // Copie nécessaire : le cache et l'image appartiennent au même objet.
            let coverage = g.coverage.clone();
            let (cw, ch) = (self.pixmap.width(), self.pixmap.height());
            let pixels = self.pixmap.pixels_mut();
            for row in 0..gh {
                let py = top + row as i32;
                if py < 0 || py >= ch as i32 {
                    continue;
                }
                for col in 0..gw {
                    let px = left + col as i32;
                    let cov = coverage[row * gw + col] as u32;
                    if cov == 0 || px < 0 || px >= cw as i32 {
                        continue;
                    }
                    let idx = py as usize * cw as usize + px as usize;
                    let dst = pixels[idx];
                    let mix =
                        |s: u8, d: u8| ((s as u32 * cov + d as u32 * (255 - cov)) / 255) as u8;
                    if let Some(p) = PremultipliedColorU8::from_rgba(
                        mix(c.0, dst.red()),
                        mix(c.1, dst.green()),
                        mix(c.2, dst.blue()),
                        255,
                    ) {
                        pixels[idx] = p;
                    }
                }
            }
            pen += advance;
        }
    }
}

fn color(c: Rgb, alpha: u8) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.0, c.1, c.2, alpha)
}

fn paint(c: Rgb, alpha: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.0, c.1, c.2, alpha);
    p.anti_alias = true;
    p
}

fn ratio(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) as f32
    }
}

fn temp_color(value: f32, warm: f32, hot: f32) -> Rgb {
    if value >= hot {
        HOT
    } else if value >= warm {
        WARM
    } else {
        GPU
    }
}

/// Format de la date courte de l'en-tête (JJ/MM/AAAA, ou AAAA-MM-JJ en anglais).
pub fn date_format(lang: Lang) -> &'static str {
    match lang {
        Lang::Fr => "%d/%m/%Y",
        Lang::En => "%Y-%m-%d",
    }
}

/// Taille lisible, en unités binaires francisées (« 12,3 Go »).
pub fn fmt_bytes(bytes: u64) -> String {
    fmt_bytes_in(lang(), bytes)
}

/// Taille lisible dans une langue donnée (« 12,3 Go », « 12.3 GB »).
pub fn fmt_bytes_in(lang: Lang, bytes: u64) -> String {
    let units: [&str; 5] = match lang {
        Lang::Fr => ["o", "Ko", "Mo", "Go", "To"],
        Lang::En => ["B", "KB", "MB", "GB", "TB"],
    };
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let text = if unit == 0 || value >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    };
    let text = match lang {
        Lang::Fr => text.replace('.', ","),
        Lang::En => text,
    };
    format!("{text} {}", units[unit])
}

/// Débit lisible (« 1,2 Mo/s »).
pub fn fmt_rate(bytes_per_s: f64) -> String {
    format!("{}/s", fmt_bytes(bytes_per_s.max(0.0) as u64))
}

/// Durée de fonctionnement compacte (« 3 j 04 h », « 5 h 07 », « 12 min »).
pub fn fmt_uptime(secs: u64) -> String {
    fmt_uptime_in(lang(), secs)
}

/// Durée de fonctionnement dans une langue donnée (« 3 j 04 h », « 3 d 04 h »).
pub fn fmt_uptime_in(lang: Lang, secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    if d > 0 {
        let day = match lang {
            Lang::Fr => "j",
            Lang::En => "d",
        };
        format!("{d} {day} {h:02} h")
    } else if h > 0 {
        format!("{h} h {m:02}")
    } else {
        format!("{m} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::*;
    use chrono::TimeZone;
    use std::time::Duration;

    fn sample() -> (Snapshot, History) {
        let snapshot = Snapshot {
            hostname: "workstation".into(),
            time: chrono::Local
                .with_ymd_and_hms(2026, 11, 14, 13, 37, 42)
                .unwrap(),
            uptime: Duration::from_secs(3 * 86_400 + 4 * 3600),
            cpu: CpuMetrics {
                model: "Ryzen 9 9950X3D".into(),
                usage: 37.0,
                per_core: (0..32).map(|i| (i * 3) as f32).collect(),
                freq_mhz: Some(4321.0),
                freq_max_mhz: Some(5712.0),
                temp_c: Some(61.5),
            },
            gpu: Some(GpuMetrics {
                name: "RTX 5090".into(),
                temp_c: Some(72.0),
                usage: Some(88.0),
                mem_used: 21 << 30,
                mem_total: 32 << 30,
                power_w: Some(431.0),
                power_limit_w: Some(600.0),
                fans: vec![45],
                clock_mhz: Some(2610),
                mem_clock_mhz: Some(15000),
            }),
            memory: MemoryMetrics {
                total: 64 << 30,
                used: 23 << 30,
                swap_total: 8 << 30,
                swap_used: 1 << 30,
            },
            disks: vec![
                DiskMetrics {
                    mount: "/".into(),
                    total: 500 << 30,
                    used: 120 << 30,
                },
                DiskMetrics {
                    mount: "/home".into(),
                    total: 3700 << 30,
                    used: 752 << 30,
                },
            ],
            network: NetworkMetrics {
                interface: "eth0".into(),
                rx_bps: 12.5e6,
                tx_bps: 840e3,
            },
        };
        let mut history = History::new(120);
        for i in 0..120 {
            let mut s = snapshot.clone();
            let wave = ((i as f32 / 9.0).sin() + 1.0) / 2.0;
            s.cpu.usage = 20.0 + 60.0 * wave;
            if let Some(gpu) = &mut s.gpu {
                gpu.usage = Some(100.0 * wave);
            }
            s.network.rx_bps = 20e6 * wave as f64;
            history.record(&s);
        }
        (snapshot, history)
    }

    fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = (y * WIDTH + x) as usize * 4;
        [data[i], data[i + 1], data[i + 2], data[i + 3]]
    }

    #[test]
    fn renders_full_opaque_frame() {
        let (snapshot, history) = sample();
        let mut renderer = Renderer::new();
        let data = renderer.render(&snapshot, &history, &DashboardConfig::default());
        assert_eq!(data.len(), (WIDTH * HEIGHT * 4) as usize);
        assert!(
            data.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
            "image opaque attendue"
        );
        // Coin : fond ; intérieur d'un panneau : couleur de panneau.
        assert_eq!(pixel(data, 2, 2), [BG.0, BG.1, BG.2, 255]);
        assert_eq!(pixel(data, 600, 90), [PANEL.0, PANEL.1, PANEL.2, 255]);
        // Du texte clair et des accents colorés ont bien été dessinés.
        let bright = data
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200)
            .count();
        assert!(bright > 2000, "texte absent ({bright} pixels clairs)");
        let green = data
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[..3] == [GPU.0, GPU.1, GPU.2])
            .count();
        assert!(green > 500, "accent GPU absent ({green} pixels)");
    }

    #[test]
    fn rotation_180_flips_the_frame() {
        let (snapshot, history) = sample();
        let mut renderer = Renderer::new();
        let normal = renderer
            .render(&snapshot, &history, &DashboardConfig::default())
            .to_vec();
        let cfg = DashboardConfig {
            rotation: 180,
            ..Default::default()
        };
        let rotated = renderer.render(&snapshot, &history, &cfg);
        assert_eq!(pixel(rotated, 0, 0), pixel(&normal, WIDTH - 1, HEIGHT - 1));
        assert_eq!(
            pixel(rotated, 100, 50),
            pixel(&normal, WIDTH - 101, HEIGHT - 51)
        );
    }

    #[test]
    fn quarter_turns_use_a_portrait_layout() {
        let (snapshot, history) = sample();
        let mut renderer = Renderer::new();
        let render = |renderer: &mut Renderer, rotation: u16, widgets: &[&str]| {
            let cfg = DashboardConfig {
                rotation,
                widgets: widgets.iter().map(|w| (*w).to_owned()).collect(),
                ..Default::default()
            };
            renderer.render(&snapshot, &history, &cfg).to_vec()
        };
        let all = ["gpu", "cpu", "memory", "disks", "network"];
        let r90 = render(&mut renderer, 90, &all);
        let r270 = render(&mut renderer, 270, &all);
        let r0 = render(&mut renderer, 0, &all);
        for data in [&r90, &r270] {
            assert_eq!(data.len(), (WIDTH * HEIGHT * 4) as usize);
            assert!(data.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        }
        assert_ne!(r90, r0);
        // 270° = 90° puis un demi-tour.
        for (x, y) in [(0, 0), (100, 50), (640, 400), (1279, 799)] {
            assert_eq!(
                pixel(&r270, x, y),
                pixel(&r90, WIDTH - 1 - x, HEIGHT - 1 - y),
                "({x}, {y})"
            );
        }
        // Remis droit : 800 × 1280, carte graphique en pleine largeur sous l'en-tête,
        // panneaux du bas deux par deux.
        let upright = rotate::quarter_turn(&r90, WIDTH, HEIGHT, false);
        let at = |x: u32, y: u32| {
            let i = (y * HEIGHT + x) as usize * 4;
            [upright[i], upright[i + 1], upright[i + 2]]
        };
        assert_eq!(at(2, 2), [BG.0, BG.1, BG.2]);
        assert_eq!(at(400, 112), [PANEL.0, PANEL.1, PANEL.2], "panneau GPU");
        assert_eq!(at(780, 112), [PANEL.0, PANEL.1, PANEL.2], "pleine largeur");
        assert_eq!(at(400, 1270), [BG.0, BG.1, BG.2], "marge basse");
        // Sous-ensembles : rien ne panique, l'image reste pleine.
        for widgets in [
            &["network"][..],
            &["gpu"],
            &["memory", "disks", "network"],
            &[],
        ] {
            assert_eq!(
                render(&mut renderer, 270, widgets).len(),
                (WIDTH * HEIGHT * 4) as usize
            );
        }
        // Le canevas paysage est retrouvé ensuite.
        assert_eq!(render(&mut renderer, 0, &all), r0);
    }

    #[test]
    fn portrait_layout_fills_the_canvas() {
        let panels = portrait_layout(
            &["gpu", "cpu"],
            &["memory", "disks", "network"],
            800.0,
            1280.0,
            104.0,
        );
        assert_eq!(panels.len(), 5);
        let get = |n: &str| panels.iter().find(|(name, _)| name == n).unwrap().1;
        let (gpu, cpu, mem, disks, net) = (
            get("gpu"),
            get("cpu"),
            get("memory"),
            get("disks"),
            get("network"),
        );
        assert_eq!((gpu.x, gpu.w), (16.0, 768.0));
        assert!(cpu.y > gpu.bottom() && mem.y > cpu.bottom());
        assert_eq!(mem.y, disks.y, "mémoire et disques côte à côte");
        assert_eq!(net.w, 768.0, "le dernier seul s'étend");
        assert!((net.bottom() - (1280.0 - 16.0)).abs() < 0.5, "jusqu'en bas");
        assert!(gpu.h >= 340.0, "place pour les jauges ({})", gpu.h);
        assert!(portrait_layout(&[], &[], 800.0, 1280.0, 104.0).is_empty());
        let only = portrait_layout(&[], &["network"], 800.0, 1280.0, 104.0);
        assert!((only[0].1.h - (1280.0 - 104.0 - 16.0)).abs() < 0.5);
    }

    #[test]
    fn survives_missing_gpu_empty_history_and_widget_subsets() {
        let (mut snapshot, _) = sample();
        snapshot.gpu = None;
        snapshot.disks.clear();
        snapshot.cpu.per_core.clear();
        snapshot.cpu.temp_c = None;
        let mut renderer = Renderer::new();
        for widgets in [
            vec!["gpu", "cpu", "memory", "disks", "network"],
            vec!["cpu"],
            vec!["network", "disks"],
            vec![],
        ] {
            let cfg = DashboardConfig {
                widgets: widgets.into_iter().map(String::from).collect(),
                ..Default::default()
            };
            let data = renderer.render(&snapshot, &History::new(10), &cfg);
            assert_eq!(data.len(), (WIDTH * HEIGHT * 4) as usize);
        }
    }

    #[test]
    fn widget_order_follows_the_configuration() {
        let (snapshot, history) = sample();
        let mut renderer = Renderer::new();
        let accent_x = |data: &[u8], c: Rgb| -> u32 {
            // Abscisse de la barre d'accent du titre du panneau (rangée du haut).
            (0..WIDTH)
                .find(|x| pixel(data, *x, 115)[..3] == [c.0, c.1, c.2])
                .unwrap()
        };
        let data = renderer
            .render(&snapshot, &history, &DashboardConfig::default())
            .to_vec();
        assert!(accent_x(&data, GPU) < accent_x(&data, CPU));
        let cfg = DashboardConfig {
            widgets: ["cpu", "gpu", "network"].map(String::from).to_vec(),
            ..Default::default()
        };
        let data = renderer.render(&snapshot, &history, &cfg).to_vec();
        assert!(accent_x(&data, CPU) < accent_x(&data, GPU));
    }

    #[test]
    fn formatting_helpers() {
        assert_eq!(fmt_bytes(0), "0 o");
        assert_eq!(fmt_bytes(1536), "1,5 Ko");
        assert_eq!(fmt_bytes(32 << 30), "32,0 Go");
        assert_eq!(fmt_bytes(3700 << 30), "3,6 To");
        assert_eq!(fmt_rate(12.5e6), "11,9 Mo/s");
        assert_eq!(fmt_uptime(59), "0 min");
        assert_eq!(fmt_uptime(5 * 3600 + 7 * 60), "5 h 07");
        assert_eq!(fmt_uptime(3 * 86_400 + 4 * 3600), "3 j 04 h");
        // Anglais : unités anglaises, point décimal.
        assert_eq!(fmt_bytes_in(Lang::En, 1536), "1.5 KB");
        assert_eq!(fmt_bytes_in(Lang::En, 3700 << 30), "3.6 TB");
        assert_eq!(fmt_bytes_in(Lang::En, 0), "0 B");
        assert_eq!(fmt_uptime_in(Lang::En, 3 * 86_400 + 4 * 3600), "3 d 04 h");
        assert_eq!(date_format(Lang::Fr), "%d/%m/%Y");
        assert_eq!(date_format(Lang::En), "%Y-%m-%d");
    }

    /// Écrit une capture dans `target/` pour inspection visuelle (PPM, sans dépendance).
    #[test]
    fn writes_snapshot_for_inspection() {
        let (snapshot, history) = sample();
        let mut renderer = Renderer::new();
        let data = renderer.render(&snapshot, &history, &DashboardConfig::default());
        let mut ppm = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
        ppm.extend(
            data.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]]),
        );
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(dir.join("dashboard-test.ppm"), ppm);
            // Portrait, remis droit.
            let cfg = DashboardConfig {
                rotation: 90,
                ..Default::default()
            };
            let turned = renderer.render(&snapshot, &history, &cfg).to_vec();
            let upright = rotate::quarter_turn(&turned, WIDTH, HEIGHT, false);
            let mut ppm = format!("P6\n{HEIGHT} {WIDTH}\n255\n").into_bytes();
            ppm.extend(
                upright
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2]]),
            );
            let _ = std::fs::write(dir.join("dashboard-portrait-test.ppm"), ppm);
        }
    }
}
