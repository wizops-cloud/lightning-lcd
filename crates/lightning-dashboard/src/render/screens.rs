//! Écrans plein format hors tableau de bord : horloge et lecture en cours.

use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use lightning_i18n::{Lang, tr, trf};
use serde::{Deserialize, Serialize};
use tiny_skia::{
    FillRule, FilterQuality, GradientStop, IntSize, LinearGradient, Paint, PathBuilder, Pattern,
    Pixmap, Point, SpreadMode, Transform,
};

use super::{Area, BG, HEIGHT, MUTED, PANEL, Renderer, Rgb, TEXT, TRACK, WIDTH, Weight, color};

/// Accent de la lecture en cours (barre de progression, icône).
const ACCENT: Rgb = (45, 212, 191);

/// Réglages de l'horloge plein écran (section `[clock]`).
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ClockConfig {
    /// Affiche les secondes.
    pub seconds: bool,
    /// Affiche la date sous l'heure.
    pub date: bool,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            seconds: false,
            date: true,
        }
    }
}

/// Morceau en cours de lecture (MPRIS).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Nom du lecteur (« Spotify », « Rhythmbox »…).
    pub player: String,
    /// Durée du morceau, si connue.
    pub length: Option<Duration>,
    pub position: Duration,
    /// En lecture (sinon en pause).
    pub playing: bool,
}

impl Track {
    /// Résumé d'une ligne : « Titre — Artiste ».
    pub fn summary(&self) -> String {
        match (self.title.is_empty(), self.artist.is_empty()) {
            (true, _) => self.player.clone(),
            (false, true) => self.title.clone(),
            (false, false) => format!("{} — {}", self.title, self.artist),
        }
    }

    /// Avancement entre 0 et 1, si la durée est connue.
    pub fn progress(&self) -> Option<f32> {
        let length = self.length.filter(|l| !l.is_zero())?;
        Some((self.position.as_secs_f32() / length.as_secs_f32()).clamp(0.0, 1.0))
    }
}

