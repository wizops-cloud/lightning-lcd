//! Petits outils de dessin sur un tampon `0RGB` (un `u32` par pixel) : recopie
//! mise à l'échelle, texte, bandeau de debug.

// Sans la fenêtre d'aperçu, seule l'image d'arrêt utilise ce module.
#![cfg_attr(not(feature = "window"), allow(dead_code))]

use std::collections::HashMap;

use fontdue::{Font, FontSettings};

type Rgb = (u8, u8, u8);

struct Glyph {
    width: usize,
    height: usize,
    xmin: i32,
    ymin: i32,
    advance: f32,
    coverage: Vec<u8>,
}

/// Dessine du texte avec la police embarquée du tableau de bord.
pub struct TextPainter {
    font: Font,
    glyphs: HashMap<(char, u32), Glyph>,
}

impl Default for TextPainter {
    fn default() -> Self {
        Self::new()
    }
}

fn pack(c: Rgb) -> u32 {
    (u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2)
}

fn mix(dst: u32, src: Rgb, alpha: u32) -> u32 {
    let blend = |s: u8, shift: u32| {
        let d = (dst >> shift) & 0xFF;
        ((u32::from(s) * alpha + d * (255 - alpha)) / 255) << shift
    };
    blend(src.0, 16) | blend(src.1, 8) | blend(src.2, 0)
}

impl TextPainter {
    pub fn new() -> Self {
        let font = Font::from_bytes(
            lightning_dashboard::render::FONT_REGULAR,
            FontSettings::default(),
        )
        .expect("police embarquée valide");
        Self {
            font,
            glyphs: HashMap::new(),
        }
    }

