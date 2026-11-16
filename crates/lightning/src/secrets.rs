//! Jeton de rafraîchissement Google : trousseau (Secret Service) ou fichier.
//!
//! Le trousseau de la session (GNOME Trousseau, KeePassXC…) est préféré ; s'il
//! est absent ou verrouillé (service sans session graphique), le jeton reste
//! dans un fichier lisible par l'utilisateur seul. Un jeton trouvé dans le
//! fichier alors que le trousseau répond y est déplacé (migration).
//!
//! Le client Secret Service est écrit directement avec zbus (déjà utilisé par
//! le démon) plutôt qu'avec une bibliothèque dédiée : quatre appels suffisent.
//! La session est `plain` : le secret ne transite que sur le bus de session de
//! l'utilisateur. Aucun secret n'est écrit dans les journaux.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, Value};

/// Un emplacement de secret.
pub trait SecretStore {
    /// Secret enregistré, s'il y en a un.
    fn load(&self) -> Result<Option<String>, String>;
    /// Enregistre (ou remplace) le secret.
    fn save(&self, secret: &str) -> Result<(), String>;
    /// Nom lisible pour les journaux.
    fn name(&self) -> &'static str;
}

// ------------------------------------------------------------------ fichier

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    refresh_token: String,
}

/// Fichier JSON en mode 0600 (`google.json`).
pub struct FileStore {
    pub path: PathBuf,
}

impl FileStore {
    fn remove(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

impl SecretStore for FileStore {
    fn load(&self) -> Result<Option<String>, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{} : {e}", self.path.display())),
        };
        let stored: Stored = serde_json::from_str(&text)
            .map_err(|_| format!("{} : contenu illisible", self.path.display()))?;
        Ok((!stored.refresh_token.is_empty()).then_some(stored.refresh_token))
    }

    fn save(&self, secret: &str) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        let write = || -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&self.path)?;
            let stored = Stored {
                refresh_token: secret.to_owned(),
            };
            file.write_all(serde_json::to_string_pretty(&stored)?.as_bytes())
        };
        write().map_err(|e| format!("{} : {e}", self.path.display()))
    }

    fn name(&self) -> &'static str {
        "fichier"
    }
}

// ----------------------------------------------------------- Secret Service

const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
/// Secret au format Secret Service : session, paramètres, valeur, type MIME.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

/// Élément du trousseau, repéré par ses attributs.
pub struct SecretServiceStore {
    connection: Connection,
    attributes: HashMap<String, String>,
    label: String,
}

impl SecretServiceStore {
    /// Trousseau du bus de session ; `attributes` identifient l'élément.
    pub fn session(attributes: &[(&str, &str)], label: &str) -> Result<Self, String> {
        let connection = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.method_timeout(std::time::Duration::from_secs(5)).build())
            .map_err(|e| format!("bus de session : {e}"))?;
        Ok(Self::with_connection(connection, attributes, label))
    }

    pub fn with_connection(
        connection: Connection,
        attributes: &[(&str, &str)],
        label: &str,
    ) -> Self {
        Self {
            connection,
            attributes: attributes
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            label: label.to_owned(),
        }
    }

    fn proxy(&self, path: &str, iface: &'static str) -> Result<Proxy<'_>, String> {
        zbus::blocking::proxy::Builder::new(&self.connection)
            .destination(SERVICE)
            .and_then(|b| b.path(path.to_owned()))
            .and_then(|b| b.interface(iface))
            .map(|b| b.cache_properties(zbus::proxy::CacheProperties::No))
            .and_then(|b| b.build())
            .map_err(explain)
    }

    fn open_session(&self) -> Result<OwnedObjectPath, String> {
        let (_, session): (zbus::zvariant::OwnedValue, OwnedObjectPath) = self
            .proxy(SERVICE_PATH, SERVICE_IFACE)?
            .call("OpenSession", &("plain", Value::from("")))
            .map_err(explain)?;
        Ok(session)
    }

    /// Déverrouille sans invite ; un déverrouillage qui demanderait une
    /// invite est traité comme « trousseau indisponible ».
    fn unlock(&self, objects: &[OwnedObjectPath]) -> Result<(), String> {
        if objects.is_empty() {
            return Ok(());
        }
        let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = self
            .proxy(SERVICE_PATH, SERVICE_IFACE)?
            .call("Unlock", &(objects,))
            .map_err(explain)?;
        if prompt.as_str() == "/" {
            Ok(())
        } else {
            Err("trousseau verrouillé".into())
        }
    }

    fn search(&self) -> Result<Vec<OwnedObjectPath>, String> {
        let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = self
            .proxy(SERVICE_PATH, SERVICE_IFACE)?
            .call("SearchItems", &(&self.attributes,))
            .map_err(explain)?;
        self.unlock(&locked)?;
        Ok(unlocked.into_iter().chain(locked).collect())
    }

    fn default_collection(&self) -> Result<OwnedObjectPath, String> {
        let path: OwnedObjectPath = self
            .proxy(SERVICE_PATH, SERVICE_IFACE)?
            .call("ReadAlias", &("default",))
            .map_err(explain)?;
        if path.as_str() == "/" {
            return Err("aucun trousseau par défaut".into());
        }
        self.unlock(std::slice::from_ref(&path))?;
        Ok(path)
    }
}

