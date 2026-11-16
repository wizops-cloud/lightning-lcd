//! Photos HEIC / HEIF (téléphones), décodées par libheif.
//!
//! Le décodage n'existe qu'avec la fonctionnalité Cargo `heic` (bibliothèque
//! système `libheif` requise à la construction). Sans elle, ces fichiers sont
//! ignorés par le diaporama et refusés avec un message clair en mode image.

use std::path::Path;

use image::DynamicImage;

/// Extensions des fichiers HEIF.
const EXTENSIONS: [&str; 2] = ["heic", "heif"];

/// Vrai si ce programme sait décoder les photos HEIC.
pub const SUPPORTED: bool = cfg!(feature = "heic");

/// Vrai si le fichier porte une extension HEIF.
pub fn is_heif(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Décode l'image principale d'un fichier HEIF, rotation et miroir du
/// conteneur appliqués (l'orientation EXIF ne doit pas l'être en plus).
#[cfg(feature = "heic")]
pub fn decode(path: &Path) -> Result<DynamicImage, String> {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

    let name = path.to_str().ok_or_else(|| {
        lightning_i18n::trf("chemin non UTF-8 : {path}", &[("path", &path.display())])
    })?;
    let context = HeifContext::read_from_file(name).map_err(|e| e.to_string())?;
    let handle = context.primary_image_handle().map_err(|e| e.to_string())?;
    let image = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .map_err(|e| e.to_string())?;
    let planes = image.planes();
    let plane = planes
        .interleaved
        .ok_or(lightning_i18n::tr("image HEIF sans plan RGBA entrelacé"))?;
    let (width, height) = (plane.width, plane.height);
    let row = width as usize * 4;
    let mut rgba = Vec::with_capacity(row * height as usize);
    for line in plane.data.chunks(plane.stride).take(height as usize) {
        rgba.extend_from_slice(
            line.get(..row)
                .ok_or(lightning_i18n::tr("plan HEIF tronqué"))?,
        );
    }
    image::RgbaImage::from_raw(width, height, rgba)
        .map(DynamicImage::ImageRgba8)
        .ok_or_else(|| lightning_i18n::tr("plan HEIF tronqué").to_owned())
}

/// Sans la fonctionnalité `heic` : erreur explicite.
#[cfg(not(feature = "heic"))]
pub fn decode(_path: &Path) -> Result<DynamicImage, String> {
    Err("photo HEIC non prise en charge par cette version \
         (reconstruire avec « --features heic », libheif requise)"
        .to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Photo d'exemple créée pour ce dépôt (96 × 64, moitié gauche rouge,
    /// moitié droite bleue, date EXIF 2024-06-21 18:30:00) ; même licence que le code.
    pub const SAMPLE: &[u8] = include_bytes!("../tests/data/exemple.heic");

    #[test]
    fn extensions() {
        assert!(is_heif(Path::new("/a/IMG_0001.HEIC")));
        assert!(is_heif(Path::new("b.heif")));
        assert!(!is_heif(Path::new("c.jpg")));
        assert!(!is_heif(Path::new("heic")));
    }

    #[cfg(feature = "heic")]
    #[test]
    fn decodes_the_sample() {
        let dir = std::env::temp_dir().join(format!("lightning-heif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("exemple.heic");
        std::fs::write(&path, SAMPLE).unwrap();
        let img = decode(&path).unwrap().into_rgba8();
        assert_eq!(img.dimensions(), (96, 64));
        let near = |p: &image::Rgba<u8>, c: [u8; 3]| {
            p.0[..3].iter().zip(c).all(|(a, b)| a.abs_diff(b) < 24)
        };
        assert!(
            near(img.get_pixel(10, 30), [220, 40, 40]),
            "{:?}",
            img.get_pixel(10, 30)
        );
        assert!(
            near(img.get_pixel(85, 30), [40, 60, 220]),
            "{:?}",
            img.get_pixel(85, 30)
        );
        assert!(decode(&dir.join("absent.heic")).is_err());
        std::fs::write(dir.join("faux.heic"), b"pas une photo").unwrap();
        assert!(decode(&dir.join("faux.heic")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(feature = "heic"))]
    #[test]
    fn explains_the_missing_feature() {
        let err = decode(Path::new("/x/photo.heic")).unwrap_err();
        assert!(err.contains("--features heic"), "{err}");
    }
}
