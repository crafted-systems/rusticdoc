//! Assets a service holds: images and stylesheets that templates and pages
//! name instead of carrying. They come from a mounted directory and from a
//! replica's copy of the uploaded ones, and the mounted one wins where both
//! have a name.
//!
//! A name is a relative path of plain characters. It is checked before it goes
//! near a file system, so neither a template nor its data can name anything
//! outside the two directories.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use minijinja::value::{Object, ObjectRepr, Value};
use minijinja::{Error, ErrorKind, State, UndefinedBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Largest asset. The same as the largest image in data.
pub const MAX_ASSET_BYTES: u64 = 20 * 1024 * 1024;
/// Longest asset name, its folders included.
pub const MAX_NAME_BYTES: usize = 200;
/// Deepest folder an asset may sit in.
pub const MAX_DEPTH: usize = 8;
/// Most entries looked at to list a mounted directory.
const MAX_LISTED: usize = 10_000;
/// What a stored asset's copy is named, after the hash of its name.
const COPY_EXTENSION: &str = "asset";

/// What an asset is, by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Image,
    Stylesheet,
}

/// The kind of asset a file name is for, or `None` for one that cannot be one.
pub fn kind_of(name: &str) -> Option<AssetKind> {
    let (_, ext) = name.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => Some(AssetKind::Image),
        "css" => Some(AssetKind::Stylesheet),
        _ => None,
    }
}

/// Checks a name: folders and a file separated by `/`, each of ASCII letters,
/// digits, `.`, `_` and `-`, none starting with a dot, and an extension an
/// asset can have.
pub fn check_name(name: &str) -> Result<AssetKind, String> {
    if name.len() > MAX_NAME_BYTES {
        return Err(format!(
            "an asset name holds at most {MAX_NAME_BYTES} bytes"
        ));
    }
    let parts: Vec<&str> = name.split('/').collect();
    if parts.len() > MAX_DEPTH + 1 {
        return Err(format!("{name:?} is more than {MAX_DEPTH} folders deep"));
    }
    let plain = |part: &&str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    if !parts.iter().all(plain) {
        return Err(format!(
            "{name:?} is not an asset name: use letters, digits, '.', '_' and '-', with '/' \
             between folders and no part starting with a dot"
        ));
    }
    // A store in a directory could not hold both `a.png` and `a.png/b.png`.
    if parts[..parts.len() - 1]
        .iter()
        .any(|folder| kind_of(folder).is_some())
    {
        return Err(format!("{name:?} has a folder named as a file"));
    }
    kind_of(name).ok_or_else(|| {
        format!("{name:?} is not an image (PNG, JPEG, GIF, SVG, WebP) or a stylesheet (CSS)")
    })
}

/// Checks that the bytes are what the name says, as far as their first bytes
/// tell. What reads an asset checks the rest.
pub fn check_content(name: &str, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 > MAX_ASSET_BYTES {
        return Err(format!("{name:?} is larger than {MAX_ASSET_BYTES} bytes"));
    }
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    let ok = match ext.to_ascii_lowercase().as_str() {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        "svg" => std::str::from_utf8(bytes).is_ok_and(|text| text.contains("<svg")),
        "css" => std::str::from_utf8(bytes).is_ok(),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{name:?} does not hold what its extension says"))
    }
}

/// The file a stored asset is copied to: a hash of its name, so that a copy is
/// flat and nothing in a name reaches the file system.
pub fn copy_name(name: &str) -> String {
    format!(
        "{}.{COPY_EXTENSION}",
        hex::encode(Sha256::digest(name.as_bytes()))
    )
}

/// True for a file name [`copy_name`] gives.
pub fn is_copy_name(file: &str) -> bool {
    file.strip_suffix(COPY_EXTENSION)
        .and_then(|f| f.strip_suffix('.'))
        .is_some_and(|hash| {
            hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
}

/// A file in the mounted directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedAsset {
    pub name: String,
    pub size: u64,
}

/// Where assets are read from. Files are read when they are used, so a change
/// to either directory is seen by the next render.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetDirs {
    /// The mounted directory. It need not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounted: Option<PathBuf>,
    /// This replica's copy of the uploaded assets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored: Option<PathBuf>,
}