fn explain(e: zbus::Error) -> String {
    match &e {
        zbus::Error::MethodError(name, detail, _) => {
            format!("{} {}", name.as_str(), detail.as_deref().unwrap_or(""))
                .trim()
                .to_owned()
        }
        _ => e.to_string(),
    }
}

impl SecretStore for SecretServiceStore {
    fn load(&self) -> Result<Option<String>, String> {
        let items = self.search()?;
        let Some(item) = items.first() else {
            return Ok(None);
        };
        let session = self.open_session()?;
        let secrets: HashMap<OwnedObjectPath, Secret> = self
            .proxy(SERVICE_PATH, SERVICE_IFACE)?
            .call("GetSecrets", &(vec![item.clone()], &session))
            .map_err(explain)?;
        let Some((_, _, value, _)) = secrets.get(item) else {
            return Ok(None);
        };
        let text = String::from_utf8(value.clone()).map_err(|_| "secret non UTF-8".to_owned())?;
        Ok((!text.is_empty()).then_some(text))
    }

    fn save(&self, secret: &str) -> Result<(), String> {
        let collection = self.default_collection()?;
        let session = self.open_session()?;
        let mut properties: HashMap<&str, Value<'_>> = HashMap::new();
        properties.insert(
            "org.freedesktop.Secret.Item.Label",
            Value::from(self.label.as_str()),
        );
        properties.insert(
            "org.freedesktop.Secret.Item.Attributes",
            Value::from(self.attributes.clone()),
        );
        let value: Secret = (
            session,
            Vec::new(),
            secret.as_bytes().to_vec(),
            "text/plain".to_owned(),
        );
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = self
            .proxy(collection.as_str(), COLLECTION_IFACE)?
            .call("CreateItem", &(properties, value, true))
            .map_err(explain)?;
        if prompt.as_str() != "/" {
            return Err("le trousseau demande une confirmation".into());
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "trousseau"
    }
}

// -------------------------------------------------------- choix et migration

/// Jeton de rafraîchissement : trousseau d'abord, fichier en repli.
pub struct TokenStorage {
    pub keyring: Option<Box<dyn SecretStore + Send>>,
    pub file: FileStore,
}

impl TokenStorage {
    /// Fichier seul (tests, démonstration).
    pub fn file_only(path: &Path) -> Self {
        Self {
            keyring: None,
            file: FileStore {
                path: path.to_owned(),
            },
        }
    }

    /// Trousseau de la session si le bus répond, sinon fichier seul.
    pub fn standard(path: &Path) -> Self {
        let keyring = SecretServiceStore::session(
            &[
                ("application", "cloud.wizops.Lightning"),
                ("type", "google-refresh-token"),
            ],
            lightning_i18n::tr("Lightning : accès à Google Photos"),
        )
        .map_err(|e| warn!("trousseau indisponible ({e}) : jeton Google dans un fichier"))
        .ok()
        .map(|s| Box::new(s) as Box<dyn SecretStore + Send>);
        Self {
            keyring,
            file: FileStore {
                path: path.to_owned(),
            },
        }
    }