/// Pochette décodée, en RGBA opaque.
#[derive(Debug, Clone, PartialEq)]
pub struct Artwork {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Artwork {
    /// Couleur moyenne (échantillonnée), pour teinter le fond.
    pub fn average(&self) -> Rgb {
        let pixels = self.rgba.as_chunks::<4>().0;
        let step = (pixels.len() / 4096).max(1);
        let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
        for p in pixels.iter().step_by(step) {
            r += u64::from(p[0]);
            g += u64::from(p[1]);
            b += u64::from(p[2]);
            n += 1;
        }
        if n == 0 {
            return PANEL;
        }
        ((r / n) as u8, (g / n) as u8, (b / n) as u8)
    }
}

const WEEKDAYS: [&str; 7] = [
    "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
];
const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

const EN_WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const EN_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Date longue en français : « dimanche 1er novembre 2026 ».
pub fn fr_long_date(date: NaiveDate) -> String {
    long_date(Lang::Fr, date)
}

/// Date longue : « dimanche 1er novembre 2026 », « Sunday, November 1, 2026 ».
pub fn long_date(lang: Lang, date: NaiveDate) -> String {
    let weekday = date.weekday().num_days_from_monday() as usize;
    match lang {
        Lang::Fr => format!("{} {}", WEEKDAYS[weekday], day_date(lang, date)),
        Lang::En => format!("{}, {}", EN_WEEKDAYS[weekday], day_date(lang, date)),
    }
}

/// Date sans jour de la semaine : « 1er novembre 2026 », « November 1, 2026 ».
pub fn day_date(lang: Lang, date: NaiveDate) -> String {
    match lang {
        Lang::Fr => {
            let day = match date.day() {
                1 => "1er".to_owned(),
                d => d.to_string(),
            };
            format!("{day} {} {}", MONTHS[date.month0() as usize], date.year())
        }
        Lang::En => format!(
            "{} {}, {}",
            EN_MONTHS[date.month0() as usize],
            date.day(),
            date.year()
        ),
    }
}

/// Durée de lecture : « 3:05 », « 1:02:03 ».
pub fn fmt_track_time(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Coupe `text` en au plus `max_lines` lignes de largeur `max_w` ; la dernière
/// ligne est terminée par « … » si le texte ne tient pas.
pub fn wrap_text(
    text: &str,
    max_w: f32,
    max_lines: usize,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut truncated = false;
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if measure(&candidate) <= max_w {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            if lines.len() + 1 == max_lines {
                current = candidate;
                truncated = true;
                break;
            }
            lines.push(std::mem::take(&mut current));
        }
        // Mot plus long qu'une ligne : coupé au caractère.
        let mut rest = word.to_owned();
        while measure(&rest) > max_w {
            let mut head = String::new();
            for ch in rest.chars() {
                let mut next = head.clone();
                next.push(ch);
                if measure(&next) > max_w && !head.is_empty() {
                    break;
                }
                head = next;
            }
            let tail = rest[head.len()..].to_owned();
            if lines.len() + 1 == max_lines {
                truncated = true;
                break;
            }
            lines.push(head);
            rest = tail;
        }
        current = rest;
        if truncated {
            break;
        }
    }
    if truncated || measure(&current) > max_w {
        let mut line = current;
        while !line.is_empty() && measure(&format!("{line}…")) > max_w {
            line.pop();
        }
        current = format!("{}…", line.trim_end());
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn rounded_path(a: Area, radius: f32) -> Option<tiny_skia::Path> {
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
    pb.finish()
}

impl Renderer {
    /// Horloge plein écran, lisible de loin. Renvoie l'image RGBA 1280×800.
    pub fn render_clock(&mut self, time: NaiveDateTime, cfg: &ClockConfig, rotation: u16) -> &[u8] {
        self.pixmap.fill(color(BG, 255));
        let hm = time.format("%H:%M").to_string();
        let (size, small) = if cfg.seconds {
            (250.0, 100.0)
        } else {
            (300.0, 0.0)
        };
        let secs = format!(":{:02}", time.second());
        let hm_w = self.measure(size, Weight::Bold, &hm);
        let secs_w = if cfg.seconds {
            self.measure(small, Weight::Bold, &secs)
        } else {
            0.0
        };
        let x = (WIDTH as f32 - hm_w - secs_w) / 2.0;
        // Ligne de base : chiffres (≈ 0,73 em) centrés, date en dessous.
        let digits_h = size * 0.73;
        let block_h = if cfg.date { digits_h + 120.0 } else { digits_h };
        let baseline = (HEIGHT as f32 - block_h) / 2.0 + digits_h;
        self.text(x, baseline, size, Weight::Bold, TEXT, &hm);
        if cfg.seconds {
            self.text(x + hm_w, baseline, small, Weight::Bold, MUTED, &secs);
        }
        if cfg.date {
            let date = long_date(lightning_i18n::lang(), time.date());
            let dw = self.measure(52.0, Weight::Regular, &date);
            self.text(
                (WIDTH as f32 - dw) / 2.0,
                baseline + 120.0,
                52.0,
                Weight::Regular,
                MUTED,
                &date,
            );
        }
        self.finish(rotation)
    }

    /// Lecture en cours : pochette, titre, artiste, album, progression ;
    /// un message quand aucun lecteur ne joue.
    pub fn render_now_playing(
        &mut self,
        track: Option<&Track>,
        art: Option<&Artwork>,
        time: NaiveDateTime,
        rotation: u16,
    ) -> &[u8] {
        let (w, h) = (WIDTH as f32, HEIGHT as f32);
        // Fond : teinte de la pochette en haut à gauche, fondue vers le noir.
        let tint = art.map_or(PANEL, |a| mix(BG, a.average(), 0.35));
        self.pixmap.fill(color(BG, 255));
        if let Some(shader) = LinearGradient::new(
            Point::from_xy(0.0, 0.0),
            Point::from_xy(w * 0.7, h),
            vec![
                GradientStop::new(0.0, color(tint, 255)),
                GradientStop::new(1.0, color(BG, 255)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            let paint = Paint {
                shader,
                ..Paint::default()
            };
            if let Some(rect) = tiny_skia::Rect::from_xywh(0.0, 0.0, w, h) {
                self.pixmap
                    .fill_rect(rect, &paint, Transform::identity(), None);
            }
        }

        // Petite horloge en haut à droite.
        let clock = time.format("%H:%M").to_string();
        let cw = self.measure(34.0, Weight::Bold, &clock);
        self.text(w - 64.0 - cw, 76.0, 34.0, Weight::Bold, TEXT, &clock);

        let Some(track) = track else {
            self.note_icon(w / 2.0, 300.0, 1.6, MUTED);
            let title = tr("Aucune lecture en cours");
            let tw = self.measure(48.0, Weight::Bold, title);
            self.text((w - tw) / 2.0, 470.0, 48.0, Weight::Bold, TEXT, title);
            let hint = tr("Lancez un morceau dans un lecteur compatible MPRIS");
            let hw = self.measure(24.0, Weight::Regular, hint);
            self.text((w - hw) / 2.0, 520.0, 24.0, Weight::Regular, MUTED, hint);
            let hint = tr("(Spotify, Rhythmbox, Lollypop, navigateur…)");
            let hw = self.measure(24.0, Weight::Regular, hint);
            self.text((w - hw) / 2.0, 556.0, 24.0, Weight::Regular, MUTED, hint);
            return self.finish(rotation);
        };

        let header = if track.player.is_empty() {
            tr("EN COURS DE LECTURE").to_owned()
        } else {
            trf(
                "EN COURS DE LECTURE · {player}",
                &[("player", &track.player.to_uppercase())],
            )
        };
        self.text(64.0, 74.0, 22.0, Weight::Regular, MUTED, &header);

        // Pochette (ou icône) à gauche.
        let cover = Area {
            x: 64.0,
            y: 160.0,
            w: 480.0,
            h: 480.0,
        };
        if !art.is_some_and(|a| self.artwork(a, cover, 24.0)) {
            self.rounded(cover, 24.0, PANEL, 255);
            self.note_icon(cover.x + cover.w / 2.0, cover.y + cover.h / 2.0, 2.2, TRACK);
        }

        // Textes à droite.
        let (x, max_w) = (608.0, w - 608.0 - 64.0);
        let title = if track.title.is_empty() {
            tr("Titre inconnu")
        } else {
            &track.title
        };
        let mut y = 236.0;
        let lines = wrap_text(title, max_w, 3, |s| self.measure(50.0, Weight::Bold, s));
        for line in &lines {
            self.text(x, y, 50.0, Weight::Bold, TEXT, line);
            y += 62.0;
        }
        y -= 4.0;
        if !track.artist.is_empty() {
            for line in wrap_text(&track.artist, max_w, 1, |s| {
                self.measure(32.0, Weight::Regular, s)
            }) {
                self.text(x, y, 32.0, Weight::Regular, TEXT, &line);
            }
            y += 46.0;
        }
        if !track.album.is_empty() {
            for line in wrap_text(&track.album, max_w, 1, |s| {
                self.measure(24.0, Weight::Regular, s)
            }) {
                self.text(x, y, 24.0, Weight::Regular, MUTED, &line);
            }
        }

        // État, progression et temps.
        let state_y = 548.0;
        if track.playing {
            self.play_icon(x, state_y - 22.0, 24.0, ACCENT);
        } else {
            self.pause_icon(x, state_y - 22.0, 24.0, ACCENT);
        }
        let state = if track.playing {
            tr("En lecture")
        } else {
            tr("En pause")
        };
        self.text(x + 40.0, state_y, 24.0, Weight::Regular, MUTED, state);
        let bar = Area {
            x,
            y: 580.0,
            w: max_w,
            h: 10.0,
        };
        self.rounded(bar, 5.0, TRACK, 255);
        if let Some(frac) = track.progress() {
            if frac > 0.0 {
                self.rounded(
                    Area {
                        w: (bar.w * frac).max(bar.h),
                        ..bar
                    },
                    5.0,
                    ACCENT,
                    255,
                );
            }
            let total = fmt_track_time(track.length.unwrap_or_default());
            let tw = self.measure(22.0, Weight::Regular, &total);
            self.text(
                bar.right() - tw,
                630.0,
                22.0,
                Weight::Regular,
                MUTED,
                &total,
            );
        }
        let pos = fmt_track_time(track.position);
        self.text(x, 630.0, 22.0, Weight::Regular, MUTED, &pos);
        self.finish(rotation)
    }

    /// Rotation éventuelle puis données RGBA.
    fn finish(&mut self, rotation: u16) -> &[u8] {
        if rotation == 180 {
            self.pixmap.pixels_mut().reverse();
        }
        self.pixmap.data()
    }

    /// Dessine la pochette recadrée (remplissage) aux coins arrondis.
    fn artwork(&mut self, art: &Artwork, a: Area, radius: f32) -> bool {
        let expected = art.width as usize * art.height as usize * 4;
        let Some(size) = IntSize::from_wh(art.width, art.height) else {
            return false;
        };
        if art.rgba.len() != expected {
            return false;
        }
        // Opaque : RGBA et RGBA prémultiplié coïncident.
        let mut data = art.rgba.clone();
        for p in data.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        let Some(source) = Pixmap::from_vec(data, size) else {
            return false;
        };
        let scale = (a.w / art.width as f32).max(a.h / art.height as f32);
        let dx = a.x - (art.width as f32 * scale - a.w) / 2.0;
        let dy = a.y - (art.height as f32 * scale - a.h) / 2.0;
        let pattern = Pattern::new(
            source.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bicubic,
            1.0,
            Transform::from_row(scale, 0.0, 0.0, scale, dx, dy),
        );
        let paint = Paint {
            shader: pattern,
            anti_alias: true,
            ..Paint::default()
        };
        let Some(path) = rounded_path(a, radius) else {
            return false;
        };
        self.pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        true
    }

    /// Note de musique stylisée centrée sur `(cx, cy)` (taille ≈ 60 × `scale`).
    fn note_icon(&mut self, cx: f32, cy: f32, scale: f32, c: Rgb) {
        let s = scale;
        // Deux têtes, deux hampes, une barre.
        for (hx, hy) in [(-22.0, 26.0), (22.0, 18.0)] {
            if let Some(path) = PathBuilder::from_oval(
                tiny_skia::Rect::from_xywh(
                    cx + (hx - 12.0) * s,
                    cy + (hy - 9.0) * s,
                    24.0 * s,
                    18.0 * s,
                )
                .expect("ovale valide"),
            ) {
                self.pixmap.fill_path(
                    &path,
                    &super::paint(c, 255),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
        self.rect(
            Area {
                x: cx - 13.0 * s,
                y: cy - 30.0 * s,
                w: 4.0 * s,
                h: 56.0 * s,
            },
            c,
            255,
        );
        self.rect(
            Area {
                x: cx + 31.0 * s,
                y: cy - 38.0 * s,
                w: 4.0 * s,
                h: 56.0 * s,
            },
            c,
            255,
        );
        let mut pb = PathBuilder::new();
        pb.move_to(cx - 13.0 * s, cy - 30.0 * s);
        pb.line_to(cx + 35.0 * s, cy - 38.0 * s);
        pb.line_to(cx + 35.0 * s, cy - 26.0 * s);
        pb.line_to(cx - 13.0 * s, cy - 18.0 * s);
        pb.close();
        if let Some(path) = pb.finish() {
            self.pixmap.fill_path(
                &path,
                &super::paint(c, 255),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn play_icon(&mut self, x: f32, y: f32, size: f32, c: Rgb) {
        let mut pb = PathBuilder::new();
        pb.move_to(x + size * 0.1, y);
        pb.line_to(x + size, y + size / 2.0);
        pb.line_to(x + size * 0.1, y + size);
        pb.close();
        if let Some(path) = pb.finish() {
            self.pixmap.fill_path(
                &path,
                &super::paint(c, 255),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn pause_icon(&mut self, x: f32, y: f32, size: f32, c: Rgb) {
        for dx in [0.1, 0.6] {
            self.rounded(
                Area {
                    x: x + size * dx,
                    y,
                    w: size * 0.3,
                    h: size,
                },
                2.0,
                c,
                255,
            );
        }
    }
}

/// Pochette de démonstration : dégradé et cercles, sans donnée extérieure.
pub fn demo_artwork(size: u32) -> Artwork {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 / s, y as f32 / s);
            let base = mix((255, 138, 76), (178, 137, 255), (fx + fy) / 2.0);
            let d1 = ((fx - 0.62).powi(2) + (fy - 0.4).powi(2)).sqrt();
            let d2 = ((fx - 0.3).powi(2) + (fy - 0.72).powi(2)).sqrt();
            // Bords adoucis sur environ un pixel.
            let edge = |inside: f32| (inside * s).clamp(0.0, 1.0);
            let mut c = base;
            c = mix(c, (255, 255, 255), 0.6 * edge(0.012 - (d1 - 0.3).abs()));
            c = mix(c, (250, 204, 21), 0.85 * edge(0.22 - d1));
            c = mix(c, (20, 25, 35), 0.8 * edge(0.14 - d2));
            rgba.extend_from_slice(&[c.0, c.1, c.2, 255]);
        }
    }
    Artwork {
        width: size,
        height: size,
        rgba,
    }
}

/// Morceau de démonstration (captures de la documentation).
pub fn demo_track(elapsed: Duration) -> Track {
    let length = Duration::from_secs(245);
    Track {
        title: tr("Lumière d'automne sur la ville").into(),
        artist: tr("Ensemble Démonstration").into(),
        album: tr("Écran de démonstration").into(),
        player: tr("Lecteur").into(),
        length: Some(length),
        position: Duration::from_secs(83 + elapsed.as_secs() % 150),
        playing: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, s)
            .unwrap()
    }

    fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = (y * WIDTH + x) as usize * 4;
        [data[i], data[i + 1], data[i + 2], data[i + 3]]
    }

    fn bright(data: &[u8]) -> usize {
        data.as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200)
            .count()
    }

    #[test]
    fn french_dates_and_times() {
        assert_eq!(
            fr_long_date(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap()),
            "lundi 16 novembre 2026"
        );
        assert_eq!(
            fr_long_date(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()),
            "dimanche 1er novembre 2026"
        );
        assert_eq!(
            fr_long_date(NaiveDate::from_ymd_opt(2027, 8, 15).unwrap()),
            "dimanche 15 août 2027"
        );
        assert_eq!(
            long_date(Lang::En, NaiveDate::from_ymd_opt(2026, 11, 16).unwrap()),
            "Monday, November 16, 2026"
        );
        assert_eq!(
            day_date(Lang::En, NaiveDate::from_ymd_opt(2024, 7, 1).unwrap()),
            "July 1, 2024"
        );
        assert_eq!(
            day_date(Lang::Fr, NaiveDate::from_ymd_opt(2024, 7, 1).unwrap()),
            "1er juillet 2024"
        );
        assert_eq!(fmt_track_time(Duration::from_secs(185)), "3:05");
        assert_eq!(fmt_track_time(Duration::from_secs(9)), "0:09");
        assert_eq!(fmt_track_time(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn wrapping_and_ellipsis() {
        let m = |s: &str| s.chars().count() as f32;
        assert_eq!(wrap_text("un deux trois", 20.0, 2, m), ["un deux trois"]);
        assert_eq!(wrap_text("un deux trois", 8.0, 2, m), ["un deux", "trois"]);
        assert_eq!(
            wrap_text("un deux trois quatre", 8.0, 2, m),
            ["un deux", "trois q…"]
        );
        let long = wrap_text("abcdefghijklmnop", 6.0, 2, m);
        assert_eq!(long, ["abcdef", "ghijk…"]);
        assert!(long.iter().all(|l| m(l) <= 6.0));
        assert_eq!(wrap_text("abcdefghij", 4.0, 1, m), ["abc…"]);
        assert!(wrap_text("", 10.0, 2, m).is_empty());
        assert!(wrap_text("texte", 10.0, 0, m).is_empty());
    }

    #[test]
    fn track_summary_and_progress() {
        let mut t = demo_track(Duration::ZERO);
        assert_eq!(
            t.summary(),
            "Lumière d'automne sur la ville — Ensemble Démonstration"
        );
        assert!((t.progress().unwrap() - 83.0 / 245.0).abs() < 1e-4);
        t.length = None;
        assert_eq!(t.progress(), None);
        t.artist.clear();
        assert_eq!(t.summary(), "Lumière d'automne sur la ville");
        t.title.clear();
        assert_eq!(t.summary(), "Lecteur");
        t.length = Some(Duration::from_secs(10));
        t.position = Duration::from_secs(60);
        assert_eq!(t.progress(), Some(1.0));
    }

    #[test]
    fn clock_renders_time_and_rotates() {
        let mut r = Renderer::new();
        let time = at(2026, 11, 16, 13, 37, 42);
        let plain = r.render_clock(time, &ClockConfig::default(), 0).to_vec();
        assert_eq!(plain.len(), (WIDTH * HEIGHT * 4) as usize);
        assert!(plain.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        assert_eq!(pixel(&plain, 2, 2), [BG.0, BG.1, BG.2, 255]);
        assert!(bright(&plain) > 20_000, "chiffres attendus en grand");
        let with_seconds = r
            .render_clock(
                time,
                &ClockConfig {
                    seconds: true,
                    date: false,
                },
                0,
            )
            .to_vec();
        assert_ne!(plain, with_seconds);
        let other_minute = r
            .render_clock(at(2026, 11, 16, 13, 38, 0), &ClockConfig::default(), 0)
            .to_vec();
        assert_ne!(plain, other_minute);
        let rotated = r.render_clock(time, &ClockConfig::default(), 180);
        assert_eq!(pixel(rotated, 0, 0), pixel(&plain, WIDTH - 1, HEIGHT - 1));
        assert_eq!(
            pixel(rotated, 300, 200),
            pixel(&plain, WIDTH - 301, HEIGHT - 201)
        );
    }

    #[test]
    fn now_playing_renders_track_art_and_idle_screen() {
        let mut r = Renderer::new();
        let time = at(2026, 11, 16, 21, 5, 0);
        let art = demo_artwork(256);
        let track = demo_track(Duration::ZERO);
        let full = r
            .render_now_playing(Some(&track), Some(&art), time, 0)
            .to_vec();
        assert!(full.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        // Pochette au centre de son cadre, accent dans la barre de progression.
        let cover = pixel(&full, 64 + 240, 160 + 240);
        assert_ne!(&cover[..3], &[PANEL.0, PANEL.1, PANEL.2]);
        assert_eq!(pixel(&full, 612, 585)[..3], [ACCENT.0, ACCENT.1, ACCENT.2]);
        assert_eq!(
            pixel(&full, WIDTH - 70, 585)[..3],
            [TRACK.0, TRACK.1, TRACK.2],
            "fin de barre non remplie à 34 %"
        );
        assert!(bright(&full) > 5_000, "texte attendu");

        // Sans pochette : cadre neutre ; en pause : rendu différent.
        let no_art = r.render_now_playing(Some(&track), None, time, 0).to_vec();
        assert_eq!(
            pixel(&no_art, 64 + 30, 160 + 30)[..3],
            [PANEL.0, PANEL.1, PANEL.2]
        );
        let paused = Track {
            playing: false,
            ..track.clone()
        };
        assert_ne!(
            r.render_now_playing(Some(&paused), None, time, 0).to_vec(),
            no_art
        );

        // Rien en lecture : message, et une pochette invalide ne fait pas planter.
        let idle = r.render_now_playing(None, None, time, 0).to_vec();
        assert!(bright(&idle) > 3_000);
        let broken = Artwork {
            width: 10,
            height: 10,
            rgba: vec![0; 12],
        };
        let _ = r.render_now_playing(Some(&track), Some(&broken), time, 180);
    }

    /// Écrit les écrans dans `target/` pour inspection visuelle (PPM).
    #[test]
    fn writes_screens_for_inspection() {
        let mut r = Renderer::new();
        let time = at(2026, 11, 16, 13, 37, 42);
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let write = |name: &str, data: &[u8]| {
            let mut ppm = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
            ppm.extend(
                data.as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2]]),
            );
            let _ = std::fs::write(dir.join(name), ppm);
        };
        write(
            "clock-test.ppm",
            r.render_clock(time, &ClockConfig::default(), 0),
        );
        write(
            "clock-seconds-test.ppm",
            r.render_clock(
                time,
                &ClockConfig {
                    seconds: true,
                    date: true,
                },
                0,
            ),
        );
        let art = demo_artwork(400);
        let track = demo_track(Duration::ZERO);
        write(
            "now-playing-test.ppm",
            r.render_now_playing(Some(&track), Some(&art), time, 0),
        );
        write(
            "now-playing-idle-test.ppm",
            r.render_now_playing(None, None, time, 0),
        );
    }
}
