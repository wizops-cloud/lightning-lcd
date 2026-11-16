//! Traductions de lightning-lcd.
//!
//! Le texte source est le français, écrit tel quel dans le code et passé à
//! [`tr`] ou [`trf`] ; la traduction anglaise est cherchée dans un catalogue au
//! format PO (`po/*.po`), embarqué dans le binaire.
//!
//! Choix : pas de gettext/libintl. Le format PO reste celui des outils de
//! traduction (Poedit, Weblate…), mais le catalogue est compilé dans le
//! programme : rien à installer dans `/usr/share/locale` (installation dans
//! `~/.local`, archive, RPM et Flatpak identiques) ni dépendance système. Un
//! texte absent du catalogue s'affiche en français.
//!
//! Langue : `LANGUAGE`, puis `LC_ALL`, `LC_MESSAGES`, `LANG`. Français si la
//! variable commence par `fr`, ou en l'absence de langue (`C`, `POSIX`, vide) ;
//! anglais pour toute autre langue.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// Langue de l'interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Fr,
    En,
}

/// Catalogues anglais, un par partie du programme.
const CATALOGS: &[&str] = &[
    include_str!("../po/en-app.po"),
    include_str!("../po/en-daemon.po"),
];

static LANG: AtomicU8 = AtomicU8::new(0);

fn english() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| {
        CATALOGS
            .iter()
            .flat_map(|po| parse_po(po))
            .filter(|(_, en)| !en.is_empty())
            .collect()
    })
}

/// Langue d'après une valeur de type `fr_FR.UTF-8` ou `en_US:fr`.
pub fn lang_from(value: &str) -> Option<Lang> {
    let first = value.split(':').find(|v| !v.is_empty())?;
    let code = first.split(['_', '.', '@']).next().unwrap_or(first);
    match code {
        "" | "C" | "POSIX" => None,
        "fr" => Some(Lang::Fr),
        _ => Some(Lang::En),
    }
}

/// Langue d'après l'environnement (voir la documentation du module).
pub fn detect(get: impl Fn(&str) -> Option<String>) -> Lang {
    // LANGUAGE n'est pris en compte que si une locale est réglée (comme gettext).
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| get(k).filter(|v| !v.is_empty()));
    let Some(locale) = locale else {
        return Lang::Fr;
    };
    get("LANGUAGE")
        .filter(|_| lang_from(&locale).is_some())
        .and_then(|v| lang_from(&v))
        .or_else(|| lang_from(&locale))
        .unwrap_or(Lang::Fr)
}

/// Règle la langue d'après l'environnement du processus ; à appeler au démarrage.
pub fn init() -> Lang {
    let lang = detect(|k| std::env::var(k).ok());
    set(lang);
    lang
}

/// Règle la langue (tests, options).
pub fn set(lang: Lang) {
    LANG.store(
        match lang {
            Lang::Fr => 0,
            Lang::En => 1,
        },
        Ordering::SeqCst,
    );
}

/// Langue en vigueur (français par défaut, avant [`init`]).
pub fn lang() -> Lang {
    match LANG.load(Ordering::SeqCst) {
        1 => Lang::En,
        _ => Lang::Fr,
    }
}

/// Traduction d'un texte source français.
pub fn tr(fr: &'static str) -> &'static str {
    match lang() {
        Lang::Fr => fr,
        Lang::En => english().get(fr).map_or(fr, String::as_str),
    }
}

/// Traduction avec paramètres nommés : `trf("{n} photos", &[("n", &n)])`.
pub fn trf(fr: &'static str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    fill(tr(fr), args)
}

/// Remplace les `{nom}` d'un modèle.
pub fn fill(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = template.to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// Nombre décimal avec le séparateur de la langue (virgule en français).
pub fn decimal(value: f64, digits: usize) -> String {
    let text = format!("{value:.digits$}");
    match lang() {
        Lang::Fr => text.replace('.', ","),
        Lang::En => text,
    }
}

/// Entrées `(msgid, msgstr)` d'un fichier PO (sous-ensemble : chaînes simples
/// et continuées, échappements `\"`, `\\`, `\n`, `\t` ; pas de pluriels).
pub fn parse_po(text: &str) -> Vec<(String, String)> {
    #[derive(PartialEq)]
    enum Field {
        None,
        Id,
        Str,
    }
    let mut entries = Vec::new();
    let (mut id, mut msg, mut field) = (String::new(), String::new(), Field::None);
    let mut flush = |id: &mut String, msg: &mut String| {
        if !id.is_empty() {
            entries.push((std::mem::take(id), std::mem::take(msg)));
        }
        id.clear();
        msg.clear();
    };
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("msgid ") {
            flush(&mut id, &mut msg);
            id = unquote(rest);
            field = Field::Id;
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            msg = unquote(rest);
            field = Field::Str;
        } else if line.starts_with('"') {
            match field {
                Field::Id => id.push_str(&unquote(line)),
                Field::Str => msg.push_str(&unquote(line)),
                Field::None => {}
            }
        } else if line.is_empty() || line.starts_with('#') {
            field = Field::None;
        }
    }
    flush(&mut id, &mut msg);
    entries
}

fn unquote(s: &str) -> String {
    let inner = s
        .trim()
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or("");
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Paramètres `{nom}` d'un modèle, triés.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut names: Vec<String> = text
        .split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(n, _)| n.to_owned()))
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    names.sort();
    names
}

