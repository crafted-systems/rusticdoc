//! Asset libraries that come with requests: a ZIP of images and stylesheets for
//! one request, unpacked to a directory the render workers read.
//!
//! A library stays after its request, so that a later request from the same scope
//! can name it by its hash, as with templates. Everything here shares one budget of
//! disk. Room is made by removing the libraries nothing uses, the one used longest
//! ago first; a library in use is never removed, and when only those are left a new
//! one is refused.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusticdoc_template::assets::{MAX_NAME_BYTES, SentAssets, copy_name};

use crate::assets;
use crate::error::Error;
use crate::job::Document;
use crate::pdfops::Deadline;

/// Most libraries kept. Small ones take little room and an entry each here.
const MAX_LIBRARIES: usize = 1_000;
/// What a file takes on disk is a multiple of this.
const BLOCK: u64 = 4096;

/// Why a library was not unpacked.
#[derive(Debug)]
pub enum LibraryError {
    /// Every byte of the budget is held by libraries in use.
    Full,
    Failed(Error),
}

impl From<Error> for LibraryError {
    fn from(e: Error) -> Self {
        Self::Failed(e)
    }
}

impl From<std::io::Error> for LibraryError {
    fn from(e: std::io::Error) -> Self {
        Self::Failed(e.into())
    }
}

/// The bytes the libraries hold between them.
#[derive(Debug)]
struct Space {
    max: u64,
    used: Mutex<u64>,
}

/// One unpacked library. Dropping the last handle removes it and gives its room
/// back.
#[derive(Debug)]
struct Unpacked {
    dir: Option<tempfile::TempDir>,
    bytes: u64,
    space: Arc<Space>,
}

impl Unpacked {
    fn path(&self) -> &Path {
        self.dir.as_ref().expect("there until drop").path()
    }
}

impl Drop for Unpacked {
    fn drop(&mut self) {
        // Off the disk before its room is anyone else's.
        drop(self.dir.take());
        *lock(&self.space.used) -= self.bytes;
    }
}

/// By scope and hash, the one used longest ago first.
type Held = Vec<(String, Arc<Unpacked>)>;

/// The libraries this service holds. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Libraries {
    space: Arc<Space>,
    held: Arc<Mutex<Held>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn key(scope: &str, hash: &str) -> String {
    format!("{scope}/{hash}")
}

fn sent(library: Arc<Unpacked>) -> SentAssets {
    SentAssets::new(library.path().to_path_buf(), library)
}

impl Libraries {
    /// Holds at most `max` bytes on disk, the libraries in use included.
    pub fn new(max: u64) -> Self {
        Self {
            space: Arc::new(Space {
                max,
                used: Mutex::new(0),
            }),
            held: Arc::default(),
        }
    }

    /// The library this `scope` sent with the hex SHA-256 `hash`, if it is still
    /// here. A scope is `key` or `public`, as for templates.
    pub fn recall(&self, scope: &str, hash: &str) -> Option<SentAssets> {
        let key = key(scope, &hash.trim().to_ascii_lowercase());
        let mut held = lock(&self.held);
        let at = held.iter().position(|(k, _)| *k == key)?;
        let entry = held.remove(at);
        // What cleans a temp directory may have been here.
        if !entry.1.path().is_dir() {
            return None;
        }
        let library = entry.1.clone();
        held.push(entry);
        Some(sent(library))
    }