impl AssetDirs {
    pub fn is_empty(&self) -> bool {
        self.mounted.is_none() && self.stored.is_none()
    }

    /// The mounted file an asset name stands for, if there is one.
    pub fn mounted_file(&self, name: &str) -> Option<PathBuf> {
        check_name(name).ok()?;
        self.mounted
            .as_ref()
            .map(|dir| dir.join(name))
            .filter(|path| path.is_file())
    }

    /// The file an asset name stands for: the mounted one, else the copy of
    /// the uploaded one.
    pub fn find(&self, name: &str) -> Option<PathBuf> {
        check_name(name).ok()?;
        self.mounted_file(name).or_else(|| {
            self.stored
                .as_ref()
                .map(|dir| dir.join(copy_name(name)))
                .filter(|path| path.is_file())
        })
    }

    /// An asset's bytes, or `None` when there is none of that name.
    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(path) = self.find(name) else {
            return Ok(None);
        };
        // The error says which asset, never where it is on disk.
        let failed = |e: std::io::Error| format!("asset {name:?} cannot be read: {}", e.kind());
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(MAX_ASSET_BYTES + 1).read_to_end(&mut bytes))
            .map_err(failed)?;
        if bytes.len() as u64 > MAX_ASSET_BYTES {
            return Err(format!(
                "asset {name:?} is larger than {MAX_ASSET_BYTES} bytes"
            ));
        }
        Ok(Some(bytes))
    }

    /// The assets in the mounted directory, by name. Files with other names or
    /// extensions are passed over. Links are followed, as Kubernetes mounts a
    /// ConfigMap through them.
    pub fn list_mounted(&self) -> Vec<MountedAsset> {
        fn walk(
            dir: &Path,
            prefix: &str,
            depth: usize,
            seen: &mut usize,
            out: &mut Vec<MountedAsset>,
        ) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                *seen += 1;
                if *seen > MAX_LISTED {
                    return;
                }
                let file = entry.file_name();
                let Some(file) = file.to_str().filter(|f| !f.starts_with('.')) else {
                    continue;
                };
                let name = format!("{prefix}{file}");
                let path = entry.path();
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                if meta.is_dir() {
                    if depth < MAX_DEPTH {
                        walk(&path, &format!("{name}/"), depth + 1, seen, out);
                    }
                } else if meta.is_file() && check_name(&name).is_ok() {
                    out.push(MountedAsset {
                        name,
                        size: meta.len(),
                    });
                }
            }
        }
        let mut out = Vec::new();
        if let Some(dir) = &self.mounted {
            walk(dir, "", 0, &mut 0, &mut out);
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

/// What `asset("logo.png")` gives an Office template, for `image` and a
/// barcode's `logo` to read.
#[derive(Debug)]
pub(crate) struct Asset {
    name: String,
    dirs: Arc<AssetDirs>,
}

impl Asset {
    pub(crate) fn read(&self) -> Result<Vec<u8>, String> {
        self.dirs
            .read(&self.name)?
            .ok_or_else(|| missing(&self.name))
    }
}

impl Object for Asset {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

fn missing(name: &str) -> String {
    format!("there is no asset {name:?}; mount it or upload it")
}

fn fail(message: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::InvalidOperation, format!("asset: {message}"))
}

/// The error for printing an asset as text in an Office template.
pub(crate) fn printed() -> Error {
    fail("an asset is printed with the image filter, or given to barcode as its logo")
}

/// Adds `asset(name)`. In an Office template it is a value for `image` and
/// `barcode`; in a text template it is the name itself, for a page's `src`. A
/// name with no asset is an error in both.
pub(crate) fn add_function(
    env: &mut minijinja::Environment<'static>,
    dirs: &AssetDirs,
    office: bool,
) {
    let dirs = Arc::new(dirs.clone());
    env.add_function(
        "asset",
        move |state: &State, name: Value| -> Result<Value, Error> {
            if name.is_undefined() {
                return match state.undefined_behavior() {
                    UndefinedBehavior::Strict => Err(Error::new(
                        ErrorKind::UndefinedError,
                        "asset: the name is undefined",
                    )),
                    _ => Ok(Value::UNDEFINED),
                };
            }
            let name = name
                .as_str()
                .ok_or_else(|| fail("the name must be a string"))?;
            check_name(name).map_err(fail)?;
            if dirs.find(name).is_none() {
                return Err(fail(missing(name)));
            }
            Ok(if office {
                Value::from_object(Asset {
                    name: name.to_string(),
                    dirs: dirs.clone(),
                })
            } else if matches!(state.auto_escape(), minijinja::AutoEscape::Html) {
                // Nothing in a name needs escaping, but `/` would be.
                Value::from_safe_string(name.to_string())
            } else {
                Value::from(name)
            })
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain_relative_paths() {
        for good in ["logo.png", "brand/Logo-2.SVG", "a/b/c.css", "x.tar.gif"] {
            assert!(check_name(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "logo",
            "/logo.png",
            "../logo.png",
            "a/../logo.png",
            "a//logo.png",
            "a\\logo.png",
            ".hidden.png",
            ".git/logo.png",
            "lo go.png",
            "logo.png\0",
            "logo.js",
            "file:logo.png",
            "a.png/logo.png",
            "a/b/c/d/e/f/g/h/i/j.png",
        ] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        assert!(check_name(&format!("{}.png", "a".repeat(200))).is_err());
    }

    #[test]
    fn content_has_to_match_the_extension() {
        assert!(check_content("a.png", b"\x89PNG\r\n\x1a\nrest").is_ok());
        assert!(check_content("a.png", b"<svg/>").is_err());
        assert!(check_content("a.svg", b"<?xml?><svg/>").is_ok());
        assert!(check_content("a.css", b"p { color: red }").is_ok());
        assert!(check_content("a.css", &[0xff, 0xfe]).is_err());
    }

    #[test]
    fn the_mount_wins_and_nothing_outside_is_read() {
        let root = tempfile::tempdir().unwrap();
        let (mount, stored) = (root.path().join("mount"), root.path().join("stored"));
        std::fs::create_dir_all(mount.join("brand")).unwrap();
        std::fs::create_dir_all(&stored).unwrap();
        std::fs::write(root.path().join("secret.css"), "secret").unwrap();
        std::fs::write(mount.join("brand/a.css"), "mounted").unwrap();
        std::fs::write(mount.join(".hidden.css"), "hidden").unwrap();
        std::fs::write(mount.join("notes.txt"), "notes").unwrap();
        std::fs::write(stored.join(copy_name("brand/a.css")), "stored").unwrap();
        std::fs::write(stored.join(copy_name("b.css")), "stored b").unwrap();
        let dirs = AssetDirs {
            mounted: Some(mount.clone()),
            stored: Some(stored),
        };
        let text = |name: &str| {
            dirs.read(name)
                .unwrap()
                .map(|b| String::from_utf8(b).unwrap())
        };
        assert_eq!(text("brand/a.css").as_deref(), Some("mounted"));
        assert_eq!(text("b.css").as_deref(), Some("stored b"));
        assert_eq!(text("../secret.css"), None);
        assert_eq!(text(".hidden.css"), None);
        assert_eq!(text("brand"), None);
        let listed: Vec<String> = dirs.list_mounted().into_iter().map(|a| a.name).collect();
        assert_eq!(listed, ["brand/a.css"]);
        // Read when used: a change needs no reload.
        std::fs::remove_file(mount.join("brand/a.css")).unwrap();
        assert_eq!(text("brand/a.css").as_deref(), Some("stored"));
        assert!(AssetDirs::default().read("b.css").unwrap().is_none());
    }

    #[test]
    fn a_mounted_file_over_the_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("big.png")).unwrap();
        file.set_len(MAX_ASSET_BYTES + 1).unwrap();
        let dirs = AssetDirs {
            mounted: Some(dir.path().to_path_buf()),
            stored: None,
        };
        assert!(dirs.read("big.png").unwrap_err().contains("larger than"));
    }

    #[test]
    fn copy_names_are_recognised() {
        assert!(is_copy_name(&copy_name("brand/a.css")));
        assert!(!is_copy_name("a.asset"));
        assert!(!is_copy_name("notes.txt"));
    }
}