    fn glyph(&mut self, ch: char, size: f32) -> &Glyph {
        let font = &self.font;
        self.glyphs
            .entry((ch, (size * 4.0) as u32))
            .or_insert_with(|| {
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

    pub fn measure(&mut self, text: &str, size: f32) -> f32 {
        text.chars().map(|ch| self.glyph(ch, size).advance).sum()
    }

    /// Dessine `text` dans `buf` (`w` × `h`), `y` étant la ligne de base.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        buf: &mut [u32],
        w: usize,
        h: usize,
        x: f32,
        y: f32,
        size: f32,
        color: Rgb,
        text: &str,
    ) {
        let mut pen = x;
        for ch in text.chars() {
            let g = self.glyph(ch, size);
            let left = pen.round() as i64 + i64::from(g.xmin);
            let top = y.round() as i64 - g.height as i64 - i64::from(g.ymin);
            for row in 0..g.height {
                let py = top + row as i64;
                if py < 0 || py >= h as i64 {
                    continue;
                }
                for col in 0..g.width {
                    let px = left + col as i64;
                    let cov = u32::from(g.coverage[row * g.width + col]);
                    if cov == 0 || px < 0 || px >= w as i64 {
                        continue;
                    }
                    let idx = py as usize * w + px as usize;
                    buf[idx] = mix(buf[idx], color, cov);
                }
            }
            pen += g.advance;
        }
    }

    /// Bandeau de debug semi-transparent en haut à gauche.
    pub fn overlay(&mut self, buf: &mut [u32], w: usize, h: usize, lines: &[String]) {
        if lines.is_empty() || w < 40 || h < 40 {
            return;
        }
        let size = 15.0;
        let line_h = 20usize;
        let width = lines
            .iter()
            .map(|l| self.measure(l, size))
            .fold(0.0_f32, f32::max);
        let box_w = (width as usize + 20).min(w);
        let box_h = (lines.len() * line_h + 12).min(h);
        fill(buf, w, h, 0, 0, box_w, box_h, (0, 0, 0), 190);
        for (i, line) in lines.iter().enumerate() {
            let y = (8 + (i + 1) * line_h - 5) as f32;
            self.draw(buf, w, h, 10.0, y, size, (230, 236, 245), line);
        }
    }
}

/// Remplit un rectangle avec transparence (`alpha` de 0 à 255).
#[allow(clippy::too_many_arguments)]
pub fn fill(
    buf: &mut [u32],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    rw: usize,
    rh: usize,
    c: Rgb,
    alpha: u32,
) {
    for row in y..(y + rh).min(h) {
        for px in &mut buf[row * w + x.min(w)..row * w + (x + rw).min(w)] {
            *px = if alpha >= 255 {
                pack(c)
            } else {
                mix(*px, c, alpha)
            };
        }
    }
}

/// Rectangle occupé par une image `sw` × `sh` (après rotation) centrée dans une
/// zone `dw` × `dh` en conservant ses proportions : `(x, y, largeur, hauteur)`.
pub fn fit_rect(
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
    rotation: u16,
) -> (usize, usize, usize, usize) {
    let (rw, rh) = if rotation.is_multiple_of(180) {
        (sw, sh)
    } else {
        (sh, sw)
    };
    if rw == 0 || rh == 0 || dw == 0 || dh == 0 {
        return (0, 0, 0, 0);
    }
    let (ow, oh) = if dw * rh <= dh * rw {
        (dw, (dw * rh / rw).max(1))
    } else {
        ((dh * rw / rh).max(1), dh)
    };
    ((dw - ow) / 2, (dh - oh) / 2, ow, oh)
}

/// Recopie une image RGBA dans un tampon `0RGB`, mise à l'échelle (plus proche
/// voisin), centrée, avec rotation de 0, 90, 180 ou 270° dans le sens horaire.
pub fn blit(
    src: &[u8],
    sw: usize,
    sh: usize,
    dst: &mut [u32],
    dw: usize,
    dh: usize,
    rotation: u16,
) {
    const BACKGROUND: u32 = 0x0010_1216;
    dst.fill(BACKGROUND);
    if src.len() < sw * sh * 4 || dst.len() < dw * dh {
        return;
    }
    let rotation = rotation % 360;
    let (ox, oy, ow, oh) = fit_rect(sw, sh, dw, dh, rotation);
    if ow == 0 || oh == 0 {
        return;
    }
    let (rw, rh) = if rotation.is_multiple_of(180) {
        (sw, sh)
    } else {
        (sh, sw)
    };
    // Coordonnées dans l'image tournée pour chaque colonne de destination.
    let us: Vec<usize> = (0..ow).map(|x| (x * rw / ow).min(rw - 1)).collect();
    for y in 0..oh {
        let v = (y * rh / oh).min(rh - 1);
        let row = &mut dst[(oy + y) * dw + ox..(oy + y) * dw + ox + ow];
        for (px, &u) in row.iter_mut().zip(&us) {
            let (sx, sy) = match rotation {
                90 => (v, sh - 1 - u),
                180 => (sw - 1 - u, sh - 1 - v),
                270 => (sw - 1 - v, u),
                _ => (u, v),
            };
            let i = (sy * sw + sx) * 4;
            *px = (u32::from(src[i]) << 16) | (u32::from(src[i + 1]) << 8) | u32::from(src[i + 2]);
        }
    }
}

/// Convertit un tampon `0RGB` en octets RGBA opaques.
pub fn to_rgba(buf: &[u32]) -> Vec<u8> {
    buf.iter()
        .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255])
        .collect()
}