    /// Unpacks the ZIP in `zip`, under the rules of an upload, and keeps it for
    /// `scope` to recall. Blocks: it reads the archive and writes files.
    pub fn add(
        &self,
        scope: &str,
        zip: &Document,
        deadline: &Deadline,
    ) -> std::result::Result<SentAssets, LibraryError> {
        if !zip.bytes.starts_with(b"PK") {
            return Err(Error::InvalidRequest(format!(
                "the asset library '{}' is not a ZIP",
                zip.name
            ))
            .into());
        }
        let hash = crate::template::template_hash(&zip.bytes);
        if let Some(library) = self.recall(scope, &hash) {
            return Ok(library);
        }
        let mut unpacked = Unpacked {
            dir: Some(
                tempfile::Builder::new()
                    .prefix("rusticdoc-library-")
                    .tempdir()?,
            ),
            bytes: 0,
            space: self.space.clone(),
        };
        // A directory grows with the names in it.
        self.charge(&mut unpacked, BLOCK)?;
        assets::each_in_zip(zip, |asset| {
            deadline.check()?;
            let size = asset.bytes.len() as u64;
            let room = size.next_multiple_of(BLOCK) + MAX_NAME_BYTES as u64;
            self.charge(&mut unpacked, room)?;
            std::fs::write(unpacked.path().join(copy_name(&asset.name)), &asset.bytes)?;
            Ok::<_, LibraryError>(())
        })?;

        let key = key(scope, &hash);
        let library = Arc::new(unpacked);
        // Dropped after the lock: removing a directory takes a while.
        let mut gone = Vec::new();
        let mut held = lock(&self.held);
        if let Some((_, first)) = held.iter().find(|(k, _)| *k == key) {
            // Another request unpacked the same one meanwhile.
            gone.push(library);
            let first = first.clone();
            drop(held);
            return Ok(sent(first));
        }
        while held.len() >= MAX_LIBRARIES {
            gone.push(held.remove(0).1);
        }
        held.push((key, library.clone()));
        drop(held);
        Ok(sent(library))
    }

    /// Takes `bytes` of the budget for `library`, removing libraries nothing uses
    /// until there is room.
    fn charge(&self, library: &mut Unpacked, bytes: u64) -> std::result::Result<(), LibraryError> {
        if library.bytes + bytes > self.space.max {
            return Err(Error::InvalidRequest(format!(
                "the asset library takes more than the {} bytes of disk this service gives them",
                self.space.max
            ))
            .into());
        }
        loop {
            {
                let mut used = lock(&self.space.used);
                if *used + bytes <= self.space.max {
                    *used += bytes;
                    library.bytes += bytes;
                    return Ok(());
                }
            }
            let idle = {
                let mut held = lock(&self.held);
                // One handle is the one here. `recall` takes another under the lock.
                let at = held.iter().position(|(_, l)| Arc::strong_count(l) == 1);
                at.map(|at| held.remove(at).1)
            };
            match idle {
                Some(idle) => drop(idle),
                None => return Err(LibraryError::Full),
            }
        }
    }

