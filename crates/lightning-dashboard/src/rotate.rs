//! Rotation logicielle de l'image envoyée à l'écran (1280×800).
//!
//! Convention : `rotation` est l'angle, dans le sens des aiguilles d'une montre,
//! dont l'image est tournée avant l'envoi. À 90 et 270°, le contenu est composé
//! en portrait (800×1280) puis tourné pour occuper l'écran paysage.

use crate::render::{HEIGHT, WIDTH};

/// Angles pris en charge.
pub const ROTATIONS: [u16; 4] = [0, 90, 180, 270];

/// Vrai si l'angle est pris en charge.
pub fn is_valid(rotation: u16) -> bool {
    ROTATIONS.contains(&rotation)
}

/// Vrai pour 90 et 270° (contenu composé en portrait).
pub fn is_portrait(rotation: u16) -> bool {
    matches!(rotation, 90 | 270)
}

/// Taille du canevas sur lequel composer le contenu, avant rotation.
pub fn canvas(rotation: u16) -> (u32, u32) {
    if is_portrait(rotation) {
        (HEIGHT, WIDTH)
    } else {
        (WIDTH, HEIGHT)
    }
}

/// Tourne une image RGBA `w` × `h` d'un quart de tour (sens horaire si
/// `clockwise`) ; le résultat mesure `h` × `w`.
pub fn quarter_turn(src: &[u8], w: u32, h: u32, clockwise: bool) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    assert_eq!(src.len(), w * h * 4, "taille RGBA incohérente");
    let (dw, dh) = (h, w);
    let mut out = vec![0u8; src.len()];
    let pixels = src.as_chunks::<4>().0;
    let dest = out.as_chunks_mut::<4>().0;
    for y in 0..dh {
        for x in 0..dw {
            // Sens horaire : le pixel source (sx, sy) arrive en (h-1-sy, sx).
            let (sx, sy) = if clockwise {
                (y, h - 1 - x)
            } else {
                (w - 1 - y, x)
            };
            dest[y * dw + x] = pixels[sy * w + sx];
        }
    }
    out
}

/// Applique la rotation à une image composée sur [`canvas`]`(rotation)` ;
/// renvoie une image de [`WIDTH`] × [`HEIGHT`].
pub fn to_screen(mut rgba: Vec<u8>, rotation: u16) -> Vec<u8> {
    let (w, h) = canvas(rotation);
    match rotation {
        90 => quarter_turn(&rgba, w, h, true),
        270 => quarter_turn(&rgba, w, h, false),
        180 => {
            rgba.as_chunks_mut::<4>().0.reverse();
            rgba
        }
        _ => rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(data: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [data[i], data[i + 1], data[i + 2], data[i + 3]]
    }

    /// Image où chaque pixel encode sa position.
    fn coded(w: u32, h: u32) -> Vec<u8> {
        (0..h)
            .flat_map(|y| {
                (0..w).flat_map(move |x| [x as u8, y as u8, (x >> 8) as u8, (y >> 8) as u8])
            })
            .collect()
    }

    #[test]
    fn quarter_turns() {
        // 3 × 2 :  a b c      sens horaire :  d a     sens inverse :  c f
        //          d e f                      e b                     b e
        //                                     f c                     a d
        let src = coded(3, 2);
        let cw = quarter_turn(&src, 3, 2, true);
        assert_eq!(px(&cw, 2, 0, 0), px(&src, 3, 0, 1), "d en haut à gauche");
        assert_eq!(px(&cw, 2, 1, 0), px(&src, 3, 0, 0), "a en haut à droite");
        assert_eq!(px(&cw, 2, 1, 2), px(&src, 3, 2, 0), "c en bas à droite");
        let ccw = quarter_turn(&src, 3, 2, false);
        assert_eq!(px(&ccw, 2, 0, 0), px(&src, 3, 2, 0), "c en haut à gauche");
        assert_eq!(px(&ccw, 2, 1, 2), px(&src, 3, 0, 1), "d en bas à droite");
        // Quatre quarts de tour : retour au départ ; horaire puis inverse aussi.
        let mut back = src.clone();
        let (mut w, mut h) = (3, 2);
        for _ in 0..4 {
            back = quarter_turn(&back, w, h, true);
            (w, h) = (h, w);
        }
        assert_eq!(back, src);
        assert_eq!(quarter_turn(&cw, 2, 3, false), src);
    }

    #[test]
    fn canvases_and_screen_output() {
        assert_eq!(canvas(0), (1280, 800));
        assert_eq!(canvas(180), (1280, 800));
        assert_eq!(canvas(90), (800, 1280));
        assert_eq!(canvas(270), (800, 1280));
        assert!(is_valid(270) && !is_valid(45));
        for rotation in ROTATIONS {
            let (w, h) = canvas(rotation);
            let src = coded(w, h);
            let out = to_screen(src.clone(), rotation);
            assert_eq!(out.len(), (WIDTH * HEIGHT * 4) as usize, "{rotation}");
            // Le coin haut gauche du contenu se retrouve au bon coin de l'écran.
            let top_left = px(&src, w, 0, 0);
            let corner = match rotation {
                0 => (0, 0),
                90 => (WIDTH - 1, 0),
                180 => (WIDTH - 1, HEIGHT - 1),
                _ => (0, HEIGHT - 1),
            };
            assert_eq!(px(&out, WIDTH, corner.0, corner.1), top_left, "{rotation}");
        }
    }
}