/// Textes passés à `tr("…")` / `trf("…"` dans un fichier source Rust.
pub fn source_strings(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in ["tr(", "trf("] {
        let mut rest = code;
        while let Some(pos) = rest.find(marker) {
            // `tr(` doit être un appel isolé (pas `str(`, `attr(`…).
            let before = rest[..pos].chars().next_back();
            rest = &rest[pos + marker.len()..];
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            // rustfmt peut passer à la ligne avant le littéral.
            let Some(literal) = rest.trim_start().strip_prefix('"') else {
                continue;
            };
            if let Some(text) = rust_string(literal) {
                found.push(text);
            }
        }
    }
    found
}

/// Contenu d'un littéral Rust commençant juste après le guillemet ouvrant.
fn rust_string(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                // Continuation de ligne : la fin de ligne et l'indentation sautent.
                '\n' => {
                    let rest: String = chars.clone().collect();
                    let trimmed = rest.trim_start();
                    let skip = rest.chars().count() - trimmed.chars().count();
                    for _ in 0..skip {
                        chars.next();
                    }
                }
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_detection() {
        let env = |pairs: &'static [(&str, &str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(detect(env(&[])), Lang::Fr, "aucune langue : français");
        assert_eq!(detect(env(&[("LANG", "C.UTF-8")])), Lang::Fr);
        assert_eq!(detect(env(&[("LANG", "fr_FR.UTF-8")])), Lang::Fr);
        assert_eq!(detect(env(&[("LANG", "en_US.UTF-8")])), Lang::En);
        assert_eq!(
            detect(env(&[("LANG", "de_DE.UTF-8")])),
            Lang::En,
            "autre : anglais"
        );
        assert_eq!(
            detect(env(&[("LANG", "fr_FR.UTF-8"), ("LC_ALL", "en_GB.UTF-8")])),
            Lang::En
        );
        assert_eq!(
            detect(env(&[
                ("LANG", "en_US.UTF-8"),
                ("LC_MESSAGES", "fr_CA.UTF-8")
            ])),
            Lang::Fr
        );
        assert_eq!(
            detect(env(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "fr:en")])),
            Lang::Fr,
            "LANGUAGE l'emporte"
        );
        assert_eq!(
            detect(env(&[("LANG", "C"), ("LANGUAGE", "en")])),
            Lang::Fr,
            "LANGUAGE ignoré sans locale"
        );
        assert_eq!(lang_from("fr_BE@euro"), Some(Lang::Fr));
        assert_eq!(lang_from("POSIX"), None);
    }

    #[test]
    fn po_parsing() {
        let po = r#"
# commentaire
msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"

#: ui.rs
msgid "Tableau de bord"
msgstr "Dashboard"

msgid "Ligne "
"continuée « {n} »"
msgstr "Continued "
"line \"{n}\""

msgid "Pas encore traduit"
msgstr ""
"#;
        let entries = parse_po(po);
        assert_eq!(
            entries,
            [
                ("Tableau de bord".to_owned(), "Dashboard".to_owned()),
                (
                    "Ligne continuée « {n} »".to_owned(),
                    "Continued line \"{n}\"".to_owned()
                ),
                ("Pas encore traduit".to_owned(), String::new()),
            ]
        );
    }

    #[test]
    fn translation_and_formatting() {
        set(Lang::Fr);
        assert_eq!(tr("Texte absent du catalogue"), "Texte absent du catalogue");
        assert_eq!(decimal(2.5, 1), "2,5");
        set(Lang::En);
        assert_eq!(tr("Texte absent du catalogue"), "Texte absent du catalogue");
        assert_eq!(decimal(2.5, 1), "2.5");
        set(Lang::Fr);
        assert_eq!(
            fill("{n} photos dans {d}", &[("n", &3), ("d", &"Vacances")]),
            "3 photos dans Vacances"
        );
        assert_eq!(placeholders("{b} et {a}, {{x"), ["a", "b"]);
    }

    #[test]
    fn source_scanning() {
        let code = r#"
            let a = tr("Accueil");
            let e = trf(
                "Passé à la ligne {n}",
                &[("n", &n)],
            );
            let b = trf("{n} photos", &[("n", &n)]);
            let c = attr("pas moi");
            let d = tr("Avec \"guillemets\" et \
                        continuation");
        "#;
        assert_eq!(
            source_strings(code),
            [
                "Accueil",
                "Avec \"guillemets\" et continuation",
                "Passé à la ligne {n}",
                "{n} photos"
            ]
        );
    }

    /// Chaque texte marqué dans les sources a sa traduction anglaise, avec les
    /// mêmes paramètres ; le catalogue ne garde pas de texte disparu du code.
    #[test]
    fn catalog_covers_every_marked_string() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut sources = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if !path.ends_with("lightning-i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|e| e == "rs") {
                    sources.push(path);
                }
            }
        }
        let mut marked = std::collections::BTreeSet::new();
        for path in &sources {
            let code = std::fs::read_to_string(path).unwrap();
            for text in source_strings(&code) {
                marked.insert(text);
            }
        }
        let mut catalog: HashMap<String, String> = HashMap::new();
        for (fr, en) in CATALOGS.iter().flat_map(|po| parse_po(po)) {
            if let Some(other) = catalog.get(&fr) {
                assert_eq!(other, &en, "« {fr} » traduit de deux façons");
            }
            catalog.insert(fr, en);
        }
        let missing: Vec<&String> = marked
            .iter()
            .filter(|t| catalog.get(*t).is_none_or(|en| en.is_empty()))
            .collect();
        assert!(missing.is_empty(), "textes sans traduction : {missing:#?}");
        for (fr, en) in &catalog {
            assert_eq!(placeholders(fr), placeholders(en), "paramètres de « {fr} »");
        }
        let stale: Vec<&String> = catalog.keys().filter(|k| !marked.contains(*k)).collect();
        assert!(stale.is_empty(), "traductions orphelines : {stale:#?}");
    }
}
