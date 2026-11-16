//! Lecture en cours : lecteurs MPRIS (`org.mpris.MediaPlayer2.*`) du bus de session.
//!
//! Un fil interroge les lecteurs une fois par seconde et garde le dernier
//! morceau connu ; la position est extrapolée entre deux relevés.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lightning_dashboard::{Artwork, Track};
use serde::{Deserialize, Serialize};
use tracing::debug;
use zbus::zvariant::OwnedValue;

/// Préfixe des noms de bus des lecteurs.
const PREFIX: &str = "org.mpris.MediaPlayer2.";
/// Côté maximal d'une pochette conservée.
const ART_MAX: u32 = 600;

/// Réglages du mode « en cours de lecture » (section `[now_playing]`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NowPlayingConfig {
    /// Lecteur préféré : partie de son nom MPRIS (`spotify`, `firefox`…) ;
    /// vide = le premier qui joue.
    pub player: String,
}

/// Métadonnées utiles d'un morceau.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub length: Option<Duration>,
    pub art_url: String,
}

fn string(v: &OwnedValue) -> Option<String> {
    v.try_clone().ok().and_then(|v| String::try_from(v).ok())
}

/// Extrait les champs `xesam:*` et `mpris:*` d'un dictionnaire `Metadata`.
pub fn parse_metadata(map: &HashMap<String, OwnedValue>) -> Metadata {
    let text = |key: &str| map.get(key).and_then(string).unwrap_or_default();
    let artist = map
        .get("xesam:artist")
        .and_then(|v| {
            v.try_clone()
                .ok()
                .and_then(|v| Vec::<String>::try_from(v).ok())
                .or_else(|| string(v).map(|s| vec![s]))
        })
        .unwrap_or_default()
        .join(", ");
    // La spécification dit `x` (i64) ; certains lecteurs envoient `t` (u64).
    let length = map.get("mpris:length").and_then(|v| {
        let micros = v
            .try_clone()
            .ok()
            .and_then(|v| i64::try_from(v).ok())
            .or_else(|| {
                v.try_clone()
                    .ok()
                    .and_then(|v| u64::try_from(v).ok())
                    .and_then(|u| i64::try_from(u).ok())
            })?;
        (micros > 0).then(|| Duration::from_micros(micros as u64))
    });
    Metadata {
        title: text("xesam:title"),
        artist,
        album: text("xesam:album"),
        length,
        art_url: text("mpris:artUrl"),
    }
}

/// Choisit un lecteur parmi `(nom de bus, PlaybackStatus)` : d'abord celui
/// qui joue, sinon celui en pause ; `preferred` restreint aux noms qui le
/// contiennent (sans tenir compte de la casse). Les lecteurs arrêtés sont ignorés.
pub fn pick_player(players: &[(String, String)], preferred: &str) -> Option<usize> {
    let preferred = preferred.trim().to_ascii_lowercase();
    let eligible = |name: &str| {
        preferred.is_empty()
            || name
                .strip_prefix(PREFIX)
                .unwrap_or(name)
                .to_ascii_lowercase()
                .contains(&preferred)
    };
    ["Playing", "Paused"].iter().find_map(|wanted| {
        players
            .iter()
            .position(|(name, status)| status == wanted && eligible(name))
    })
}

