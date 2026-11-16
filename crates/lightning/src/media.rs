//! Chargement des contenus : couleur, image, vidéo (ffmpeg).

use std::path::Path;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};
use image::imageops::FilterType;
use lightning_ipc::Fit;

/// Analyse une couleur `RRGGBB` (avec ou sans `#`).
pub fn parse_color(text: &str) -> Result<[u8; 3]> {
    let hex = text.trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        bail!(lightning_i18n::tr("couleur attendue au format RRGGBB"));
    }
    let mut rgb = [0u8; 3];
    for (i, channel) in rgb.iter_mut().enumerate() {
        *channel = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
            .context(lightning_i18n::tr("couleur attendue au format RRGGBB"))?;
    }
    Ok(rgb)
}

/// Image RGBA unie.
pub fn solid(rgb: [u8; 3], width: u32, height: u32) -> Vec<u8> {
    [rgb[0], rgb[1], rgb[2], 255].repeat((width * height) as usize)
}

/// Charge une image et l'ajuste à l'écran ; renvoie des pixels RGBA.
pub fn load_image(path: &Path, fit: Fit, width: u32, height: u32) -> Result<Vec<u8>> {
    let img = if crate::heif::is_heif(path) {
        crate::heif::decode(path)
            .map_err(anyhow::Error::msg)
            .with_context(|| {
                lightning_i18n::trf("décodage de {path}", &[("path", &path.display())])
            })?
    } else {
        image::ImageReader::open(path)
            .with_context(|| {
                lightning_i18n::trf("ouverture de {path}", &[("path", &path.display())])
            })?
            .with_guessed_format()?
            .decode()
            .with_context(|| {
                lightning_i18n::trf("décodage de {path}", &[("path", &path.display())])
            })?
    };
    let filter = FilterType::CatmullRom;
    let fitted = match fit {
        Fit::Cover => img.resize_to_fill(width, height, filter).into_rgba8(),
        Fit::Stretch => img.resize_exact(width, height, filter).into_rgba8(),
        Fit::Contain => {
            let small = img.resize(width, height, filter).into_rgba8();
            let mut canvas =
                image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
            let x = i64::from((width - small.width()) / 2);
            let y = i64::from((height - small.height()) / 2);
            image::imageops::overlay(&mut canvas, &small, x, y);
            canvas
        }
    };
    Ok(fitted.into_raw())
}

/// Retourne une image RGBA de 180° (sur place).
pub fn rotate_180(rgba: &mut [u8]) {
    let (pixels, _) = rgba.as_chunks_mut::<4>();
    pixels.reverse();
}

/// Lance ffmpeg pour convertir `file` en flux MJPEG à la taille de l'écran.
pub fn spawn_ffmpeg(
    file: &str,
    fps: f32,
    looped: bool,
    rotate_180: bool,
    width: u32,
    height: u32,
) -> Result<Child> {
    let (w, h) = (width, height);
    let mut filter = format!(
        "scale={w}:{h}:force_original_aspect_ratio=decrease,\
         pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={fps},format=yuvj420p"
    );
    if rotate_180 {
        filter.push_str(",hflip,vflip");
    }
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
    if looped {
        cmd.args(["-stream_loop", "-1"]);
    }
    cmd.args(["-re", "-i", file, "-an", "-vf", &filter])
        .args([
            "-c:v", "mjpeg", "-q:v", "5", "-huffman", "default", "-f", "mjpeg", "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    cmd.spawn().context(lightning_i18n::tr(
        "lancement de ffmpeg (est-il installé ?)",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("ff8000").unwrap(), [255, 128, 0]);
        assert_eq!(parse_color("#00FF7f").unwrap(), [0, 255, 127]);
        assert!(parse_color("fff").is_err());
        assert!(parse_color("gg0000").is_err());
        assert!(parse_color("ééé").is_err());
    }

    #[test]
    fn solid_and_rotation() {
        let mut img = solid([1, 2, 3], 2, 1);
        assert_eq!(img, [1, 2, 3, 255, 1, 2, 3, 255]);
        img[0] = 9;
        rotate_180(&mut img);
        assert_eq!(img, [1, 2, 3, 255, 9, 2, 3, 255]);
    }

    #[test]
    fn images_are_fitted_to_the_screen() {
        let dir = std::env::temp_dir().join(format!("lightning-media-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rouge.png");
        image::RgbImage::from_pixel(40, 10, image::Rgb([255, 0, 0]))
            .save(&path)
            .unwrap();
        for fit in [Fit::Cover, Fit::Contain, Fit::Stretch] {
            let rgba = load_image(&path, fit, 80, 50).unwrap();
            assert_eq!(rgba.len(), 80 * 50 * 4);
            let centre = &rgba[(25 * 80 + 40) * 4..][..4];
            assert_eq!(centre, [255, 0, 0, 255], "{fit}");
            let corner = &rgba[..4];
            let expected = if fit == Fit::Contain {
                [0, 0, 0, 255]
            } else {
                [255, 0, 0, 255]
            };
            assert_eq!(corner, expected, "{fit}");
        }
        assert!(load_image(&dir.join("absent.png"), Fit::Cover, 8, 8).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
