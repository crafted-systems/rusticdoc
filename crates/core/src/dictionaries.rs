//! The service's translation dictionaries: `<lang>.json` files in
//! `RUSTICDOC_TRANSLATIONS_DIR`, each a flat object of keys and texts, that
//! `t` consults below a request's own `translations`.
//!
//! They are read at start, where a bad file stops the service so that a
//! broken deploy is noticed, and read again when a file changes. A request
//! looks at the directory at most once a second, comparing each file's size
//! and modification time, so nothing is parsed per request. A file that turns
//! bad while the service runs is logged and its last good version kept.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rusticdoc_template::locale::{fallbacks, normalize_lang};
use rusticdoc_template::translate::{self, Translations};

/// Largest dictionary file.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Most dictionary files.
pub const MAX_FILES: usize = 200;
/// Most bytes of dictionaries in all. Without a request language every
/// dictionary goes along with a render, to whichever worker runs it.
pub const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024;
/// How often a request may look at the directory for changes.
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// A file as last read.
struct Loaded {
    modified: Option<SystemTime>,
    len: u64,
    table: BTreeMap<String, String>,
}

#[derive(Default)]
struct State {
    files: BTreeMap<String, Loaded>,
    checked: Option<Instant>,
    snapshot: Arc<Translations>,
}

/// The dictionaries directory and what was last read from it.
#[derive(Default)]
pub struct Dictionaries {
    dir: Option<PathBuf>,
    state: Mutex<State>,
}

impl std::fmt::Debug for Dictionaries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dictionaries")
            .field("dir", &self.dir)
            .finish()
    }
}

impl Dictionaries {
    /// Reads every dictionary in `dir`. A missing directory, a file that is not
    /// a flat object of strings, a name that is not a language, or a file over
    /// the limits is an error naming the file.
    pub fn open(dir: Option<PathBuf>) -> Result<Self, String> {
        let this = Self {
            dir,
            state: Mutex::new(State::default()),
        };
        if let Some(dir) = &this.dir {
            let mut state = this.state.lock().expect("dictionaries");
            refresh(dir, &mut state, true)?;
        }
        Ok(this)
    }

    /// No dictionaries.
    pub fn none() -> Self {
        Self::default()
    }

    /// Every dictionary, read again first when a file changed.
    pub fn current(&self) -> Arc<Translations> {
        let Some(dir) = &self.dir else {
            return Arc::default();
        };
        let mut state = self.state.lock().expect("dictionaries");
        if state.checked.is_none_or(|t| t.elapsed() >= CHECK_EVERY) {
            // Errors here are logged per file inside; the last good version stays.
            let _ = refresh(dir, &mut state, false);
        }
        state.snapshot.clone()
    }

    /// The dictionaries a render in `lang` can use: that language and those it
    /// falls back to, or every one when the request names no language, since
    /// the template's own may then apply.
    pub fn for_lang(&self, lang: Option<&str>) -> Translations {
        let all = self.current();
        match lang.and_then(normalize_lang) {
            None => (*all).clone(),
            Some(lang) => fallbacks(&lang)
                .filter_map(|tag| all.get(tag).map(|t| (tag.to_string(), t.clone())))
                .collect(),
        }
    }
}

