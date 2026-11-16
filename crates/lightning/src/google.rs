//! Import de photos Google Photos par la **Picker API**.
//!
//! Depuis le 31 mars 2025, la Library API ne donne plus accès à la bibliothèque
//! de l'utilisateur ; la voie officielle est la Picker API : l'application crée
//! une session, l'utilisateur choisit ses photos dans l'interface Google
//! (`pickerUri`), puis l'application liste les éléments choisis et télécharge
//! leurs `baseUrl` (valables 60 minutes, avec le jeton OAuth).
//!
//! Les photos sont copiées dans un cache local qui sert de dossier au
//! diaporama. Rien n'est envoyé ailleurs que chez Google. L'identifiant client
//! OAuth (type « application de bureau ») vient de la configuration ; le jeton
//! de rafraîchissement est conservé dans `google.json` (mode 0600).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::secrets::TokenStorage;

/// Portée OAuth de la Picker API (lecture des éléments choisis par l'utilisateur).
pub const SCOPE: &str = "https://www.googleapis.com/auth/photospicker.mediaitems.readonly";

/// Identifiants du client OAuth (section `[google]`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GoogleConfig {
    /// Identifiant du client OAuth de type « application de bureau ».
    pub client_id: String,
    pub client_secret: String,
}

impl GoogleConfig {
    pub fn is_configured(&self) -> bool {
        !self.client_id.is_empty() && !self.client_secret.is_empty()
    }
}

/// Adresses des services ; remplaçables par un serveur local dans les tests.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub auth: String,
    pub token: String,
    pub picker: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            auth: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            picker: "https://photospicker.googleapis.com/v1".into(),
        }
    }
}

/// Avancement de l'import, publié sur D-Bus.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Status {
    /// `unconfigured`, `idle`, `awaiting_auth`, `awaiting_selection`,
    /// `downloading` ou `error`.
    pub state: String,
    pub detail: String,
    /// Page à ouvrir dans le navigateur (autorisation, puis choix des photos).
    pub url: String,
}

pub type SharedStatus = Arc<Mutex<Status>>;

fn set(status: &SharedStatus, state: &str, detail: impl Into<String>, url: impl Into<String>) {
    *status.lock().unwrap_or_else(|e| e.into_inner()) = Status {
        state: state.to_owned(),
        detail: detail.into(),
        url: url.into(),
    };
}

// ------------------------------------------------------------------ PKCE

/// Chaîne aléatoire sûre pour une URL (`n` octets d'entropie).
pub fn random_token(n: usize) -> std::io::Result<String> {
    let mut bytes = vec![0u8; n];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Défi PKCE S256 : `BASE64URL(SHA256(vérifieur))` (RFC 7636).
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn urlencode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn urldecode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(v) => {
                        out.push(v);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// URL d'autorisation OAuth 2.0 (application de bureau, PKCE, redirection locale).
pub fn auth_url(
    endpoints: &Endpoints,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> String {
    format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}\
         &code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        endpoints.auth,
        urlencode(client_id),
        urlencode(redirect_uri),
        urlencode(SCOPE),
        urlencode(challenge),
        urlencode(state)
    )
}

/// Extrait `(code, state)` de la cible d'une requête de redirection
/// (`/?code=…&state=…`), ou l'erreur renvoyée par Google.
pub fn parse_redirect(target: &str) -> Result<(String, String), String> {
    let query = target.split_once('?').map_or("", |(_, q)| q);
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "code" => code = Some(urldecode(value)),
            "state" => state = Some(urldecode(value)),
            "error" => error = Some(urldecode(value)),
            _ => {}
        }
    }
    if let Some(error) = error {
        return Err(format!("autorisation refusée ({error})"));
    }
    match (code, state) {
        (Some(code), Some(state)) => Ok((code, state)),
        _ => Err("redirection sans code d'autorisation".into()),
    }
}

// ------------------------------------------------------------------ types

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PollingConfig {
    #[serde(default)]
    pub poll_interval: String,
    #[serde(default)]
    pub timeout_in: String,
}

/// Session de sélection (`PickingSession`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    #[serde(default)]
    pub picker_uri: String,
    #[serde(default)]
    pub polling_config: Option<PollingConfig>,
    #[serde(default)]
    pub media_items_set: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaFile {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub filename: String,
}

