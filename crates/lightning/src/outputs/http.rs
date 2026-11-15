//! Sortie `http` : petit serveur local pour regarder l'écran depuis un navigateur
//! ou récupérer l'image courante depuis un outil.
//!
//! - `/` : page HTML avec le flux et l'état ;
//! - `/stream.mjpg` : flux MJPEG (`multipart/x-mixed-replace`) ;
//! - `/frame.jpg`, `/frame.png` : dernière image ;
//! - `/status.json` : métriques de debug et état de l'écran.
//!
//! Lecture seule, sans dépendance : une connexion = un fil, une requête.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, info};

use super::{Frame, HEIGHT, SharedStats, Sink, WIDTH};

const PAGE: &str = include_str!("page.html");
const BOUNDARY: &str = "lightningframe";

struct Shared {
    latest: Mutex<Option<Arc<Frame>>>,
    fresh: Condvar,
    stats: SharedStats,
    stop: Arc<AtomicBool>,
    /// Sortie retirée : le serveur et les flux en cours s'arrêtent.
    closing: AtomicBool,
}

impl Shared {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || self.closing.load(Ordering::SeqCst)
    }
}

pub struct HttpSink {
    shared: Arc<Shared>,
    #[cfg(test)]
    addr: std::net::SocketAddr,
}

impl HttpSink {
    /// Démarre le serveur sur `addr` (port 0 = port libre choisi par le système).
    pub fn spawn(addr: &str, stats: SharedStats, stop: Arc<AtomicBool>) -> Result<Self> {
        let listener =
            TcpListener::bind(addr).with_context(|| format!("écoute HTTP sur {addr}"))?;
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let shared = Arc::new(Shared {
            latest: Mutex::new(None),
            fresh: Condvar::new(),
            stats,
            stop,
            closing: AtomicBool::new(false),
        });
        {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("http".into())
                .spawn(move || accept_loop(&listener, &shared))
                .context("création du fil http")?;
        }
        info!("aperçu HTTP : http://{addr}/");
        shared.stats.lock().http = Some(format!("http://{addr}/"));
        Ok(Self {
            shared,
            #[cfg(test)]
            addr,
        })
    }

    /// Adresse réellement écoutée.
    #[cfg(test)]
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }
}

impl Sink for HttpSink {
    fn name(&self) -> &'static str {
        "http"
    }

    fn present(&mut self, frame: &Arc<Frame>) {
        *self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame.clone());
        self.shared.fresh.notify_all();
    }
}