/// Brings `state` up to date with `dir`. At start (`strict`) the first
/// problem is returned; afterwards problems are logged and skipped.
fn refresh(dir: &Path, state: &mut State, strict: bool) -> Result<(), String> {
    state.checked = Some(Instant::now());
    let fail = |message: String| -> Result<(), String> {
        if strict {
            Err(message)
        } else {
            tracing::warn!(%message, "translations not reloaded");
            Ok(())
        }
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => return fail(format!("RUSTICDOC_TRANSLATIONS_DIR {}: {e}", dir.display())),
    };
    let mut seen: BTreeMap<String, (PathBuf, Option<SystemTime>, u64)> = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let Some(lang) = normalize_lang(stem) else {
            fail(format!(
                "{} is not named after a language, such as fr.json or fr-CA.json",
                path.display()
            ))?;
            continue;
        };
        let Ok(meta) = entry.metadata() else { continue };
        if seen.len() >= MAX_FILES {
            fail(format!(
                "{} holds more than {MAX_FILES} dictionaries",
                dir.display()
            ))?;
            break;
        }
        seen.insert(lang, (path, meta.modified().ok(), meta.len()));
    }
    let mut changed = false;
    let before = state.files.len();
    state.files.retain(|lang, _| seen.contains_key(lang));
    changed |= state.files.len() != before;
    let mut total: u64 = 0;
    for (lang, (path, modified, len)) in seen {
        if let Some(old) = state.files.get(&lang)
            && old.modified == modified
            && old.len == len
        {
            total += len;
            continue;
        }
        if len > MAX_FILE_BYTES {
            fail(format!(
                "{} is {len} bytes; a dictionary may be {MAX_FILE_BYTES}",
                path.display()
            ))?;
            continue;
        }
        if total + len > MAX_TOTAL_BYTES {
            fail(format!(
                "the dictionaries in {} pass {MAX_TOTAL_BYTES} bytes at {}",
                dir.display(),
                path.display()
            ))?;
            continue;
        }
        match read(&path) {
            Ok(table) => {
                total += len;
                state.files.insert(
                    lang,
                    Loaded {
                        modified,
                        len,
                        table,
                    },
                );
                changed = true;
            }
            Err(message) => fail(message)?,
        }
    }
    if changed || strict {
        let snapshot: Translations = state
            .files
            .iter()
            .map(|(lang, l)| (lang.clone(), l.table.clone()))
            .collect();
        state.snapshot = Arc::new(snapshot);
        tracing::info!(
            languages = state.files.len(),
            dir = %dir.display(),
            "translations loaded"
        );
    }
    Ok(())
}

fn read(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    translate::parse_table(&value, &path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reloads_and_keeps_the_last_good_version() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fr.json"), r#"{"total": "Montant"}"#).unwrap();
        std::fs::write(
            dir.path().join("fr-CA.json"),
            r#"{"total": "Montant total"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        let d = Dictionaries::open(Some(dir.path().to_path_buf())).unwrap();
        assert_eq!(d.current()["fr"]["total"], "Montant");
        let some = d.for_lang(Some("fr-CA"));
        assert_eq!(some.keys().collect::<Vec<_>>(), ["fr", "fr-ca"]);
        assert_eq!(d.for_lang(Some("de")).len(), 0);
        assert_eq!(d.for_lang(None).len(), 2);

        // A change is seen once the check interval has passed.
        std::fs::write(
            dir.path().join("fr.json"),
            r#"{"total": "Somme", "tax": "Taxe"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("de.json"), r#"{"total": "Summe"}"#).unwrap();
        d.state.lock().unwrap().checked = None;
        let now = d.current();
        assert_eq!(now["fr"]["total"], "Somme");
        assert_eq!(now["de"]["total"], "Summe");

        // A file that turns bad keeps its last good version; a removed one goes.
        std::fs::write(
            dir.path().join("fr.json"),
            r#"{"total": 1, "x": "much longer now"}"#,
        )
        .unwrap();
        std::fs::remove_file(dir.path().join("de.json")).unwrap();
        d.state.lock().unwrap().checked = None;
        let now = d.current();
        assert_eq!(now["fr"]["total"], "Somme");
        assert!(!now.contains_key("de"));
    }

    #[test]
    fn a_bad_file_stops_the_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fr.json"), r#"{"total": ["no"]}"#).unwrap();
        let err = Dictionaries::open(Some(dir.path().to_path_buf())).unwrap_err();
        assert!(
            err.contains("fr.json") && err.contains("texts are strings"),
            "{err}"
        );
        std::fs::write(dir.path().join("fr.json"), "{").unwrap();
        let err = Dictionaries::open(Some(dir.path().to_path_buf())).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
        std::fs::remove_file(dir.path().join("fr.json")).unwrap();
        std::fs::write(dir.path().join("french.json"), "{}").unwrap();
        let err = Dictionaries::open(Some(dir.path().to_path_buf())).unwrap_err();
        assert!(err.contains("not named after a language"), "{err}");
        let err = Dictionaries::open(Some(dir.path().join("missing"))).unwrap_err();
        assert!(err.contains("RUSTICDOC_TRANSLATIONS_DIR"), "{err}");
    }
}