    /// Jeton enregistré. Celui du fichier passe dans le trousseau si possible.
    pub fn load(&self) -> Option<String> {
        let from_file = || match self.file.load() {
            Ok(token) => token,
            Err(e) => {
                warn!("jeton Google : {e}");
                None
            }
        };
        let Some(keyring) = &self.keyring else {
            return from_file();
        };
        match keyring.load() {
            Ok(Some(token)) => Some(token),
            Ok(None) => {
                let token = from_file()?;
                // Migration : le fichier n'est supprimé qu'une fois le trousseau écrit.
                match keyring.save(&token) {
                    Ok(()) => {
                        info!("jeton Google déplacé du fichier vers le trousseau");
                        if let Err(e) = self.file.remove() {
                            warn!("{} non supprimé : {e}", self.file.path.display());
                        }
                    }
                    Err(e) => warn!("jeton Google laissé dans le fichier (trousseau : {e})"),
                }
                Some(token)
            }
            Err(e) => {
                warn!("trousseau illisible ({e}) : jeton Google lu dans le fichier");
                from_file()
            }
        }
    }

    /// Enregistre le jeton : trousseau, sinon fichier. Renvoie l'emplacement utilisé.
    pub fn save(&self, token: &str) -> Result<&'static str, String> {
        if let Some(keyring) = &self.keyring {
            match keyring.save(token) {
                Ok(()) => {
                    // Plus de copie en clair une fois le trousseau écrit.
                    if let Err(e) = self.file.remove() {
                        warn!("{} non supprimé : {e}", self.file.path.display());
                    }
                    return Ok(keyring.name());
                }
                Err(e) => warn!("trousseau inutilisable ({e}) : jeton Google dans un fichier"),
            }
        }
        self.file.save(token).map(|()| self.file.name())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Trousseau en mémoire ; `broken` simule un trousseau absent ou verrouillé.
    #[derive(Clone, Default)]
    pub struct MemoryStore {
        pub value: Arc<Mutex<Option<String>>>,
        pub broken: bool,
    }

    impl SecretStore for MemoryStore {
        fn load(&self) -> Result<Option<String>, String> {
            if self.broken {
                return Err("trousseau verrouillé".into());
            }
            Ok(self.value.lock().unwrap().clone())
        }

        fn save(&self, secret: &str) -> Result<(), String> {
            if self.broken {
                return Err("trousseau verrouillé".into());
            }
            *self.value.lock().unwrap() = Some(secret.to_owned());
            Ok(())
        }

        fn name(&self) -> &'static str {
            "mémoire"
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lightning-secrets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("google.json")
    }

    fn storage(path: &Path, keyring: Option<MemoryStore>) -> TokenStorage {
        TokenStorage {
            keyring: keyring.map(|k| Box::new(k) as Box<dyn SecretStore + Send>),
            file: FileStore {
                path: path.to_owned(),
            },
        }
    }