/// Élément choisi (`PickedMediaItem`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub id: String,
    #[serde(default)]
    pub create_time: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub media_file: MediaFile,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaPage {
    #[serde(default)]
    media_items: Vec<MediaItem>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// Durée au format protobuf JSON (« 3.5s »).
pub fn parse_duration(text: &str) -> Option<Duration> {
    let secs: f64 = text.strip_suffix('s')?.parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

// ------------------------------------------------------------------- HTTP

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!("lightning/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// Message d'erreur lisible, avec le détail renvoyé par Google s'il y en a un.
fn explain(context: &str, err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            let detail = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v["error"]["message"]
                        .as_str()
                        .or(v["error_description"].as_str())
                        .or(v["error"].as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| body.chars().take(200).collect());
            format!("{context} : erreur {code} ({detail})")
        }
        other => format!("{context} : {other}"),
    }
}

pub fn exchange_code(
    endpoints: &Endpoints,
    cfg: &GoogleConfig,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<Tokens, String> {
    agent()
        .post(&endpoints.token)
        .send_form(&[
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ])
        .map_err(|e| explain("échange du code d'autorisation", e))?
        .into_json()
        .map_err(|e| format!("réponse du serveur de jetons illisible : {e}"))
}

pub fn refresh(
    endpoints: &Endpoints,
    cfg: &GoogleConfig,
    refresh_token: &str,
) -> Result<Tokens, String> {
    agent()
        .post(&endpoints.token)
        .send_form(&[
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .map_err(|e| explain("renouvellement du jeton", e))?
        .into_json()
        .map_err(|e| format!("réponse du serveur de jetons illisible : {e}"))
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

pub fn create_session(endpoints: &Endpoints, token: &str) -> Result<Session, String> {
    agent()
        .post(&format!("{}/sessions", endpoints.picker))
        .set("Authorization", &bearer(token))
        .send_json(serde_json::json!({}))
        .map_err(|e| explain("création de la session de sélection", e))?
        .into_json()
        .map_err(|e| format!("session illisible : {e}"))
}

pub fn get_session(endpoints: &Endpoints, token: &str, id: &str) -> Result<Session, String> {
    agent()
        .get(&format!("{}/sessions/{}", endpoints.picker, urlencode(id)))
        .set("Authorization", &bearer(token))
        .call()
        .map_err(|e| explain("lecture de la session de sélection", e))?
        .into_json()
        .map_err(|e| format!("session illisible : {e}"))
}

pub fn delete_session(endpoints: &Endpoints, token: &str, id: &str) -> Result<(), String> {
    agent()
        .delete(&format!("{}/sessions/{}", endpoints.picker, urlencode(id)))
        .set("Authorization", &bearer(token))
        .call()
        .map(drop)
        .map_err(|e| explain("suppression de la session", e))
}

/// Liste tous les éléments choisis (pages de 100).
pub fn list_media_items(
    endpoints: &Endpoints,
    token: &str,
    session_id: &str,
) -> Result<Vec<MediaItem>, String> {
    let mut items = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut request = agent()
            .get(&format!("{}/mediaItems", endpoints.picker))
            .set("Authorization", &bearer(token))
            .query("sessionId", session_id)
            .query("pageSize", "100");
        if let Some(t) = &page_token {
            request = request.query("pageToken", t);
        }
        let page: MediaPage = request
            .call()
            .map_err(|e| explain("liste des photos choisies", e))?
            .into_json()
            .map_err(|e| format!("liste des photos illisible : {e}"))?;
        items.extend(page.media_items);
        match page.next_page_token.filter(|t| !t.is_empty()) {
            Some(t) if items.len() < 5000 => page_token = Some(t),
            _ => return Ok(items),
        }
    }
}

/// Nom de fichier sûr pour le cache : identifiant de l'élément + nom d'origine nettoyé.
pub fn cache_name(item: &MediaItem) -> String {
    let clean = |s: &str, max: usize| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .take(max)
            .collect()
    };
    let id = clean(&item.id, 24);
    let stem = Path::new(&item.media_file.filename)
        .file_stem()
        .map(|s| clean(&s.to_string_lossy(), 60))
        .unwrap_or_default();
    // `=w…-h…` renvoie toujours un JPEG, quel que soit le format d'origine.
    if stem.is_empty() {
        format!("{id}.jpg")
    } else {
        format!("{id}-{stem}.jpg")
    }
}

/// Télécharge une photo à la taille de l'écran dans `dir`.
pub fn download(
    item: &MediaItem,
    token: &str,
    dir: &Path,
    width: u32,
    height: u32,
) -> Result<PathBuf, String> {
    if item.media_file.base_url.is_empty() {
        return Err(format!(
            "élément {} sans adresse de téléchargement",
            item.id
        ));
    }
    // Un `baseUrl` ne s'utilise jamais nu : on demande au plus la taille de l'écran ×2.
    let url = format!(
        "{}=w{}-h{}",
        item.media_file.base_url,
        width * 2,
        height * 2
    );
    let response = agent()
        .get(&url)
        .set("Authorization", &bearer(token))
        .call()
        .map_err(|e| explain("téléchargement d'une photo", e))?;
    let path = dir.join(cache_name(item));
    let tmp = path.with_extension("part");
    let mut file = std::fs::File::create(&tmp).map_err(|e| format!("{} : {e}", tmp.display()))?;
    std::io::copy(&mut response.into_reader().take(64 << 20), &mut file)
        .map_err(|e| format!("téléchargement interrompu : {e}"))?;
    drop(file);
    std::fs::rename(&tmp, &path).map_err(|e| format!("{} : {e}", path.display()))?;
    Ok(path)
}

// -------------------------------------------------------------- stockage

/// Dossier du cache des photos importées.
pub fn cache_root() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("lightning/google-photos")
}

// ------------------------------------------------------------------ flux

/// Paramètres d'un import.
pub struct ImportOptions {
    pub config: GoogleConfig,
    pub endpoints: Endpoints,
    /// Fichier du jeton de rafraîchissement (repli quand le trousseau manque).
    pub token_path: PathBuf,
    /// Utiliser le trousseau de la session (Secret Service).
    pub keyring: bool,
    pub cache_root: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Délai maximal laissé à l'utilisateur pour autoriser l'application.
    pub auth_timeout: Duration,
}

fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        Err("import annulé".into())
    } else {
        Ok(())
    }
}

