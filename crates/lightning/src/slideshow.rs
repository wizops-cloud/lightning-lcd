//! Mode diaporama : photos de dossiers locaux, décodées en avance dans un fil.
//!
//! [`Slideshow::poll`] est appelé régulièrement par le moteur ; il renvoie une
//! image à diffuser quand quelque chose a changé (nouvelle photo, étape de
//! fondu, horloge). Tout le travail lourd (parcours des dossiers, décodage,
//! composition) se fait dans le fil `slideshow`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Local, NaiveDateTime, Timelike};
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, RgbaImage};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::text::{self, TextPainter};

/// Réglages du diaporama (section `[slideshow]`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SlideshowConfig {
    /// Dossiers parcourus récursivement.
    pub folders: Vec<String>,
    /// Durée d'affichage de chaque photo, en secondes.
    pub interval_s: f64,
    /// `random` ou `chronological` (date de prise de vue, sinon date du fichier).
    pub order: Order,
    /// `cover` (remplir en rognant) ou `contain` (photo entière sur fond flou).
    pub fit: SlideFit,
    /// Durée du fondu enchaîné, en millisecondes ; 0 = changement sec.
    pub transition_ms: u32,
    /// Afficher la date et l'album (nom du dossier) de la photo.
    pub overlay: bool,
    /// Afficher une petite horloge.
    pub clock: bool,
}

impl Default for SlideshowConfig {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            interval_s: 15.0,
            order: Order::Random,
            fit: SlideFit::Contain,
            transition_ms: 600,
            overlay: true,
            clock: true,
        }
    }
}

impl SlideshowConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.interval_s.is_finite() || !(1.0..=86_400.0).contains(&self.interval_s) {
            return Err(lightning_i18n::tr(
                "durée par photo attendue entre 1 seconde et 24 heures",
            )
            .into());
        }
        if self.transition_ms > 5000 {
            return Err(lightning_i18n::tr("fondu de 5 secondes au maximum").into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    #[default]
    Random,
    Chronological,
}

impl Order {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Random => "random",
            Self::Chronological => "chronological",
        }
    }
}

impl std::str::FromStr for Order {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "random" => Ok(Self::Random),
            "chronological" => Ok(Self::Chronological),
            other => Err(lightning_i18n::trf(
                "ordre « {value} » inconnu (attendu : random, chronological)",
                &[("value", &other)],
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SlideFit {
    Cover,
    #[default]
    Contain,
}

impl SlideFit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Contain => "contain",
        }
    }
}

impl std::str::FromStr for SlideFit {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "cover" => Ok(Self::Cover),
            "contain" => Ok(Self::Contain),
            other => Err(lightning_i18n::trf(
                "ajustement « {value} » inconnu (attendu : cover, contain)",
                &[("value", &other)],
            )),
        }
    }
}

/// Une photo trouvée dans les dossiers.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    /// Date de prise de vue (EXIF) ou, à défaut, de modification du fichier.
    pub date: Option<NaiveDateTime>,
}

const EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "webp", "bmp"];

fn is_photo(path: &Path) -> bool {
    if crate::heif::is_heif(path) {
        return crate::heif::SUPPORTED;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 12 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        match entry.file_type() {
            Ok(t) if t.is_dir() && !hidden => walk(&path, depth + 1, out),
            Ok(t) if (t.is_file() || t.is_symlink()) && !hidden && is_photo(&path) => {
                out.push(path)
            }
            _ => {}
        }
    }
}

/// Date de prise de vue lue dans l'EXIF (`DateTimeOriginal`).
fn exif_date(path: &Path) -> Option<NaiveDateTime> {
    let file = std::fs::File::open(path).ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()?;
    let field = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)?;
    let text = field.display_value().to_string();
    NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M:%S").ok()
}

fn file_date(path: &Path) -> Option<NaiveDateTime> {
    let modified: SystemTime = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(DateTime::<Local>::from(modified).naive_local())
}

/// Parcourt les dossiers ; les dates EXIF ne sont lues que si l'ordre en dépend.
pub fn scan(folders: &[String], with_dates: bool) -> Vec<Entry> {
    let mut paths = Vec::new();
    for folder in folders {
        walk(Path::new(folder), 0, &mut paths);
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| {
            let date = with_dates
                .then(|| exif_date(&path).or_else(|| file_date(&path)))
                .flatten();
            Entry { path, date }
        })
        .collect()
}

