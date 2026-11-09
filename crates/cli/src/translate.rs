//! `rusticdoc translate`: lists the keys templates pass to `t` in a
//! dictionary for translators, and the language options `render` and
//! `validate` share with the service.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Args;
use rusticdoc_core::template::{is_office_format, is_text_template, translation_keys};
use rusticdoc_core::{Config, Document};
use rusticdoc_template::locale::{check_timezone, normalize_lang};
use rusticdoc_template::translate::{parse_table, parse_translations};

/// Most template files one run reads, so a directory given by mistake (`/`)
/// stops rather than reading everything under it.
const MAX_FILES: usize = 10_000;

#[derive(Args)]
pub struct TranslateArgs {
    /// The language of the dictionary, such as `fr` or `fr-CA`.
    #[arg(long)]
    pub lang: String,
    /// Where `<lang>.json` is created or updated.
    #[arg(long, default_value = "translations")]
    pub out: PathBuf,
    /// Office and text templates, or directories to search for them.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
}

/// What a run did, for the summary line and the tests.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub file: PathBuf,
    pub added: Vec<String>,
    pub kept: usize,
    pub unused: Vec<String>,
}

pub fn run(args: &TranslateArgs) -> anyhow::Result<Outcome> {
    let lang = normalize_lang(&args.lang).with_context(|| {
        format!(
            "--lang {:?} is not a language tag such as fr or fr-CA",
            args.lang
        )
    })?;
    let mut files = Vec::new();
    for path in &args.paths {
        collect(path, &mut files)?;
    }
    if files.is_empty() {
        bail!("no Office or text templates in the paths given");
    }
    let mut used = BTreeSet::new();
    for file in &files {
        let bytes = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("template");
        let keys = translation_keys(&Document::new(name, bytes))
            .with_context(|| format!("read the tags of {}", file.display()))?;
        used.extend(keys);
    }
    std::fs::create_dir_all(&args.out).with_context(|| format!("create {}", args.out.display()))?;
    let target = args.out.join(format!("{lang}.json"));
    let mut table: BTreeMap<String, String> = match std::fs::read(&target) {
        Ok(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("{} is not valid JSON", target.display()))?;
            parse_table(&value, &target.display().to_string()).map_err(anyhow::Error::msg)?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", target.display())),
    };
    let kept = table.len();
    let mut added = Vec::new();
    for key in &used {
        if !table.contains_key(key) {
            table.insert(key.clone(), String::new());
            added.push(key.clone());
        }
    }
    let unused: Vec<String> = table
        .keys()
        .filter(|k| !used.contains(*k))
        .cloned()
        .collect();
    let mut text = serde_json::to_string_pretty(&table)?;
    text.push('\n');
    std::fs::write(&target, text).with_context(|| format!("write {}", target.display()))?;
    Ok(Outcome {
        file: target,
        added,
        kept,
        unused,
    })
}

/// The template files under `path`: the file itself, or every template in a
/// directory and those below it.
fn collect(path: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let meta = std::fs::metadata(path).with_context(|| format!("read {}", path.display()))?;
    if meta.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
        .with_context(|| format!("read {}", path.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for entry in entries {
        let Ok(meta) = std::fs::symlink_metadata(&entry) else {
            continue;
        };
        if meta.is_dir() {
            collect(&entry, out)?;
        } else if meta.is_file() {
            let ext = entry
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if is_office_format(&ext) || is_text_template(&ext) {
                out.push(entry);
            }
        }
        if out.len() > MAX_FILES {
            bail!("more than {MAX_FILES} templates under {}", path.display());
        }
    }
    Ok(())
}

/// The template stage's language options, as the service builds them from
/// `lang`, `translations` and `timezone`, with the dictionaries in
/// `RUSTICDOC_TRANSLATIONS_DIR`.
pub fn render_options(
    config: &Config,
    lang: Option<&str>,
    translations: Option<&Path>,
    timezone: Option<&str>,
) -> anyhow::Result<rusticdoc_template::RenderOptions> {
    let lang = match lang {
        None => None,
        Some(raw) => Some(normalize_lang(raw).with_context(|| {
            format!("--lang {raw:?} is not a language tag such as fr or fr-CA")
        })?),
    };
    if let Some(tz) = timezone {
        check_timezone(tz).map_err(|e| anyhow::anyhow!("--timezone: {e}"))?;
    }
    let translations = match translations {
        None => Default::default(),
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("read {}", path.display()))?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", path.display()))?;
            parse_translations(&value, "--translations").map_err(anyhow::Error::msg)?
        }
    };
    let dictionaries =
        rusticdoc_core::dictionaries::Dictionaries::open(config.translations_dir.clone())
            .map_err(anyhow::Error::msg)?;
    Ok(rusticdoc_template::RenderOptions {
        lang: lang.clone(),
        timezone: timezone.map(str::to_string),
        translations,
        dictionary: dictionaries.for_lang(lang.as_deref()),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(dir: &Path, out: &Path) -> TranslateArgs {
        TranslateArgs {
            lang: "FR".into(),
            out: out.to_path_buf(),
            paths: vec![dir.to_path_buf()],
        }
    }

    #[test]
    fn creates_updates_and_reports_unused_keys() {
        let dir = tempfile::tempdir().unwrap();
        let templates = dir.path().join("templates");
        std::fs::create_dir_all(templates.join("sub")).unwrap();
        std::fs::write(
            templates.join("letter.txt"),
            "{% settings t.greeting.fr=\"Bonjour\" %}{{ \"total\" | t }} {{ (\"due\" | t) ~ \":\" }}",
        )
        .unwrap();
        std::fs::write(templates.join("sub/page.html"), "<p>{{ 'title' | t }}</p>").unwrap();
        std::fs::write(templates.join("notes.pdf"), "not a template").unwrap();
        let out = dir.path().join("translations");

        let first = run(&args(&templates, &out)).unwrap();
        assert_eq!(first.added, ["due", "greeting", "title", "total"]);
        assert_eq!(first.kept, 0);
        let written: BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(out.join("fr.json")).unwrap()).unwrap();
        assert!(written.values().all(String::is_empty));

        // A translator fills one in; a template drops a key and adds another.
        let mut filled = written.clone();
        filled.insert("total".into(), "Montant".into());
        std::fs::write(out.join("fr.json"), serde_json::to_string(&filled).unwrap()).unwrap();
        std::fs::write(
            templates.join("sub/page.html"),
            "<p>{{ 'heading' | t }}</p>",
        )
        .unwrap();
        let second = run(&args(&templates, &out)).unwrap();
        assert_eq!(second.added, ["heading"]);
        assert_eq!(second.kept, 4);
        assert_eq!(second.unused, ["title"]);
        let written: BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(out.join("fr.json")).unwrap()).unwrap();
        assert_eq!(written["total"], "Montant");
        assert!(written.contains_key("title"), "unused keys stay");
    }

    #[test]
    fn a_bad_language_or_no_templates_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = args(dir.path(), &dir.path().join("out"));
        assert!(
            run(&a)
                .unwrap_err()
                .to_string()
                .contains("no Office or text templates")
        );
        a.lang = "French".into();
        assert!(
            run(&a)
                .unwrap_err()
                .to_string()
                .contains("not a language tag")
        );
    }
}