    /// The bytes of disk the libraries hold.
    pub fn bytes(&self) -> u64 {
        *lock(&self.space.used)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::time::Duration;

    use rusticdoc_template::assets::AssetDirs;

    use super::*;

    fn zip_of(entries: &[(&str, &[u8])]) -> Document {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, content) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content).unwrap();
        }
        Document::new("library.zip", zip.finish().unwrap().into_inner())
    }

    fn add(
        libraries: &Libraries,
        scope: &str,
        zip: &Document,
    ) -> std::result::Result<SentAssets, LibraryError> {
        libraries.add(scope, zip, &Deadline::new(Duration::from_secs(30)))
    }

    fn read(library: &SentAssets, name: &str) -> Option<Vec<u8>> {
        let dirs = AssetDirs {
            sent: Some(library.clone()),
            ..AssetDirs::default()
        };
        dirs.read(name).unwrap()
    }

    fn path(library: &SentAssets) -> std::path::PathBuf {
        serde_json::from_value(serde_json::to_value(library).unwrap()).unwrap()
    }

    #[test]
    fn a_library_is_read_by_name_and_recalled_by_hash_in_its_scope() {
        let libraries = Libraries::new(1024 * 1024);
        let zip = zip_of(&[
            ("brand/a.css", b"p { color: red }"),
            ("LICENSE.txt", b"terms"),
        ]);
        let hash = crate::template::template_hash(&zip.bytes);
        assert!(libraries.recall("key", &hash).is_none());
        let library = add(&libraries, "key", &zip).unwrap();
        assert_eq!(read(&library, "brand/a.css").unwrap(), b"p { color: red }");
        assert!(read(&library, "LICENSE.txt").is_none());
        assert!(read(&library, "a.css").is_none());
        let recalled = libraries.recall("key", &hash.to_uppercase()).unwrap();
        assert_eq!(recalled, library);
        assert!(libraries.recall("public", &hash).is_none());
        // Sent again, it is the same copy.
        assert_eq!(add(&libraries, "key", &zip).unwrap(), library);
        assert_ne!(add(&libraries, "public", &zip).unwrap(), library);
        // Removed behind its back, it is a miss, and is unpacked again.
        std::fs::remove_dir_all(path(&library)).unwrap();
        assert!(libraries.recall("key", &hash).is_none());
        let again = add(&libraries, "key", &zip).unwrap();
        assert!(read(&again, "brand/a.css").is_some());
    }

    #[test]
    fn room_is_made_from_what_nothing_uses_and_never_from_what_something_does() {
        let room = 3 * (2 * BLOCK + MAX_NAME_BYTES as u64);
        let libraries = Libraries::new(room);
        let zips: Vec<Document> = (0..5)
            .map(|i| zip_of(&[("a.css", format!("p {{ margin: {i}px }}").as_bytes())]))
            .collect();
        let hash = |i: usize| crate::template::template_hash(&zips[i].bytes);
        let held = |i: usize| libraries.recall("key", &hash(i)).is_some();

        let first = add(&libraries, "key", &zips[0]).unwrap();
        drop(add(&libraries, "key", &zips[1]).unwrap());
        drop(add(&libraries, "key", &zips[2]).unwrap());
        assert_eq!(libraries.bytes(), room);
        // Used again, so the third is now the one used longest ago.
        assert!(held(1));
        let fourth = add(&libraries, "key", &zips[3]).unwrap();
        assert!(held(0) && held(1) && !held(2) && held(3));
        assert_eq!(libraries.bytes(), room);

        let second = libraries.recall("key", &hash(1)).unwrap();
        assert!(matches!(
            add(&libraries, "key", &zips[4]),
            Err(LibraryError::Full)
        ));
        assert_eq!(libraries.bytes(), room, "a refused library holds nothing");
        for library in [&first, &second, &fourth] {
            assert!(read(library, "a.css").is_some());
        }
        drop(second);
        assert!(add(&libraries, "key", &zips[4]).is_ok());
    }

    #[test]
    fn a_library_past_the_budget_or_not_one_is_refused_and_leaves_nothing() {
        let libraries = Libraries::new(64 * 1024);
        let big = zip_of(&[("a.css", b"p {}"), ("big.css", &vec![b' '; 128 * 1024])]);
        for (zip, why) in [
            (&big, "bytes of disk"),
            (&Document::new("logo.svg", &b"<svg/>"[..]), "not a ZIP"),
            (&zip_of(&[("../a.css", b"")]), "not an asset name"),
            (&zip_of(&[("a.css", b""), ("a.png", b"p {}")]), "extension"),
            (&zip_of(&[("notes.txt", b"")]), "no images"),
        ] {
            match add(&libraries, "key", zip) {
                Err(LibraryError::Failed(Error::InvalidRequest(m))) => {
                    assert!(m.contains(why), "{m}");
                }
                other => panic!("{why}: {other:?}"),
            }
            assert_eq!(libraries.bytes(), 0);
        }
        let late = libraries.add(
            "key",
            &zip_of(&[("a.css", b"")]),
            &Deadline::new(Duration::ZERO),
        );
        assert!(matches!(late, Err(LibraryError::Failed(Error::Timeout(_)))));
        assert_eq!(libraries.bytes(), 0);
    }

    #[test]
    fn a_library_leaves_the_disk_with_its_last_reader() {
        let libraries = Libraries::new(1024 * 1024);
        let library = add(&libraries, "key", &zip_of(&[("a.css", b"")])).unwrap();
        let path = path(&library);
        drop(libraries);
        assert!(path.is_dir());
        drop(library);
        assert!(!path.exists());
    }
}