impl Drop for HttpSink {
    fn drop(&mut self) {
        // Arrête le serveur et réveille les flux en cours.
        self.shared.closing.store(true, Ordering::SeqCst);
        self.shared.stats.lock().http = None;
        self.shared.fresh.notify_all();
    }
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    while !shared.stopping() {
        match listener.accept() {
            Ok((stream, peer)) => {
                let shared = shared.clone();
                let spawned = std::thread::Builder::new()
                    .name("http-client".into())
                    .spawn(move || {
                        if let Err(e) = handle(stream, &shared) {
                            debug!(%peer, "client HTTP : {e}");
                        }
                    });
                if let Err(e) = spawned {
                    debug!("fil client non créé : {e}");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                debug!("accept : {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn latest(shared: &Shared) -> Option<Arc<Frame>> {
    shared
        .latest
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn handle(mut stream: TcpStream, shared: &Shared) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    // En-têtes ignorés, mais lus pour vider la requête.
    let mut line = String::new();
    while reader.read_line(&mut line)? > 2 {
        line.clear();
    }
    let mut parts = request_line.split_ascii_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let path = target.split('?').next().unwrap_or("");
    if method != "GET" && method != "HEAD" {
        respond(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            b"GET uniquement\n",
        )?;
        return Ok(());
    }
    match path {
        "/" | "/index.html" => {
            respond(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                PAGE.as_bytes(),
            )?;
        }
        "/status.json" => {
            let body = serde_json::to_vec_pretty(&shared.stats.snapshot())?;
            respond(&mut stream, "200 OK", "application/json", &body)?;
        }
        "/frame.jpg" => match latest(shared) {
            Some(frame) => respond(&mut stream, "200 OK", "image/jpeg", &frame.jpeg)?,
            None => no_frame(&mut stream)?,
        },
        "/frame.png" => match latest(shared) {
            Some(frame) => {
                let png = encode_png(&frame)?;
                respond(&mut stream, "200 OK", "image/png", &png)?;
            }
            None => no_frame(&mut stream)?,
        },
        "/stream.mjpg" => stream_mjpeg(&mut stream, shared)?,
        _ => respond(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"introuvable\n",
        )?,
    }
    Ok(())
}

fn no_frame(stream: &mut TcpStream) -> std::io::Result<()> {
    respond(
        stream,
        "503 Service Unavailable",
        "text/plain; charset=utf-8",
        "aucune image produite pour l'instant\n".as_bytes(),
    )
}

/// PNG de la dernière image : pixels d'origine si disponibles, sinon JPEG décodé.
fn encode_png(frame: &Frame) -> Result<Vec<u8>> {
    let rgba = match &frame.rgba {
        Some(rgba) => image::RgbaImage::from_raw(WIDTH, HEIGHT, rgba.clone())
            .context("taille d'image inattendue")?,
        None => {
            image::load_from_memory_with_format(&frame.jpeg, image::ImageFormat::Jpeg)?.into_rgba8()
        }
    };
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .into_rgb8()
        .write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

fn stream_mjpeg(stream: &mut TcpStream, shared: &Shared) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary={BOUNDARY}\r\n\
         Cache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n"
    )?;
    let mut sent = 0u64;
    while !shared.stopping() {
        let frame = {
            let mut guard = shared.latest.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                match guard.as_ref() {
                    Some(frame) if frame.seq != sent => break Some(frame.clone()),
                    _ => {}
                }
                let (next, timeout) = shared
                    .fresh
                    .wait_timeout(guard, Duration::from_millis(500))
                    .unwrap_or_else(|e| e.into_inner());
                guard = next;
                if timeout.timed_out() {
                    break None;
                }
            }
        };
        let Some(frame) = frame else { continue };
        sent = frame.seq;
        write!(
            stream,
            "--{BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            frame.jpeg.len()
        )?;
        stream.write_all(&frame.jpeg)?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_rgba;
    use super::super::{LcdState, Outputs};
    use super::*;
    use std::io::Read;
    use std::net::SocketAddr;

    fn get(addr: SocketAddr, path: &str) -> (String, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: test\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (
            String::from_utf8_lossy(&raw[..split]).into_owned(),
            raw[split + 4..].to_vec(),
        )
    }

    fn server() -> (Outputs, SocketAddr, SharedStats, Arc<AtomicBool>) {
        let stats = SharedStats::default();
        let stop = Arc::new(AtomicBool::new(false));
        let sink = HttpSink::spawn("127.0.0.1:0", stats.clone(), stop.clone()).unwrap();
        let addr = sink.addr();
        let out = Outputs::new(vec![Box::new(sink)], stats.clone(), 80, "test");
        (out, addr, stats, stop)
    }

    #[test]
    fn serves_frame_status_page_and_errors() {
        let (mut out, addr, stats, stop) = server();
        assert!(addr.ip().is_loopback());

        // Avant la première image.
        let (head, _) = get(addr, "/frame.jpg");
        assert!(head.starts_with("HTTP/1.1 503"), "{head}");

        stats.set_lcd(LcdState::Absent, "écran introuvable");
        out.present_rgba(&test_rgba(120), Duration::from_millis(5), false)
            .unwrap();

        let (head, body) = get(addr, "/frame.jpg");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("Content-Type: image/jpeg"));
        assert_eq!(&body[..2], &[0xFF, 0xD8]);
        assert_eq!(&body[body.len() - 2..], &[0xFF, 0xD9]);
        assert!(head.contains(&format!("Content-Length: {}", body.len())));

        let (head, body) = get(addr, "/frame.png");
        assert!(head.contains("Content-Type: image/png"));
        let png = image::load_from_memory(&body).unwrap();
        assert_eq!((png.width(), png.height()), (1280, 800));
        assert_eq!(png.to_rgb8().get_pixel(10, 10).0, [120, 120, 120]);

        let (head, body) = get(addr, "/status.json?x=1");
        assert!(head.contains("application/json"));
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["frames"], 1);
        assert_eq!(json["width"], 1280);
        assert_eq!(json["outputs"][0], "http");
        assert_eq!(json["lcd"]["state"], "absent");
        assert_eq!(json["lcd"]["detail"], "écran introuvable");
        assert!(json["jpeg_bytes"].as_u64().unwrap() > 100);

        let (head, body) = get(addr, "/");
        assert!(head.contains("text/html"));
        assert!(String::from_utf8_lossy(&body).contains("/stream.mjpg"));

        assert!(get(addr, "/nope").0.starts_with("HTTP/1.1 404"));
        stop.store(true, Ordering::SeqCst);
    }

    #[test]
    fn mjpeg_stream_delivers_new_frames() {
        let (mut out, addr, _stats, stop) = server();
        out.present_rgba(&test_rgba(10), Duration::ZERO, false)
            .unwrap();

        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(stream, "GET /stream.mjpg HTTP/1.1\r\n\r\n").unwrap();
        let mut reader = BufReader::new(stream);

        let mut read_part = |expect_header: bool| -> Vec<u8> {
            let mut len = None;
            let mut seen_boundary = false;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0, "flux fermé");
                let line = line.trim_end();
                if expect_header && line.starts_with("HTTP/1.1") {
                    assert!(line.contains("200"));
                }
                if line == format!("--{BOUNDARY}") {
                    seen_boundary = true;
                }
                if let Some(v) = line.strip_prefix("Content-Length: ") {
                    len = Some(v.parse::<usize>().unwrap());
                }
                if line.is_empty() && seen_boundary && len.is_some() {
                    break;
                }
            }
            let mut body = vec![0u8; len.unwrap()];
            reader.read_exact(&mut body).unwrap();
            body
        };

        let first = read_part(true);
        assert_eq!(&first[..2], &[0xFF, 0xD8]);
        out.present_rgba(&test_rgba(250), Duration::ZERO, false)
            .unwrap();
        let second = read_part(false);
        assert_eq!(&second[..2], &[0xFF, 0xD8]);
        assert_ne!(first, second);
        stop.store(true, Ordering::SeqCst);
    }
}