/// Ordre de passage : indices dans la liste des photos.
pub fn playlist(entries: &[Entry], order: Order, seed: u64) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..entries.len()).collect();
    match order {
        Order::Chronological => {
            indices.sort_by(|a, b| {
                (entries[*a].date, &entries[*a].path).cmp(&(entries[*b].date, &entries[*b].path))
            });
        }
        Order::Random => {
            // Mélange de Fisher-Yates avec un xorshift : pas besoin de mieux ici.
            let mut state = seed | 1;
            for i in (1..indices.len()).rev() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                indices.swap(i, (state % (i as u64 + 1)) as usize);
            }
        }
    }
    indices
}

/// Date lisible dans la langue de l'interface (« 14 juillet 2024 », « July 14, 2024 »).
pub fn french_date(date: NaiveDateTime) -> String {
    lightning_dashboard::render::day_date(lightning_i18n::lang(), date.date())
}

/// Photo composée à la taille de l'écran, sans les incrustations.
pub struct Slide {
    pub path: PathBuf,
    pub rgba: Vec<u8>,
    /// Légende : date de prise de vue et album (nom du dossier).
    pub caption: String,
}

/// Décode une photo en respectant son orientation EXIF.
fn decode(path: &Path) -> Result<DynamicImage, String> {
    if crate::heif::is_heif(path) {
        return crate::heif::decode(path);
    }
    decode_common(path).map_err(|e| e.to_string())
}