    #[test]
    fn file_store_is_private_and_roundtrips() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp("file");
        let store = FileStore { path: path.clone() };
        assert_eq!(store.load(), Ok(None));
        store.save("REFRESH").unwrap();
        assert_eq!(store.load(), Ok(Some("REFRESH".into())));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::write(&path, "pas du json").unwrap();
        let err = store.load().unwrap_err();
        assert!(
            !err.contains("pas du json"),
            "contenu jamais recopié : {err}"
        );
        store.remove().unwrap();
        store.remove().unwrap();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn keyring_is_preferred_and_file_is_migrated() {
        let path = tmp("migrate");
        FileStore { path: path.clone() }.save("ANCIEN").unwrap();
        let keyring = MemoryStore::default();
        let s = storage(&path, Some(keyring.clone()));
        // Lecture : le jeton du fichier passe dans le trousseau, le fichier disparaît.
        assert_eq!(s.load().as_deref(), Some("ANCIEN"));
        assert_eq!(keyring.value.lock().unwrap().as_deref(), Some("ANCIEN"));
        assert!(!path.exists());
        assert_eq!(s.load().as_deref(), Some("ANCIEN"));
        // Écriture : trousseau, aucune copie en clair.
        assert_eq!(s.save("NOUVEAU").unwrap(), "mémoire");
        assert_eq!(keyring.value.lock().unwrap().as_deref(), Some("NOUVEAU"));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn falls_back_to_the_file_without_a_usable_keyring() {
        for keyring in [
            None,
            Some(MemoryStore {
                broken: true,
                ..Default::default()
            }),
        ] {
            let path = tmp("fallback");
            let s = storage(&path, keyring);
            assert_eq!(s.load(), None);
            assert_eq!(s.save("JETON").unwrap(), "fichier");
            assert!(path.exists());
            assert_eq!(s.load().as_deref(), Some("JETON"));
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
        // Trousseau qui répond mais refuse l'écriture : le fichier est conservé.
        let path = tmp("readonly");
        FileStore { path: path.clone() }.save("GARDÉ").unwrap();
        struct ReadOnly;
        impl SecretStore for ReadOnly {
            fn load(&self) -> Result<Option<String>, String> {
                Ok(None)
            }
            fn save(&self, _: &str) -> Result<(), String> {
                Err("refus".into())
            }
            fn name(&self) -> &'static str {
                "lecture seule"
            }
        }
        let s = TokenStorage {
            keyring: Some(Box::new(ReadOnly)),
            file: FileStore { path: path.clone() },
        };
        assert_eq!(s.load().as_deref(), Some("GARDÉ"));
        assert!(path.exists(), "pas de migration sans écriture réussie");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    // ---------------------------------------- faux Secret Service sur bus privé

    type Items = Arc<Mutex<Vec<(HashMap<String, String>, Vec<u8>)>>>;

    struct FakeService {
        items: Items,
        locked: bool,
    }

    fn item_path(i: usize) -> OwnedObjectPath {
        OwnedObjectPath::try_from(format!("/org/freedesktop/secrets/collection/login/{i}")).unwrap()
    }

    #[zbus::interface(name = "org.freedesktop.Secret.Service")]
    impl FakeService {
        #[zbus(out_args("output", "result"))]
        fn open_session(
            &self,
            algorithm: &str,
            _input: zbus::zvariant::OwnedValue,
        ) -> zbus::fdo::Result<(zbus::zvariant::OwnedValue, OwnedObjectPath)> {
            if algorithm != "plain" {
                return Err(zbus::fdo::Error::NotSupported(algorithm.into()));
            }
            Ok((
                Value::from("").try_to_owned().unwrap(),
                OwnedObjectPath::try_from("/org/freedesktop/secrets/session/1").unwrap(),
            ))
        }

        #[zbus(out_args("unlocked", "locked"))]
        fn search_items(
            &self,
            attributes: HashMap<String, String>,
        ) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
            let found: Vec<OwnedObjectPath> = self
                .items
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .filter(|(_, (attrs, _))| attributes.iter().all(|(k, v)| attrs.get(k) == Some(v)))
                .map(|(i, _)| item_path(i))
                .collect();
            if self.locked {
                (Vec::new(), found)
            } else {
                (found, Vec::new())
            }
        }

        #[zbus(out_args("unlocked", "prompt"))]
        fn unlock(
            &self,
            _objects: Vec<OwnedObjectPath>,
        ) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
            // Verrouillé : déverrouillage seulement par une invite.
            let prompt = if self.locked {
                "/org/freedesktop/secrets/prompt/1"
            } else {
                "/"
            };
            (Vec::new(), OwnedObjectPath::try_from(prompt).unwrap())
        }

        fn read_alias(&self, name: &str) -> OwnedObjectPath {
            OwnedObjectPath::try_from(if name == "default" {
                "/org/freedesktop/secrets/collection/login"
            } else {
                "/"
            })
            .unwrap()
        }

        fn get_secrets(
            &self,
            items: Vec<OwnedObjectPath>,
            session: OwnedObjectPath,
        ) -> HashMap<OwnedObjectPath, Secret> {
            let stored = self.items.lock().unwrap();
            items
                .into_iter()
                .filter_map(|path| {
                    let i: usize = path.as_str().rsplit('/').next()?.parse().ok()?;
                    let (_, value) = stored.get(i)?;
                    Some((
                        path,
                        (
                            session.clone(),
                            Vec::new(),
                            value.clone(),
                            "text/plain".into(),
                        ),
                    ))
                })
                .collect()
        }
    }

    struct FakeCollection {
        items: Items,
    }