/// Attend la redirection du navigateur sur l'écouteur local ; renvoie le code.
fn wait_for_redirect(
    listener: &TcpListener,
    expected_state: &str,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        check(cancel)?;
        if Instant::now() >= deadline {
            return Err("autorisation non reçue à temps".into());
        }
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(format!("écoute locale : {e}")),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut line = String::new();
        if BufReader::new(&stream).read_line(&mut line).is_err() {
            continue;
        }
        let target = line.split_ascii_whitespace().nth(1).unwrap_or("");
        // Le navigateur demande aussi /favicon.ico : on ne traite que la redirection.
        if !target.starts_with("/?") {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            continue;
        }
        let result = parse_redirect(target).and_then(|(code, state)| {
            if state == expected_state {
                Ok(code)
            } else {
                Err("redirection inattendue (état différent)".into())
            }
        });
        let message = match &result {
            Ok(_) => "Autorisation reçue. Vous pouvez fermer cet onglet et revenir à Lightning.",
            Err(_) => {
                "L'autorisation a échoué. Vous pouvez fermer cet onglet et réessayer depuis Lightning."
            }
        };
        let body = format!(
            "<!doctype html><html lang=\"fr\"><meta charset=\"utf-8\"><title>Lightning</title>\
             <body style=\"font-family:sans-serif;text-align:center;margin-top:20vh\"><h2>{message}</h2></body></html>"
        );
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        return result;
    }
}