/// Image affichée à l'arrêt (`--on-exit blank`) : fond noir et message discret.
pub fn stopped_frame(width: u32, height: u32, when: &str) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut buf = vec![0u32; w * h];
    let mut painter = TextPainter::new();
    let title = lightning_i18n::tr("tableau de bord arrêté");
    let tw = painter.measure(title, 44.0);
    painter.draw(
        &mut buf,
        w,
        h,
        (w as f32 - tw) / 2.0,
        h as f32 / 2.0,
        44.0,
        (120, 130, 148),
        title,
    );
    let sw = painter.measure(when, 24.0);
    painter.draw(
        &mut buf,
        w,
        h,
        (w as f32 - sw) / 2.0,
        h as f32 / 2.0 + 48.0,
        24.0,
        (80, 88, 104),
        when,
    );
    to_rgba(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Image 4×2 dont chaque pixel encode sa position : R = x, G = y.
    fn src() -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..2u8 {
            for x in 0..4u8 {
                v.extend_from_slice(&[x, y, 0, 255]);
            }
        }
        v
    }

    fn at(dst: &[u32], dw: usize, x: usize, y: usize) -> (u32, u32) {
        let p = dst[y * dw + x];
        ((p >> 16) & 0xFF, (p >> 8) & 0xFF)
    }

    #[test]
    fn fit_keeps_aspect_ratio_and_centres() {
        assert_eq!(fit_rect(1280, 800, 1280, 800, 0), (0, 0, 1280, 800));
        assert_eq!(fit_rect(1280, 800, 640, 800, 0), (0, 200, 640, 400));
        assert_eq!(fit_rect(1280, 800, 1600, 500, 0), (400, 0, 800, 500));
        assert_eq!(fit_rect(1280, 800, 800, 1280, 90), (0, 0, 800, 1280));
        assert_eq!(fit_rect(1280, 800, 0, 10, 0), (0, 0, 0, 0));
    }

    #[test]
    fn blit_identity_and_rotations() {
        let s = src();
        let mut dst = vec![0u32; 8];
        blit(&s, 4, 2, &mut dst, 4, 2, 0);
        assert_eq!(at(&dst, 4, 0, 0), (0, 0));
        assert_eq!(at(&dst, 4, 3, 1), (3, 1));

        blit(&s, 4, 2, &mut dst, 4, 2, 180);
        assert_eq!(at(&dst, 4, 0, 0), (3, 1));
        assert_eq!(at(&dst, 4, 3, 1), (0, 0));

        // 90° horaire : le coin haut-gauche de la source arrive en haut à droite.
        let mut tall = vec![0u32; 8];
        blit(&s, 4, 2, &mut tall, 2, 4, 90);
        assert_eq!(at(&tall, 2, 1, 0), (0, 0));
        assert_eq!(at(&tall, 2, 0, 0), (0, 1));
        assert_eq!(at(&tall, 2, 1, 3), (3, 0));

        // 270° : le coin haut-gauche arrive en bas à gauche.
        blit(&s, 4, 2, &mut tall, 2, 4, 270);
        assert_eq!(at(&tall, 2, 0, 3), (0, 0));
        assert_eq!(at(&tall, 2, 0, 0), (3, 0));
    }

    #[test]
    fn blit_scales_and_letterboxes() {
        let s = src();
        let mut dst = vec![0xFFFF_FFFFu32; 8 * 8];
        blit(&s, 4, 2, &mut dst, 8, 8, 0);
        // Image 8×4 centrée verticalement : bandes de fond en haut et en bas.
        assert_eq!(dst[0], 0x0010_1216);
        assert_eq!(at(&dst, 8, 0, 2), (0, 0));
        assert_eq!(at(&dst, 8, 7, 5), (3, 1));
        assert_eq!(dst[7 * 8 + 7], 0x0010_1216);
        // Tampons incohérents : pas de panique.
        blit(&s[..4], 4, 2, &mut dst, 8, 8, 0);
        blit(&s, 4, 2, &mut dst, 0, 0, 0);
    }

    #[test]
    fn overlay_and_text_touch_pixels() {
        let (w, h) = (400usize, 120usize);
        let mut buf = vec![0x00FF_FFFFu32; w * h];
        let mut painter = TextPainter::new();
        painter.overlay(
            &mut buf,
            w,
            h,
            &["2,0 img/s".to_owned(), "écran : connecté".to_owned()],
        );
        assert_ne!(buf[5 * w + 5], 0x00FF_FFFF, "fond du bandeau assombri");
        assert!(
            buf.iter().any(|p| *p > 0x00C0_C0C0 && *p != 0x00FF_FFFF),
            "texte clair dessiné"
        );
        assert_eq!(
            buf[(h - 1) * w + w - 1],
            0x00FF_FFFF,
            "reste de l'image intact"
        );
        assert!(painter.measure("abc", 15.0) > 10.0);
    }

    #[test]
    fn stopped_frame_is_dark_with_text() {
        let rgba = stopped_frame(1280, 800, "14/11/2026 13:00");
        assert_eq!(rgba.len(), 1280 * 800 * 4);
        assert_eq!(&rgba[..4], &[0, 0, 0, 255]);
        assert!(rgba.as_chunks::<4>().0.iter().filter(|p| p[0] > 60).count() > 500);
    }
}