fn decode_common(path: &Path) -> image::ImageResult<DynamicImage> {
    let mut decoder = image::ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Ajuste une photo à l'écran : `Cover` rogne, `Contain` pose la photo entière
/// sur une version floue et assombrie d'elle-même.
pub fn compose(img: &DynamicImage, fit: SlideFit, width: u32, height: u32) -> Vec<u8> {
    match fit {
        SlideFit::Cover => img
            .resize_to_fill(width, height, FilterType::CatmullRom)
            .into_rgba8()
            .into_raw(),
        SlideFit::Contain => {
            let front = img
                .resize(width, height, FilterType::CatmullRom)
                .into_rgba8();
            if front.dimensions() == (width, height) {
                return front.into_raw();
            }
            // Fond : la photo en tout petit, floutée, puis agrandie (coût négligeable).
            let small = img.resize_to_fill(width / 8, height / 8, FilterType::Triangle);
            let blurred = small.blur(3.0);
            let mut back: RgbaImage = blurred
                .resize_exact(width, height, FilterType::Triangle)
                .into_rgba8();
            for p in back.pixels_mut() {
                p.0 = [p.0[0] / 2, p.0[1] / 2, p.0[2] / 2, 255];
            }
            let x = i64::from((width - front.width()) / 2);
            let y = i64::from((height - front.height()) / 2);
            image::imageops::overlay(&mut back, &front, x, y);
            back.into_raw()
        }
    }
}

fn load(entry: &Entry, fit: SlideFit, width: u32, height: u32) -> Result<Slide, String> {
    let img = decode(&entry.path).map_err(|e| format!("{} : {e}", entry.path.display()))?;
    let date = entry
        .date
        .or_else(|| exif_date(&entry.path))
        .or_else(|| file_date(&entry.path));
    let album = entry
        .path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let caption = match (date.map(french_date), album.is_empty()) {
        (Some(d), false) => format!("{d} · {album}"),
        (Some(d), true) => d,
        (None, _) => album,
    };
    Ok(Slide {
        path: entry.path.clone(),
        rgba: compose(&img, fit, width, height),
        caption,
    })
}

enum Job {
    Scan {
        folders: Vec<String>,
        with_dates: bool,
    },
    Load {
        entry: Entry,
        fit: SlideFit,
    },
}

enum Msg {
    Scanned(Vec<Entry>),
    Loaded(Box<Result<Slide, String>>),
}

fn worker(jobs: Receiver<Job>, results: Sender<Msg>, width: u32, height: u32) {
    while let Ok(job) = jobs.recv() {
        let msg = match job {
            Job::Scan {
                folders,
                with_dates,
            } => Msg::Scanned(scan(&folders, with_dates)),
            Job::Load { entry, fit } => Msg::Loaded(Box::new(load(&entry, fit, width, height))),
        };
        if results.send(msg).is_err() {
            break;
        }
    }
}

const RESCAN_PERIOD: Duration = Duration::from_secs(60);
const FADE_STEP: Duration = Duration::from_millis(50);

struct Fade {
    from: Vec<u8>,
    started: Instant,
}

/// État du diaporama, piloté par [`Slideshow::poll`].
pub struct Slideshow {
    cfg: SlideshowConfig,
    width: u32,
    height: u32,
    jobs: Sender<Job>,
    results: Receiver<Msg>,
    entries: Vec<Entry>,
    order: Vec<usize>,
    /// Position de la prochaine photo à charger dans `order`.
    cursor: usize,
    seed: u64,
    current: Option<Slide>,
    next: Option<Slide>,
    loading: bool,
    scanned_once: bool,
    last_scan: Instant,
    shown_at: Instant,
    fade: Option<Fade>,
    last_fade_step: Instant,
    /// Minute affichée par l'horloge de la dernière image produite.
    drawn_minute: Option<u32>,
    skip: bool,
    painter: TextPainter,
    failures: u32,
}

impl Slideshow {
    pub fn new(cfg: SlideshowConfig, width: u32, height: u32) -> Self {
        let (jobs, jobs_rx) = mpsc::channel();
        let (results_tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("slideshow".into())
            .spawn(move || worker(jobs_rx, results_tx, width, height))
            .expect("création du fil du diaporama");
        let now = Instant::now();
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        let mut show = Self {
            cfg,
            width,
            height,
            jobs,
            results,
            entries: Vec::new(),
            order: Vec::new(),
            cursor: 0,
            seed,
            current: None,
            next: None,
            loading: false,
            scanned_once: false,
            last_scan: now,
            shown_at: now,
            fade: None,
            last_fade_step: now,
            drawn_minute: None,
            skip: false,
            painter: TextPainter::new(),
            failures: 0,
        };
        show.request_scan();
        show
    }

    /// Nombre de photos trouvées.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Photo actuellement affichée.
    pub fn current_path(&self) -> Option<&Path> {
        self.current.as_ref().map(|s| s.path.as_path())
    }

    /// Passe à la photo suivante dès qu'elle est prête.
    pub fn skip(&mut self) {
        self.skip = true;
    }

    fn request_scan(&mut self) {
        self.last_scan = Instant::now();
        let _ = self.jobs.send(Job::Scan {
            folders: self.cfg.folders.clone(),
            with_dates: self.cfg.order == Order::Chronological,
        });
    }

    fn request_next(&mut self) {
        if self.loading || self.next.is_some() || self.order.is_empty() {
            return;
        }
        if self.cursor >= self.order.len() {
            // Fin de liste : on repart, avec un nouveau mélange en ordre aléatoire.
            self.cursor = 0;
            self.seed = self
                .seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.order = playlist(&self.entries, self.cfg.order, self.seed);
        }
        let entry = self.entries[self.order[self.cursor]].clone();
        self.cursor += 1;
        self.loading = true;
        let _ = self.jobs.send(Job::Load {
            entry,
            fit: self.cfg.fit,
        });
    }

    fn on_scanned(&mut self, entries: Vec<Entry>) {
        let first = !self.scanned_once;
        self.scanned_once = true;
        if !first && entries == self.entries {
            return;
        }
        info!(photos = entries.len(), "diaporama : dossiers parcourus");
        self.entries = entries;
        self.order = playlist(&self.entries, self.cfg.order, self.seed);
        // On garde la photo en cours ; la suite repart de la nouvelle liste.
        self.cursor = match (&self.current, self.cfg.order) {
            (Some(slide), Order::Chronological) => self
                .order
                .iter()
                .position(|i| self.entries[*i].path == slide.path)
                .map_or(0, |p| p + 1),
            _ => 0,
        };
    }

    /// Incrustations (légende, horloge) dessinées sur une copie de l'image.
    fn decorate(&mut self, mut rgba: Vec<u8>, caption: &str, now: DateTime<Local>) -> Vec<u8> {
        if !(self.cfg.overlay && !caption.is_empty()) && !self.cfg.clock {
            return rgba;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let (pixels, _) = rgba.as_chunks::<4>();
        let mut buf: Vec<u32> = pixels
            .iter()
            .map(|p| (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]))
            .collect();
        if self.cfg.overlay && !caption.is_empty() {
            let size = 26.0;
            let tw = self.painter.measure(caption, size) as usize;
            let (bx, by, bh) = (24usize, h.saturating_sub(74), 50usize);
            text::fill(&mut buf, w, h, bx, by, tw + 32, bh, (0, 0, 0), 150);
            self.painter.draw(
                &mut buf,
                w,
                h,
                (bx + 16) as f32,
                (by + 35) as f32,
                size,
                (240, 243, 248),
                caption,
            );
        }
        if self.cfg.clock {
            let clock = now.format("%H:%M").to_string();
            let size = 40.0;
            let tw = self.painter.measure(&clock, size) as usize;
            let bx = w.saturating_sub(tw + 56);
            text::fill(&mut buf, w, h, bx, 24, tw + 32, 62, (0, 0, 0), 150);
            self.painter.draw(
                &mut buf,
                w,
                h,
                (bx + 16) as f32,
                69.0,
                size,
                (240, 243, 248),
                &clock,
            );
        }
        rgba = text::to_rgba(&buf);
        rgba
    }

    /// Fait avancer le diaporama ; renvoie une image s'il y a du nouveau à diffuser.
    pub fn poll(&mut self, now: Instant) -> Option<Vec<u8>> {
        while let Ok(msg) = self.results.try_recv() {
            match msg {
                Msg::Scanned(entries) => self.on_scanned(entries),
                Msg::Loaded(result) => {
                    self.loading = false;
                    match *result {
                        Ok(slide) => {
                            self.failures = 0;
                            self.next = Some(slide);
                        }
                        Err(e) => {
                            // Photo illisible : on passe à la suivante, sans boucler à vide.
                            self.failures += 1;
                            if self.failures <= self.order.len().max(1) as u32 {
                                debug!("diaporama : {e}");
                            } else if self.failures == self.order.len() as u32 + 1 {
                                warn!("diaporama : aucune photo lisible ({e})");
                            }
                        }
                    }
                }
            }
        }
        if now.duration_since(self.last_scan) >= RESCAN_PERIOD {
            self.request_scan();
        }
        if !self.scanned_once {
            return None;
        }
        if self.entries.is_empty() {
            return self.placeholder();
        }
        if self.failures <= self.order.len() as u32 {
            self.request_next();
        }

        let wall = Local::now();
        let due = self.current.is_none()
            || self.skip
            || now.duration_since(self.shown_at).as_secs_f64() >= self.cfg.interval_s;
        let mut changed = false;
        if due && self.fade.is_none() && self.next.is_some() {
            let previous = self.current.take();
            self.current = self.next.take();
            self.shown_at = now;
            self.skip = false;
            changed = true;
            if let (Some(prev), true) = (previous, self.cfg.transition_ms > 0) {
                self.fade = Some(Fade {
                    from: prev.rgba,
                    started: now,
                });
                self.last_fade_step = now - FADE_STEP;
            }
        }
        let slide = self.current.as_ref()?;
        let caption = slide.caption.clone();

        if let Some(fade) = &self.fade {
            if now.duration_since(self.last_fade_step) < FADE_STEP {
                return None;
            }
            self.last_fade_step = now;
            let t = now.duration_since(fade.started).as_secs_f32() * 1000.0
                / self.cfg.transition_ms as f32;
            if t >= 1.0 {
                self.fade = None;
            } else {
                let blended = blend(&fade.from, &slide.rgba, t);
                self.drawn_minute = Some(wall.minute());
                return Some(self.decorate(blended, &caption, wall));
            }
            changed = true;
        }
        let minute_changed = self.cfg.clock && self.drawn_minute != Some(wall.minute());
        if changed || minute_changed {
            self.drawn_minute = Some(wall.minute());
            let rgba = slide.rgba.clone();
            return Some(self.decorate(rgba, &caption, wall));
        }
        None
    }

    /// Message affiché quand aucun dossier ne contient de photo.
    fn placeholder(&mut self) -> Option<Vec<u8>> {
        if self.drawn_minute == Some(u32::MAX) {
            return None;
        }
        self.drawn_minute = Some(u32::MAX);
        let (w, h) = (self.width as usize, self.height as usize);
        let mut buf = vec![0x0009_0B10u32; w * h];
        let (title, hint) = if self.cfg.folders.is_empty() {
            (
                lightning_i18n::tr("diaporama : aucun dossier choisi"),
                lightning_i18n::tr("ajoutez un dossier de photos dans les réglages"),
            )
        } else {
            (
                lightning_i18n::tr("diaporama : aucune photo trouvée"),
                if crate::heif::SUPPORTED {
                    lightning_i18n::tr("formats pris en charge : JPEG, PNG, WebP, BMP, HEIC")
                } else {
                    lightning_i18n::tr("formats pris en charge : JPEG, PNG, WebP, BMP")
                },
            )
        };
        let tw = self.painter.measure(title, 40.0);
        self.painter.draw(
            &mut buf,
            w,
            h,
            (w as f32 - tw) / 2.0,
            h as f32 / 2.0,
            40.0,
            (190, 198, 212),
            title,
        );
        let hw = self.painter.measure(hint, 22.0);
        self.painter.draw(
            &mut buf,
            w,
            h,
            (w as f32 - hw) / 2.0,
            h as f32 / 2.0 + 44.0,
            22.0,
            (120, 130, 148),
            hint,
        );
        Some(text::to_rgba(&buf))
    }
}

/// Fondu : `from` à `t = 0`, `to` à `t = 1`.
pub fn blend(from: &[u8], to: &[u8], t: f32) -> Vec<u8> {
    let a = (t.clamp(0.0, 1.0) * 256.0) as u32;
    let b = 256 - a;
    from.iter()
        .zip(to)
        .map(|(f, t)| ((u32::from(*f) * b + u32::from(*t) * a) >> 8) as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lightning-slideshow-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png(path: &Path, w: u32, h: u32, rgb: [u8; 3]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbImage::from_pixel(w, h, image::Rgb(rgb))
            .save(path)
            .unwrap();
    }

    fn pixel(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    }

    fn wait_frame(show: &mut Slideshow, what: &str) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(frame) = show.poll(Instant::now()) {
                return frame;
            }
            assert!(Instant::now() < deadline, "délai dépassé : {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn scan_is_recursive_filters_extensions_and_skips_hidden() {
        let dir = tmp("scan");
        png(&dir.join("a.png"), 4, 4, [1, 1, 1]);
        png(&dir.join("vacances/été/b.PNG"), 4, 4, [1, 1, 1]);
        png(&dir.join(".cache/c.png"), 4, 4, [1, 1, 1]);
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(dir.join("vacances/film.mp4"), "x").unwrap();
        let folders = vec![
            dir.to_string_lossy().into_owned(),
            "/nonexistent/photos".to_owned(),
        ];
        let entries = scan(&folders, true);
        let names: Vec<String> = entries
            .iter()
            .map(|e| {
                e.path
                    .strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["a.png", "vacances/été/b.PNG"]);
        assert!(
            entries.iter().all(|e| e.date.is_some()),
            "date du fichier à défaut d'EXIF"
        );
        assert!(scan(&folders, false).iter().all(|e| e.date.is_none()));
        // Le même dossier cité deux fois ne double pas les photos.
        let twice = vec![folders[0].clone(), folders[0].clone()];
        assert_eq!(scan(&twice, false).len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn heic_photos_follow_the_cargo_feature() {
        let dir = tmp("heic");
        std::fs::write(dir.join("IMG_0001.HEIC"), crate::heif::tests::SAMPLE).unwrap();
        png(&dir.join("b.png"), 4, 4, [1, 1, 1]);
        let entries = scan(&[dir.to_string_lossy().into_owned()], true);
        if !crate::heif::SUPPORTED {
            assert_eq!(entries.len(), 1, "HEIC ignoré sans la fonctionnalité heic");
            let _ = std::fs::remove_dir_all(dir);
            return;
        }
        assert_eq!(entries.len(), 2);
        let heic = &entries[0];
        assert!(heic.path.ends_with("IMG_0001.HEIC"));
        // Date de prise de vue lue dans l'EXIF du conteneur HEIF.
        assert_eq!(
            heic.date,
            NaiveDate::from_ymd_opt(2024, 6, 21)
                .unwrap()
                .and_hms_opt(18, 30, 0)
        );
        let slide = load(heic, SlideFit::Cover, 160, 100).unwrap();
        assert_eq!(slide.rgba.len(), 160 * 100 * 4);
        let near = |p: [u8; 3], c: [u8; 3]| p.iter().zip(c).all(|(a, b)| a.abs_diff(b) < 30);
        assert!(near(pixel(&slide.rgba, 160, 10, 50), [220, 40, 40]));
        assert!(near(pixel(&slide.rgba, 160, 150, 50), [40, 60, 220]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn playlists() {
        let date = |d: u32| {
            NaiveDate::from_ymd_opt(2024, 7, d)
                .unwrap()
                .and_hms_opt(12, 0, 0)
        };
        let entries: Vec<Entry> = [(3, "c"), (1, "a"), (2, "b"), (1, "0")]
            .iter()
            .map(|(d, n)| Entry {
                path: PathBuf::from(n),
                date: date(*d),
            })
            .collect();
        assert_eq!(playlist(&entries, Order::Chronological, 0), [3, 1, 2, 0]);
        let random = playlist(&entries, Order::Random, 42);
        let mut sorted = random.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, [0, 1, 2, 3], "permutation complète");
        assert_eq!(
            random,
            playlist(&entries, Order::Random, 42),
            "déterministe à graine égale"
        );
        let many: Vec<Entry> = (0..50)
            .map(|i| Entry {
                path: PathBuf::from(i.to_string()),
                date: None,
            })
            .collect();
        assert_ne!(
            playlist(&many, Order::Random, 1),
            playlist(&many, Order::Random, 2)
        );
        assert!(playlist(&[], Order::Random, 1).is_empty());
    }

    #[test]
    fn dates_in_french() {
        let d = |y, m, day| {
            NaiveDate::from_ymd_opt(y, m, day)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
        };
        assert_eq!(french_date(d(2024, 7, 14)), "14 juillet 2024");
        assert_eq!(french_date(d(2023, 8, 1)), "1er août 2023");
        assert_eq!(french_date(d(2025, 12, 31)), "31 décembre 2025");
    }

    #[test]
    fn compose_cover_and_contain_with_blurred_background() {
        // Photo verticale rouge sur un écran horizontal.
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            100,
            200,
            image::Rgb([200, 0, 0]),
        ));
        let cover = compose(&img, SlideFit::Cover, 160, 100);
        assert_eq!(cover.len(), 160 * 100 * 4);
        assert_eq!(pixel(&cover, 160, 2, 2), [200, 0, 0]);

        let contain = compose(&img, SlideFit::Contain, 160, 100);
        assert_eq!(
            pixel(&contain, 160, 80, 50),
            [200, 0, 0],
            "photo entière au centre"
        );
        let edge = pixel(&contain, 160, 3, 50);
        assert!(
            edge[0] > 60 && edge[0] < 140 && edge[1] < 10,
            "fond flou assombri : {edge:?}"
        );
        assert!(contain.as_chunks::<4>().0.iter().all(|p| p[3] == 255));

        // Mêmes proportions que l'écran : pas de fond à fabriquer.
        let wide = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            320,
            200,
            image::Rgb([0, 0, 200]),
        ));
        assert_eq!(
            pixel(&compose(&wide, SlideFit::Contain, 160, 100), 160, 0, 0),
            [0, 0, 200]
        );
    }

    #[test]
    fn blend_endpoints_and_middle() {
        let (a, b) = (vec![0u8, 100, 255, 255], vec![255u8, 100, 0, 255]);
        assert_eq!(blend(&a, &b, 0.0), a);
        assert_eq!(blend(&a, &b, 1.0), b);
        let mid = blend(&a, &b, 0.5);
        assert!((126..=129).contains(&mid[0]) && mid[1] == 100 && (126..=129).contains(&mid[2]));
    }

    #[test]
    fn slideshow_shows_photos_in_order_and_reloads_when_the_folder_changes() {
        let dir = tmp("run");
        png(&dir.join("1-rouge.png"), 64, 40, [220, 0, 0]);
        png(&dir.join("2-vert.png"), 64, 40, [0, 220, 0]);
        let cfg = SlideshowConfig {
            folders: vec![dir.to_string_lossy().into_owned()],
            interval_s: 1.0,
            order: Order::Chronological,
            fit: SlideFit::Cover,
            transition_ms: 0,
            overlay: false,
            clock: false,
        };
        // Dates de fichier identiques à la seconde près : l'ordre retombe sur le nom.
        let mut show = Slideshow::new(cfg, 160, 100);
        let first = wait_frame(&mut show, "première photo");
        assert_eq!(first.len(), 160 * 100 * 4);
        assert_eq!(show.count(), 2);
        let first_color = pixel(&first, 160, 80, 50);
        assert!(first_color == [220, 0, 0] || first_color == [0, 220, 0]);
        assert!(show.current_path().is_some());

        show.skip();
        let second = wait_frame(&mut show, "photo suivante");
        let second_color = pixel(&second, 160, 80, 50);
        assert_ne!(first_color, second_color);

        // Une photo ajoutée est prise en compte au prochain parcours.
        png(&dir.join("3-bleu.png"), 64, 40, [0, 0, 220]);
        show.request_scan();
        let deadline = Instant::now() + Duration::from_secs(10);
        while show.count() != 3 {
            show.poll(Instant::now());
            assert!(Instant::now() < deadline, "nouveau parcours");
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fade_overlay_clock_and_placeholders() {
        let dir = tmp("fade");
        png(&dir.join("album/a.png"), 64, 40, [255, 255, 255]);
        png(&dir.join("album/b.png"), 64, 40, [0, 0, 0]);
        let cfg = SlideshowConfig {
            folders: vec![dir.to_string_lossy().into_owned()],
            interval_s: 1.0,
            order: Order::Chronological,
            fit: SlideFit::Cover,
            transition_ms: 400,
            overlay: true,
            clock: true,
        };
        let mut show = Slideshow::new(cfg, 320, 200);
        let first = wait_frame(&mut show, "première photo");
        let base = pixel(&first, 320, 160, 100);
        // Légende en bas à gauche et horloge en haut à droite : fonds assombris.
        if base == [255, 255, 255] {
            assert_ne!(
                pixel(&first, 320, 30, 200 - 70),
                [255, 255, 255],
                "bandeau de légende"
            );
            assert_ne!(
                pixel(&first, 320, 320 - 40, 30),
                [255, 255, 255],
                "fond de l'horloge"
            );
        }
        show.skip();
        // Pendant le fondu, le centre passe par des gris intermédiaires.
        let mut saw_intermediate = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(frame) = show.poll(Instant::now()) {
                let c = pixel(&frame, 320, 160, 100)[0];
                if (20..=235).contains(&c) {
                    saw_intermediate = true;
                }
                if saw_intermediate && (c == 0 || c == 255) && c != base[0] {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "fondu jamais terminé");
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut empty = Slideshow::new(SlideshowConfig::default(), 320, 200);
        let frame = wait_frame(&mut empty, "message sans dossier");
        assert_eq!(empty.count(), 0);
        assert!(
            frame.as_chunks::<4>().0.iter().any(|p| p[0] > 150),
            "texte du message"
        );
        assert!(
            empty.poll(Instant::now()).is_none(),
            "message produit une seule fois"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn config_validation_and_parsing() {
        assert!(SlideshowConfig::default().validate().is_ok());
        let bad = SlideshowConfig {
            interval_s: 0.2,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
        let bad = SlideshowConfig {
            transition_ms: 9000,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
        assert_eq!(
            "chronological".parse::<Order>().unwrap(),
            Order::Chronological
        );
        assert!("alphabetical".parse::<Order>().is_err());
        assert_eq!("cover".parse::<SlideFit>().unwrap(), SlideFit::Cover);
        assert!("stretch".parse::<SlideFit>().is_err());
    }
}