/// Chemin local d'une adresse `file://` (caractères `%XX` décodés).
pub fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .filter(|h| bytes[i] == b'%' && h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        if let Some(byte) = hex {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let path = PathBuf::from(String::from_utf8(out).ok()?);
    path.is_absolute().then_some(path)
}

/// Décode une pochette et la réduit à [`ART_MAX`] pixels de côté au plus.
pub fn decode_art(bytes: &[u8]) -> Option<Artwork> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > ART_MAX || img.height() > ART_MAX {
        img.thumbnail(ART_MAX, ART_MAX)
    } else {
        img
    };
    let rgba = img.into_rgba8();
    Some(Artwork {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// Charge une pochette (`file://` ou `http(s)://`, 8 Mo au plus).
fn load_art(url: &str) -> Option<Artwork> {
    const LIMIT: u64 = 8 << 20;
    let bytes = if let Some(path) = file_url_path(url) {
        let mut buf = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(LIMIT)
            .read_to_end(&mut buf)
            .ok()?;
        buf
    } else if url.starts_with("https://") || url.starts_with("http://") {
        let response = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .build()
            .get(url)
            .call()
            .ok()?;
        let mut buf = Vec::new();
        response
            .into_reader()
            .take(LIMIT)
            .read_to_end(&mut buf)
            .ok()?;
        buf
    } else {
        return None;
    };
    decode_art(&bytes)
}

/// Dernier relevé des lecteurs.
#[derive(Default)]
struct Sample {
    track: Option<Track>,
    art: Option<Arc<Artwork>>,
    at: Option<Instant>,
}

enum Source {
    Live {
        sample: Arc<Mutex<Sample>>,
        stop: Arc<AtomicBool>,
    },
    Demo {
        start: Instant,
        art: Arc<Artwork>,
    },
}

/// Observateur des lecteurs MPRIS ; s'arrête à sa destruction.
pub struct Watcher {
    source: Source,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if let Source::Live { stop, .. } = &self.source {
            stop.store(true, Ordering::SeqCst);
        }
    }
}

impl Watcher {
    /// Lance le fil d'observation sur le bus de session.
    pub fn spawn(preferred: String) -> Self {
        let sample = Arc::new(Mutex::new(Sample::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (sample_, stop_) = (sample.clone(), stop.clone());
        let spawned = std::thread::Builder::new()
            .name("mpris".into())
            .spawn(move || watch(&preferred, &sample_, &stop_));
        if let Err(e) = spawned {
            debug!("fil MPRIS non lancé : {e}");
        }
        Self {
            source: Source::Live { sample, stop },
        }
    }

    /// Morceau factice qui avance (mode démonstration).
    pub fn demo() -> Self {
        Self {
            source: Source::Demo {
                start: Instant::now(),
                art: Arc::new(lightning_dashboard::render::demo_artwork(480)),
            },
        }
    }

    /// Morceau courant, position extrapolée à `now`, et sa pochette.
    pub fn current(&self, now: Instant) -> (Option<Track>, Option<Arc<Artwork>>) {
        match &self.source {
            Source::Demo { start, art } => (
                Some(lightning_dashboard::render::demo_track(
                    now.saturating_duration_since(*start),
                )),
                Some(art.clone()),
            ),
            Source::Live { sample, .. } => {
                let s = sample.lock().unwrap_or_else(|e| e.into_inner());
                let track = s.track.clone().map(|mut t| {
                    if t.playing
                        && let Some(at) = s.at
                    {
                        t.position += now.saturating_duration_since(at);
                        if let Some(length) = t.length {
                            t.position = t.position.min(length);
                        }
                    }
                    t
                });
                (track, s.art.clone())
            }
        }
    }
}

fn watch(preferred: &str, sample: &Mutex<Sample>, stop: &AtomicBool) {
    let mut connection = None;
    let mut art_url = String::new();
    let mut art: Option<Arc<Artwork>> = None;
    while !stop.load(Ordering::SeqCst) {
        if connection.is_none() {
            connection = zbus::blocking::connection::Builder::session()
                .and_then(|b| b.method_timeout(Duration::from_secs(2)).build())
                .map_err(|e| debug!("MPRIS : bus de session indisponible : {e}"))
                .ok();
        }
        let track = match &connection {
            Some(c) => match poll(c, preferred) {
                Ok(found) => found,
                Err(e) => {
                    debug!("MPRIS : {e}");
                    connection = None;
                    None
                }
            },
            None => None,
        };
        // Pochette rechargée seulement quand son adresse change.
        let wanted = track
            .as_ref()
            .map(|(_, url)| url.clone())
            .unwrap_or_default();
        if wanted != art_url {
            art_url = wanted;
            art = if art_url.is_empty() {
                None
            } else {
                load_art(&art_url).map(Arc::new)
            };
        }
        {
            let mut s = sample.lock().unwrap_or_else(|e| e.into_inner());
            s.track = track.map(|(t, _)| t);
            s.art = art.clone();
            s.at = Some(Instant::now());
        }
        for _ in 0..10 {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn properties<'a>(
    connection: &'a zbus::blocking::Connection,
    name: &'a str,
) -> zbus::Result<zbus::blocking::fdo::PropertiesProxy<'a>> {
    zbus::blocking::fdo::PropertiesProxy::builder(connection)
        .destination(name)?
        .path("/org/mpris/MediaPlayer2")?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
}

/// Un relevé : le morceau du lecteur choisi et l'adresse de sa pochette.
fn poll(
    connection: &zbus::blocking::Connection,
    preferred: &str,
) -> zbus::Result<Option<(Track, String)>> {
    use zbus::names::InterfaceName;
    let names = zbus::blocking::fdo::DBusProxy::new(connection)?.list_names()?;
    let player_iface = InterfaceName::from_static_str_unchecked("org.mpris.MediaPlayer2.Player");
    let mut players = Vec::new();
    let mut all = HashMap::new();
    for name in names {
        let name = name.to_string();
        if !name.starts_with(PREFIX) {
            continue;
        }
        // Un lecteur qui ne répond pas est simplement ignoré.
        let Ok(props) =
            properties(connection, &name).and_then(|p| Ok(p.get_all(player_iface.clone())?))
        else {
            continue;
        };
        let status = props
            .get("PlaybackStatus")
            .and_then(string)
            .unwrap_or_default();
        players.push((name.clone(), status));
        all.insert(name, props);
    }
    let Some(index) = pick_player(&players, preferred) else {
        return Ok(None);
    };
    let (name, status) = &players[index];
    let props = &all[name];
    let meta = props
        .get("Metadata")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| HashMap::<String, OwnedValue>::try_from(v).ok())
        .map(|m| parse_metadata(&m))
        .unwrap_or_default();
    let position = props
        .get("Position")
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| i64::try_from(v).ok())
        .filter(|p| *p > 0)
        .map_or(Duration::ZERO, |p| Duration::from_micros(p as u64));
    let identity = properties(connection, name)
        .and_then(|p| {
            Ok(p.get(
                InterfaceName::from_static_str_unchecked("org.mpris.MediaPlayer2"),
                "Identity",
            )?)
        })
        .ok()
        .and_then(|v| String::try_from(v).ok())
        .unwrap_or_else(|| player_name(name));
    let track = Track {
        title: meta.title,
        artist: meta.artist,
        album: meta.album,
        player: identity,
        length: meta.length,
        position,
        playing: status == "Playing",
    };
    Ok(Some((track, meta.art_url)))
}

/// Nom lisible tiré du nom de bus : `org.mpris.MediaPlayer2.spotify` → `spotify`.
pub fn player_name(bus_name: &str) -> String {
    let short = bus_name.strip_prefix(PREFIX).unwrap_or(bus_name);
    // Instances multiples : `firefox.instance_1_23`.
    short.split('.').next().unwrap_or(short).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn owned(v: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(v).unwrap()
    }

    #[test]
    fn metadata_fields() {
        let mut map = HashMap::new();
        map.insert("xesam:title".into(), owned(Value::from("Titre")));
        map.insert(
            "xesam:artist".into(),
            owned(Value::from(vec!["A".to_owned(), "B".to_owned()])),
        );
        map.insert("xesam:album".into(), owned(Value::from("Album")));
        map.insert("mpris:length".into(), owned(Value::from(185_000_000i64)));
        map.insert(
            "mpris:artUrl".into(),
            owned(Value::from("file:///tmp/a%20b.png")),
        );
        let meta = parse_metadata(&map);
        assert_eq!(meta.title, "Titre");
        assert_eq!(meta.artist, "A, B");
        assert_eq!(meta.album, "Album");
        assert_eq!(meta.length, Some(Duration::from_secs(185)));
        assert_eq!(meta.art_url, "file:///tmp/a%20b.png");

        // Variantes : durée en u64, artiste en simple chaîne, champs absents.
        let mut map = HashMap::new();
        map.insert("mpris:length".into(), owned(Value::from(2_000_000u64)));
        map.insert("xesam:artist".into(), owned(Value::from("Solo")));
        let meta = parse_metadata(&map);
        assert_eq!(meta.length, Some(Duration::from_secs(2)));
        assert_eq!(meta.artist, "Solo");
        assert!(meta.title.is_empty() && meta.art_url.is_empty());
        let mut map = HashMap::new();
        map.insert("mpris:length".into(), owned(Value::from(0i64)));
        assert_eq!(parse_metadata(&map).length, None);
    }

    #[test]
    fn player_choice() {
        let players = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter()
                .map(|(n, s)| (format!("{PREFIX}{n}"), (*s).to_owned()))
                .collect()
        };
        let list = players(&[
            ("firefox.instance_1_2", "Paused"),
            ("spotify", "Playing"),
            ("rhythmbox", "Stopped"),
        ]);
        assert_eq!(pick_player(&list, ""), Some(1), "celui qui joue d'abord");
        assert_eq!(pick_player(&list, "Firefox"), Some(0), "préféré, en pause");
        assert_eq!(pick_player(&list, "rhythmbox"), None, "arrêté");
        assert_eq!(pick_player(&list, "vlc"), None);
        assert_eq!(
            pick_player(&players(&[("a", "Paused"), ("b", "Paused")]), ""),
            Some(0)
        );
        assert_eq!(pick_player(&[], ""), None);
        assert_eq!(
            player_name(&format!("{PREFIX}firefox.instance_1_2")),
            "firefox"
        );
        assert_eq!(player_name(&format!("{PREFIX}spotify")), "spotify");
    }

    #[test]
    fn file_urls_and_art_decoding() {
        assert_eq!(
            file_url_path("file:///home/moi/Musique/%C3%A9t%C3%A9%20.jpg"),
            Some(PathBuf::from("/home/moi/Musique/été .jpg"))
        );
        assert_eq!(file_url_path("file:///a%2"), Some(PathBuf::from("/a%2")));
        assert_eq!(file_url_path("https://exemple.org/a.jpg"), None);
        assert_eq!(file_url_path("file://relatif"), None);

        let img = image::RgbImage::from_pixel(1200, 900, image::Rgb([200, 10, 10]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let art = decode_art(&png).unwrap();
        assert_eq!((art.width, art.height), (600, 450));
        assert_eq!(&art.rgba[..4], &[200, 10, 10, 255]);
        assert!(decode_art(b"pas une image").is_none());
    }

    #[test]
    fn demo_watcher_advances_and_live_watcher_starts_empty() {
        let demo = Watcher::demo();
        let start = Instant::now();
        let (a, art) = demo.current(start);
        let (b, _) = demo.current(start + Duration::from_secs(5));
        assert!(art.is_some());
        assert!(b.unwrap().position > a.unwrap().position);

        // Sans relevé encore : aucun morceau.
        let live = Watcher {
            source: Source::Live {
                sample: Arc::new(Mutex::new(Sample::default())),
                stop: Arc::new(AtomicBool::new(false)),
            },
        };
        assert_eq!(live.current(Instant::now()).0, None);
        // Extrapolation bornée par la durée.
        if let Source::Live { sample, .. } = &live.source {
            let mut s = sample.lock().unwrap();
            s.track = Some(Track {
                position: Duration::from_secs(10),
                length: Some(Duration::from_secs(12)),
                playing: true,
                ..Default::default()
            });
            s.at = Some(start);
        }
        let later = live.current(start + Duration::from_secs(1)).0.unwrap();
        assert_eq!(later.position, Duration::from_secs(11));
        let end = live.current(start + Duration::from_secs(30)).0.unwrap();
        assert_eq!(end.position, Duration::from_secs(12));
    }
}