/// Obtient un jeton d'accès : par le jeton enregistré, sinon par le navigateur.
fn access_token(
    opts: &ImportOptions,
    status: &SharedStatus,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let storage = if opts.keyring {
        TokenStorage::standard(&opts.token_path)
    } else {
        TokenStorage::file_only(&opts.token_path)
    };
    if let Some(stored) = storage.load() {
        match refresh(&opts.endpoints, &opts.config, &stored) {
            Ok(tokens) => return Ok(tokens.access_token),
            // Jeton révoqué ou expiré : on redemande l'autorisation.
            Err(e) => warn!("jeton Google enregistré inutilisable ({e}) : nouvelle autorisation"),
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("écoute locale : {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");
    let verifier = random_token(48).map_err(|e| e.to_string())?;
    let state = random_token(16).map_err(|e| e.to_string())?;
    let url = auth_url(
        &opts.endpoints,
        &opts.config.client_id,
        &redirect_uri,
        &pkce_challenge(&verifier),
        &state,
    );
    set(
        status,
        "awaiting_auth",
        "Autorisez Lightning dans le navigateur",
        url,
    );
    let code = wait_for_redirect(&listener, &state, cancel, opts.auth_timeout)?;
    let tokens = exchange_code(
        &opts.endpoints,
        &opts.config,
        &code,
        &verifier,
        &redirect_uri,
    )?;
    match &tokens.refresh_token {
        Some(token) => match storage.save(token) {
            Ok(place) => info!("jeton Google enregistré ({place})"),
            Err(e) => warn!("jeton Google non enregistré : {e}"),
        },
        None => debug!("pas de jeton de rafraîchissement dans la réponse"),
    }
    Ok(tokens.access_token)
}

/// Déroule un import complet ; renvoie le nombre de photos téléchargées.
pub fn import(
    opts: &ImportOptions,
    status: &SharedStatus,
    cancel: &AtomicBool,
) -> Result<usize, String> {
    if !opts.config.is_configured() {
        return Err(
            "identifiants Google absents : renseignez [google] client_id et client_secret".into(),
        );
    }
    let token = access_token(opts, status, cancel)?;
    let session = create_session(&opts.endpoints, &token)?;
    if session.picker_uri.is_empty() {
        return Err("session de sélection sans adresse".into());
    }
    set(
        status,
        "awaiting_selection",
        "Choisissez vos photos dans Google Photos",
        format!("{}/autoclose", session.picker_uri.trim_end_matches('/')),
    );
    let polling = session.polling_config.clone().unwrap_or_default();
    let mut interval = parse_duration(&polling.poll_interval).unwrap_or(Duration::from_secs(5));
    let timeout = parse_duration(&polling.timeout_in)
        .filter(|d| !d.is_zero())
        .unwrap_or(Duration::from_secs(1800));
    let deadline = Instant::now() + timeout;
    let result = (|| {
        loop {
            let waited = Instant::now();
            while waited.elapsed()
                < interval.clamp(Duration::from_millis(50), Duration::from_secs(30))
            {
                check(cancel)?;
                std::thread::sleep(Duration::from_millis(50));
            }
            let current = get_session(&opts.endpoints, &token, &session.id)?;
            if current.media_items_set {
                break;
            }
            if Instant::now() >= deadline {
                return Err("sélection non terminée à temps".to_owned());
            }
            if let Some(next) = current
                .polling_config
                .as_ref()
                .and_then(|p| parse_duration(&p.poll_interval))
            {
                interval = next;
            }
        }
        let items = list_media_items(&opts.endpoints, &token, &session.id)?;
        let photos: Vec<&MediaItem> = items.iter().filter(|i| i.kind != "VIDEO").collect();
        let safe_id: String = session
            .id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(64)
            .collect();
        let dir = opts.cache_root.join(if safe_id.is_empty() {
            "session".to_owned()
        } else {
            safe_id
        });
        std::fs::create_dir_all(&dir).map_err(|e| format!("{} : {e}", dir.display()))?;
        let mut done = 0usize;
        for (i, item) in photos.iter().enumerate() {
            check(cancel)?;
            set(
                status,
                "downloading",
                format!("Téléchargement {} / {}", i + 1, photos.len()),
                "",
            );
            match download(item, &token, &dir, opts.width, opts.height) {
                Ok(_) => done += 1,
                Err(e) => warn!("photo Google ignorée : {e}"),
            }
        }
        if done == 0 && !photos.is_empty() {
            return Err("aucune photo n'a pu être téléchargée".to_owned());
        }
        info!(photos = done, dossier = %dir.display(), "photos Google importées");
        Ok(done)
    })();
    // La session ne sert plus : on la supprime, que l'import ait réussi ou non.
    if let Err(e) = delete_session(&opts.endpoints, &token, &session.id) {
        debug!("{e}");
    }
    result
}

/// Import en arrière-plan, annulable.
pub struct Importer {
    pub status: SharedStatus,
    cancel: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
}

impl Importer {
    pub fn new(configured: bool) -> Self {
        let status = SharedStatus::default();
        set(
            &status,
            if configured { "idle" } else { "unconfigured" },
            "",
            "",
        );
        Self {
            status,
            cancel: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Remet l'état au repos (après un changement d'identifiants, par exemple).
    pub fn reset(&self, configured: bool) {
        if !self.is_running() {
            set(
                &self.status,
                if configured { "idle" } else { "unconfigured" },
                "",
                "",
            );
        }
    }

    /// Lance un import ; `done` reçoit le nombre de photos ou l'erreur.
    pub fn start(
        &self,
        opts: ImportOptions,
        done: impl FnOnce(Result<usize, String>) + Send + 'static,
    ) -> Result<(), String> {
        if !opts.config.is_configured() {
            return Err("identifiants Google absents : renseignez [google] client_id et client_secret dans la configuration".into());
        }
        if self.running.swap(true, Ordering::SeqCst) {
            return Err("un import est déjà en cours".into());
        }
        self.cancel.store(false, Ordering::SeqCst);
        set(&self.status, "awaiting_auth", "Connexion à Google…", "");
        let (status, cancel, running) = (
            self.status.clone(),
            self.cancel.clone(),
            self.running.clone(),
        );
        std::thread::Builder::new()
            .name("google-photos".into())
            .spawn(move || {
                let result = import(&opts, &status, &cancel);
                match &result {
                    Ok(n) => set(&status, "idle", format!("{n} photos importées"), ""),
                    Err(e) => set(&status, "error", e.clone(), ""),
                }
                running.store(false, Ordering::SeqCst);
                done(result);
            })
            .map(drop)
            .map_err(|e| {
                self.running.store(false, Ordering::SeqCst);
                e.to_string()
            })
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// Vide le cache des photos importées ; renvoie le nombre de fichiers supprimés.
pub fn clear_cache(root: &Path) -> std::io::Result<usize> {
    fn count(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| {
                        if e.path().is_dir() {
                            count(&e.path())
                        } else {
                            1
                        }
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
    if !root.exists() {
        return Ok(0);
    }
    let n = count(root);
    std::fs::remove_dir_all(root)?;
    std::fs::create_dir_all(root)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::TcpStream;

    #[test]
    fn pkce_matches_rfc_7636_appendix_b() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let a = random_token(48).unwrap();
        assert_eq!(a.len(), 64);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(a, random_token(48).unwrap());
    }

    #[test]
    fn auth_url_carries_pkce_scope_and_loopback_redirect() {
        let url = auth_url(
            &Endpoints::default(),
            "abc.apps.googleusercontent.com",
            "http://127.0.0.1:4242",
            "CHAL",
            "st/ate",
        );
        assert!(url.starts_with(
            "https://accounts.google.com/o/oauth2/v2/auth?client_id=abc.apps.googleusercontent.com&"
        ));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A4242"));
        assert!(url.contains(
            "scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fphotospicker.mediaitems.readonly"
        ));
        assert!(url.contains("code_challenge=CHAL&code_challenge_method=S256"));
        assert!(url.contains("state=st%2Fate"));
        assert!(url.contains("response_type=code") && url.contains("access_type=offline"));
        assert!(
            !url.contains("photoslibrary"),
            "les portées de la Library API sont supprimées"
        );
    }

    #[test]
    fn redirect_parsing() {
        assert_eq!(
            parse_redirect(
                "/?state=abc&code=4%2F0AX4XfWh-x_y&scope=https://www.googleapis.com/auth/x"
            )
            .unwrap(),
            ("4/0AX4XfWh-x_y".to_owned(), "abc".to_owned())
        );
        assert!(
            parse_redirect("/?error=access_denied&state=abc")
                .unwrap_err()
                .contains("access_denied")
        );
        assert!(parse_redirect("/?state=abc").is_err());
        assert!(parse_redirect("/favicon.ico").is_err());
        assert_eq!(urldecode("a%20b+c%2"), "a b c%2");
        assert_eq!(urlencode("a b/c~é"), "a%20b%2Fc~%C3%A9");
    }

    // Formes tirées de la référence REST de la Picker API (PickingSession,
    // PollingConfig, PickedMediaItem, MediaFile).
    const SESSION_PENDING: &str = r#"{
        "id": "f4c7b8d2-1a2b-4c3d-9e8f-0123456789ab",
        "pickerUri": "https://photos.google.com/picker/session/f4c7b8d2",
        "pollingConfig": { "pollInterval": "0.05s", "timeoutIn": "1800s" },
        "expireTime": "2026-11-14T18:30:00Z",
        "mediaItemsSet": false
    }"#;
    const SESSION_DONE: &str = r#"{
        "id": "f4c7b8d2-1a2b-4c3d-9e8f-0123456789ab",
        "pickerUri": "https://photos.google.com/picker/session/f4c7b8d2",
        "expireTime": "2026-11-14T18:30:00Z",
        "mediaItemsSet": true
    }"#;

    fn items_page(base: &str, names: &[(&str, &str, &str)], next: Option<&str>) -> String {
        let items: Vec<String> = names
            .iter()
            .map(|(id, kind, file)| {
                format!(
                    r#"{{"id":"{id}","createTime":"2024-07-14T10:20:30Z","type":"{kind}",
                    "mediaFile":{{"baseUrl":"{base}/media/{id}","mimeType":"image/jpeg","filename":"{file}",
                    "mediaFileMetadata":{{"width":4032,"height":3024,"cameraMake":"Google","cameraModel":"Pixel 8",
                    "photoMetadata":{{"focalLength":6.9,"apertureFNumber":1.68,"isoEquivalent":50,"exposureTime":"0.001s"}}}}}}}}"#
                )
            })
            .collect();
        let token = next
            .map(|t| format!(r#","nextPageToken":"{t}""#))
            .unwrap_or_default();
        format!(r#"{{"mediaItems":[{}]{token}}}"#, items.join(","))
    }

    #[test]
    fn reference_json_shapes_parse() {
        let pending: Session = serde_json::from_str(SESSION_PENDING).unwrap();
        assert!(!pending.media_items_set);
        assert_eq!(
            pending.picker_uri,
            "https://photos.google.com/picker/session/f4c7b8d2"
        );
        let polling = pending.polling_config.unwrap();
        assert_eq!(
            parse_duration(&polling.poll_interval),
            Some(Duration::from_millis(50))
        );
        assert_eq!(
            parse_duration(&polling.timeout_in),
            Some(Duration::from_secs(1800))
        );
        let done: Session = serde_json::from_str(SESSION_DONE).unwrap();
        assert!(done.media_items_set && done.polling_config.is_none());

        let page: MediaPage = serde_json::from_str(&items_page(
            "https://lh3.googleusercontent.com/p",
            &[("AF1", "PHOTO", "IMG_0001.HEIC")],
            Some("tok"),
        ))
        .unwrap();
        assert_eq!(page.next_page_token.as_deref(), Some("tok"));
        let item = &page.media_items[0];
        assert_eq!((item.id.as_str(), item.kind.as_str()), ("AF1", "PHOTO"));
        assert_eq!(
            item.media_file.base_url,
            "https://lh3.googleusercontent.com/p/media/AF1"
        );
        assert_eq!(cache_name(item), "AF1-IMG_0001.jpg");

        let tokens: Tokens = serde_json::from_str(
            r#"{"access_token":"ya29.a0","expires_in":3599,"refresh_token":"1//0g","scope":"x","token_type":"Bearer"}"#,
        )
        .unwrap();
        assert_eq!(tokens.refresh_token.as_deref(), Some("1//0g"));
        assert_eq!(parse_duration("3.5s"), Some(Duration::from_millis(3500)));
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("-1s"), None);
    }

    #[test]
    fn cache_names_are_safe() {
        let item = |id: &str, file: &str| MediaItem {
            id: id.into(),
            create_time: String::new(),
            kind: "PHOTO".into(),
            media_file: MediaFile {
                filename: file.into(),
                ..Default::default()
            },
        };
        assert_eq!(
            cache_name(&item("../../etc", "../passwd")),
            "....etc-passwd.jpg"
        );
        assert!(!cache_name(&item("a/b", "c/d.png")).contains('/'));
        assert_eq!(cache_name(&item("ID9", "")), "ID9.jpg");
    }

    /// Faux Google : jetons, sessions, liste paginée et téléchargements.
    struct Mock {
        addr: String,
        log: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
    }

    impl Mock {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let log = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (log_, stop_, base) = (log.clone(), stop.clone(), addr.clone());
            std::thread::spawn(move || {
                let mut polls = 0;
                while !stop_.load(Ordering::SeqCst) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    reader.read_line(&mut request_line).unwrap();
                    let mut headers = HashMap::new();
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        let line = line.trim_end();
                        if line.is_empty() {
                            break;
                        }
                        if let Some((k, v)) = line.split_once(": ") {
                            headers.insert(k.to_ascii_lowercase(), v.to_owned());
                        }
                    }
                    let len: usize = headers
                        .get("content-length")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    let mut body = vec![0u8; len];
                    reader.read_exact(&mut body).unwrap();
                    let body = String::from_utf8_lossy(&body).into_owned();
                    let mut parts = request_line.split_ascii_whitespace();
                    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                    let auth = headers.get("authorization").cloned().unwrap_or_default();
                    log_.lock()
                        .unwrap()
                        .push(format!("{method} {target} [{auth}] {body}"));
                    let path = target.split('?').next().unwrap_or("");
                    let authorised = auth == "Bearer ACCESS";
                    let (status, content_type, payload): (&str, &str, Vec<u8>) = match (method, path) {
                        ("POST", "/token") if body.contains("grant_type=authorization_code") && body.contains("code=THECODE") && body.contains("code_verifier=") => (
                            "200 OK",
                            "application/json",
                            br#"{"access_token":"ACCESS","expires_in":3599,"refresh_token":"REFRESH","token_type":"Bearer"}"#.to_vec(),
                        ),
                        ("POST", "/token") if body.contains("grant_type=refresh_token") && body.contains("refresh_token=REFRESH") => (
                            "200 OK",
                            "application/json",
                            br#"{"access_token":"ACCESS","expires_in":3599,"token_type":"Bearer"}"#.to_vec(),
                        ),
                        ("POST", "/token") => (
                            "400 Bad Request",
                            "application/json",
                            br#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#.to_vec(),
                        ),
                        (_, p) if p.starts_with("/v1/") && !authorised => (
                            "401 Unauthorized",
                            "application/json",
                            br#"{"error":{"code":401,"message":"Request had invalid authentication credentials.","status":"UNAUTHENTICATED"}}"#.to_vec(),
                        ),
                        ("POST", "/v1/sessions") => ("200 OK", "application/json", SESSION_PENDING.as_bytes().to_vec()),
                        ("GET", p) if p.starts_with("/v1/sessions/") => {
                            polls += 1;
                            let json = if polls >= 3 { SESSION_DONE } else { SESSION_PENDING };
                            ("200 OK", "application/json", json.as_bytes().to_vec())
                        }
                        ("DELETE", p) if p.starts_with("/v1/sessions/") => ("200 OK", "application/json", b"{}".to_vec()),
                        ("GET", "/v1/mediaItems") if target.contains("pageToken=PAGE2") => (
                            "200 OK",
                            "application/json",
                            items_page(&base, &[("P3", "PHOTO", "trois.jpg"), ("BAD", "PHOTO", "cassee.jpg")], None).into_bytes(),
                        ),
                        ("GET", "/v1/mediaItems") => (
                            "200 OK",
                            "application/json",
                            items_page(&base, &[("P1", "PHOTO", "un.jpg"), ("V1", "VIDEO", "film.mp4"), ("P2", "PHOTO", "deux.heic")], Some("PAGE2")).into_bytes(),
                        ),
                        ("GET", p) if p.starts_with("/media/BAD") => ("404 Not Found", "text/plain", b"gone".to_vec()),
                        ("GET", p) if p.starts_with("/media/") && authorised && p.ends_with("=w2560-h1600") => {
                            let mut png = std::io::Cursor::new(Vec::new());
                            image::RgbImage::from_pixel(8, 5, image::Rgb([9, 99, 199]))
                                .write_to(&mut png, image::ImageFormat::Jpeg)
                                .unwrap();
                            ("200 OK", "image/jpeg", png.into_inner())
                        }
                        _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    let _ = stream.write_all(&payload);
                }
            });
            Self { addr, log, stop }
        }

        fn endpoints(&self) -> Endpoints {
            Endpoints {
                auth: format!("{}/auth", self.addr),
                token: format!("{}/token", self.addr),
                picker: format!("{}/v1", self.addr),
            }
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl Drop for Mock {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    fn options(mock: &Mock, dir: &Path) -> ImportOptions {
        ImportOptions {
            config: GoogleConfig {
                client_id: "CLIENT".into(),
                client_secret: "SECRET".into(),
            },
            endpoints: mock.endpoints(),
            token_path: dir.join("google.json"),
            keyring: false,
            cache_root: dir.join("cache"),
            width: 1280,
            height: 800,
            auth_timeout: Duration::from_secs(10),
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lightning-google-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Joue le rôle du navigateur : suit l'URL d'autorisation jusqu'à la redirection locale.
    fn browser(status: SharedStatus, code: &'static str) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let url = loop {
                let s = status.lock().unwrap().clone();
                if s.state == "awaiting_auth" && !s.url.is_empty() {
                    break s.url;
                }
                assert!(
                    Instant::now() < deadline,
                    "URL d'autorisation jamais publiée"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            let param = |name: &str| -> String {
                let start = url.find(&format!("{name}=")).unwrap() + name.len() + 1;
                let end = url[start..].find('&').map_or(url.len(), |e| start + e);
                urldecode(&url[start..end])
            };
            let redirect = param("redirect_uri");
            let state = param("state");
            assert!(redirect.starts_with("http://127.0.0.1:"), "{redirect}");
            let mut stream = TcpStream::connect(redirect.trim_start_matches("http://")).unwrap();
            // Une requête parasite d'abord, comme le ferait un navigateur.
            write!(stream, "GET /favicon.ico HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut sink = String::new();
            let _ = stream.read_to_string(&mut sink);
            let mut stream = TcpStream::connect(redirect.trim_start_matches("http://")).unwrap();
            write!(
                stream,
                "GET /?state={}&code={code} HTTP/1.1\r\nHost: x\r\n\r\n",
                urlencode(&state)
            )
            .unwrap();
            let mut page = String::new();
            let _ = stream.read_to_string(&mut page);
            assert!(
                page.contains("200 OK") && page.contains("Autorisation reçue"),
                "{page}"
            );
        })
    }

    #[test]
    fn full_import_against_a_mock_google() {
        let mock = Mock::start();
        let dir = tmp("import");
        let opts = options(&mock, &dir);
        let status = SharedStatus::default();
        let cancel = AtomicBool::new(false);
        let user = browser(status.clone(), "THECODE");

        let imported = import(&opts, &status, &cancel).unwrap();
        user.join().unwrap();
        assert_eq!(
            imported, 3,
            "trois photos, la vidéo ignorée, une photo en échec tolérée"
        );

        // Fichiers en cache, lisibles par le diaporama.
        let session_dir = opts.cache_root.join("f4c7b8d2-1a2b-4c3d-9e8f-0123456789ab");
        let mut files: Vec<String> = std::fs::read_dir(&session_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, ["P1-un.jpg", "P2-deux.jpg", "P3-trois.jpg"]);
        let entries =
            crate::slideshow::scan(&[opts.cache_root.to_string_lossy().into_owned()], false);
        assert_eq!(entries.len(), 3);
        assert!(image::open(&entries[0].path).is_ok());

        // Jeton de rafraîchissement enregistré en 0600.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&opts.token_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            TokenStorage::file_only(&opts.token_path).load().as_deref(),
            Some("REFRESH")
        );

        let log = mock.log();
        let token_call = log.iter().find(|l| l.starts_with("POST /token")).unwrap();
        assert!(
            token_call.contains("client_id=CLIENT") && token_call.contains("client_secret=SECRET")
        );
        assert!(token_call.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A"));
        assert!(
            log.iter()
                .any(|l| l.starts_with("POST /v1/sessions [Bearer ACCESS]"))
        );
        assert!(log.iter().any(
            |l| l.contains("sessionId=f4c7b8d2-1a2b-4c3d-9e8f-0123456789ab")
                && l.contains("pageSize=100")
        ));
        assert!(log.iter().any(|l| l.contains("pageToken=PAGE2")));
        assert!(
            log.iter()
                .any(|l| l.starts_with("GET /media/P1=w2560-h1600 [Bearer ACCESS]"))
        );
        assert!(
            !log.iter().any(|l| l.contains("/media/V1")),
            "les vidéos ne sont pas téléchargées"
        );
        assert!(
            log.iter().any(|l| l.starts_with("DELETE /v1/sessions/")),
            "session supprimée"
        );
        assert_eq!(status.lock().unwrap().state, "downloading");

        // Second import : le jeton enregistré suffit, pas de navigateur.
        let before = mock.log().len();
        let status2 = SharedStatus::default();
        assert_eq!(import(&opts, &status2, &cancel).unwrap(), 3);
        let second = &mock.log()[before..];
        assert!(second[0].contains("grant_type=refresh_token"), "{second:?}");
        assert!(!second.iter().any(|l| l.contains("authorization_code")));

        assert_eq!(clear_cache(&opts.cache_root).unwrap(), 3);
        assert_eq!(clear_cache(&opts.cache_root).unwrap(), 0);
        assert!(opts.cache_root.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn errors_are_reported_and_cancellation_works() {
        let mock = Mock::start();
        let dir = tmp("errors");
        let cancel = AtomicBool::new(false);

        // Identifiants absents.
        let mut opts = options(&mock, &dir);
        opts.config = GoogleConfig::default();
        let err = import(&opts, &SharedStatus::default(), &cancel).unwrap_err();
        assert!(err.contains("client_id"), "{err}");

        // Code refusé par le serveur de jetons : le message de Google remonte.
        let opts = options(&mock, &dir);
        let status = SharedStatus::default();
        let user = browser(status.clone(), "WRONG");
        let err = import(&opts, &status, &cancel).unwrap_err();
        user.join().unwrap();
        assert!(
            err.contains("400") && err.contains("expired or revoked"),
            "{err}"
        );
        assert!(!opts.token_path.exists());

        // Jeton enregistré révoqué : on retombe sur l'autorisation, annulable.
        TokenStorage::file_only(&opts.token_path)
            .save("REVOKED")
            .unwrap();
        let importer = Importer::new(true);
        assert_eq!(importer.status().state, "idle");
        let (tx, rx) = std::sync::mpsc::channel();
        importer
            .start(options(&mock, &dir), move |r| tx.send(r).unwrap())
            .unwrap();
        assert!(
            importer
                .start(options(&mock, &dir), |_| {})
                .unwrap_err()
                .contains("déjà en cours")
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while importer.status().url.is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(importer.status().state, "awaiting_auth");
        importer.cancel();
        let result = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(result.unwrap_err(), "import annulé");
        assert_eq!(importer.status().state, "error");
        assert!(!importer.is_running());

        let unconfigured = Importer::new(false);
        assert_eq!(unconfigured.status().state, "unconfigured");
        assert!(
            unconfigured
                .start(opts_unconfigured(&mock, &dir), |_| {})
                .is_err()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn opts_unconfigured(mock: &Mock, dir: &Path) -> ImportOptions {
        let mut opts = options(mock, dir);
        opts.config = GoogleConfig::default();
        opts
    }
}