    #[zbus::interface(name = "org.freedesktop.Secret.Collection")]
    impl FakeCollection {
        #[zbus(out_args("item", "prompt"))]
        fn create_item(
            &self,
            properties: HashMap<String, zbus::zvariant::OwnedValue>,
            secret: Secret,
            replace: bool,
        ) -> zbus::fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
            let attributes: HashMap<String, String> = properties
                .get("org.freedesktop.Secret.Item.Attributes")
                .and_then(|v| v.try_clone().ok())
                .and_then(|v| HashMap::try_from(v).ok())
                .ok_or_else(|| zbus::fdo::Error::InvalidArgs("attributs".into()))?;
            assert!(properties.contains_key("org.freedesktop.Secret.Item.Label"));
            assert_eq!(secret.3, "text/plain");
            let mut items = self.items.lock().unwrap();
            let existing = items.iter().position(|(a, _)| *a == attributes);
            let index = match existing {
                Some(i) if replace => {
                    items[i].1 = secret.2;
                    i
                }
                _ => {
                    items.push((attributes, secret.2));
                    items.len() - 1
                }
            };
            Ok((item_path(index), OwnedObjectPath::try_from("/").unwrap()))
        }
    }

    fn serve_fake(
        bus: &crate::daemon::dbus::tests::PrivateBus,
        locked: bool,
    ) -> (Connection, Items) {
        let items = Items::default();
        let connection = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .serve_at(
                SERVICE_PATH,
                FakeService {
                    items: items.clone(),
                    locked,
                },
            )
            .unwrap()
            .serve_at(
                "/org/freedesktop/secrets/collection/login",
                FakeCollection {
                    items: items.clone(),
                },
            )
            .unwrap()
            .name(SERVICE)
            .unwrap()
            .build()
            .unwrap();
        (connection, items)
    }

    #[test]
    fn secret_service_client_on_a_private_bus() {
        let Some(bus) = crate::daemon::dbus::tests::PrivateBus::start() else {
            eprintln!("dbus-daemon introuvable : test Secret Service sauté");
            return;
        };
        let attrs = [("application", "test"), ("type", "jeton")];
        // Pas de trousseau sur le bus : erreur, donc repli sur le fichier.
        let absent = SecretServiceStore::with_connection(bus.connect(), &attrs, "Test");
        assert!(absent.load().is_err());
        assert!(absent.save("x").is_err());

        let (_service, items) = serve_fake(&bus, false);
        let store = SecretServiceStore::with_connection(bus.connect(), &attrs, "Test");
        assert_eq!(store.load(), Ok(None));
        store.save("PREMIER").unwrap();
        assert_eq!(store.load(), Ok(Some("PREMIER".into())));
        store.save("SECOND").unwrap();
        assert_eq!(store.load(), Ok(Some("SECOND".into())));
        assert_eq!(
            items.lock().unwrap().len(),
            1,
            "élément remplacé, pas dupliqué"
        );
        let other = SecretServiceStore::with_connection(
            bus.connect(),
            &[("application", "autre")],
            "Autre",
        );
        assert_eq!(other.load(), Ok(None));

        // Migration de bout en bout : fichier → trousseau.
        let path = tmp("dbus");
        FileStore { path: path.clone() }.save("DU-FICHIER").unwrap();
        let migrating = SecretServiceStore::with_connection(
            bus.connect(),
            &[("application", "migration")],
            "Migration",
        );
        let storage = TokenStorage {
            keyring: Some(Box::new(migrating)),
            file: FileStore { path: path.clone() },
        };
        assert_eq!(storage.load().as_deref(), Some("DU-FICHIER"));
        assert!(!path.exists());
        assert_eq!(storage.load().as_deref(), Some("DU-FICHIER"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn locked_keyring_is_treated_as_unavailable() {
        let Some(bus) = crate::daemon::dbus::tests::PrivateBus::start() else {
            return;
        };
        let (_service, items) = serve_fake(&bus, true);
        items.lock().unwrap().push((
            [("application".to_owned(), "test".to_owned())].into(),
            b"X".to_vec(),
        ));
        let store =
            SecretServiceStore::with_connection(bus.connect(), &[("application", "test")], "Test");
        assert!(store.load().unwrap_err().contains("verrouillé"));
        assert!(store.save("Y").unwrap_err().contains("verrouillé"));
        let path = tmp("locked");
        let storage = TokenStorage {
            keyring: Some(Box::new(store)),
            file: FileStore { path: path.clone() },
        };
        assert_eq!(storage.save("Z").unwrap(), "fichier");
        assert_eq!(storage.load().as_deref(), Some("Z"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
