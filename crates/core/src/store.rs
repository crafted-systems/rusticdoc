//! Where uploaded templates and fonts live when a service runs as many
//! containers: a directory they share, or an S3-compatible bucket. Nothing here is
//! on until `RUSTICDOC_STORE` names one, because a default on a container's own
//! disk would answer on one replica and not on the next.
//!
//! Each object is written once and never changed, and a metadata object written
//! after the content is what makes it exist, so replicas need no locks or
//! coordination. Templates are read from the store. Fonts have to be files for
//! LibreOffice, so each replica keeps a copy in sync with [`Store::sync_fonts`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{StreamExt as _, TryStreamExt as _, stream};
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};
use object_store::local::LocalFileSystem;
use object_store::path::Path as ObjectPath;
use object_store::prefix::PrefixStore;
use object_store::{BackoffConfig, ClientConfigKey, ObjectStore, ObjectStoreExt as _, RetryConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fonts::{FontFace, ParsedFont};
use crate::job::Document;

/// Where one caller's templates go when every caller shares them.
pub const SHARED_SCOPE: &str = "shared";

/// Most bytes of template content kept in memory.
const CACHE_BYTES: usize = 256 * 1024 * 1024;
/// Objects read at once for one listing or sync.
const CONCURRENT_READS: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found")]
    NotFound,
    /// The store could not be reached or refused. Not for clients: it can name
    /// the bucket and its endpoint.
    #[error("store: {0}")]
    Backend(String),
    /// What came back is not what was written.
    #[error("store integrity: {0}")]
    Integrity(String),
}

pub type StoreResult<T> = std::result::Result<T, StoreError>;

impl From<object_store::Error> for StoreError {
    fn from(e: object_store::Error) -> Self {
        match e {
            object_store::Error::NotFound { .. } => StoreError::NotFound,
            other => StoreError::Backend(other.to_string()),
        }
    }
}

/// A stored template, as clients see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateMeta {
    pub id: String,
    /// The file name, whose extension decides its format.
    pub name: String,
    pub format: String,
    pub size: u64,
    /// Hex SHA-256 of the content.
    pub sha256: String,
    /// RFC 3339.
    pub created_at: String,
}

/// A stored font, as clients see it. Its id is the hash of its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FontMeta {
    pub id: String,
    pub file: String,
    pub size: u64,
    pub faces: Vec<FontFace>,
    /// RFC 3339.
    pub uploaded_at: String,
}

/// One page of a caller's templates.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplatePage {
    pub templates: Vec<TemplateMeta>,
    /// Pass as `cursor` for the next page. Absent on the last.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// What is held for a scope, for quotas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub count: usize,
    pub bytes: u64,
}

/// What one sync did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// The copy on disk changed, so LibreOffice has to look again.
    pub changed: bool,
    /// Fonts in the copy now.
    pub fonts: usize,
    /// Fonts that could not be fetched, to try again next time.
    pub failed: usize,
    /// Fonts whose content does not match their id, left out.
    pub corrupt: usize,
}

/// Template content by scope and id, oldest out first. Entries are safe to keep
/// because content never changes; a delete is seen because metadata is always read.
#[derive(Default)]
struct ContentCache {
    entries: HashMap<String, Bytes>,
    order: VecDeque<String>,
    bytes: usize,
}

impl ContentCache {
    fn get(&mut self, key: &str) -> Option<Bytes> {
        let hit = self.entries.get(key).cloned()?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_string());
        Some(hit)
    }

    fn put(&mut self, key: String, value: Bytes) {
        if value.len() > CACHE_BYTES / 4 || self.entries.contains_key(&key) {
            return;
        }
        self.bytes += value.len();
        self.order.push_back(key.clone());
        self.entries.insert(key, value);
        while self.bytes > CACHE_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(gone) = self.entries.remove(&oldest) {
                self.bytes -= gone.len();
            }
        }
    }

    fn remove(&mut self, key: &str) {
        if let Some(gone) = self.entries.remove(key) {
            self.bytes -= gone.len();
            self.order.retain(|k| k != key);
        }
    }
}

pub struct Store {
    objects: Arc<dyn ObjectStore>,
    /// Which kind of store, for health and logs. Holds no credentials.
    description: String,
    cache: Mutex<ContentCache>,
    /// Metadata of the fonts in the copy. Fonts never change, so it is fetched once.
    known_fonts: Mutex<HashMap<String, FontMeta>>,
    /// One sync at a time.
    syncing: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("description", &self.description)
            .finish()
    }
}

/// A scope as a path segment. Principals are a scheme and a digest, or `public`.
pub fn scope_key(principal: &str) -> String {
    principal
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn is_template_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.hyphenated().to_string() == id)
}

fn is_font_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn template_meta_path(scope: &str, id: &str) -> ObjectPath {
    ObjectPath::from(format!("templates/{scope}/meta/{id}.json"))
}

fn template_content_path(scope: &str, id: &str) -> ObjectPath {
    ObjectPath::from(format!("templates/{scope}/content/{id}"))
}

fn font_meta_path(id: &str) -> ObjectPath {
    ObjectPath::from(format!("fonts/meta/{id}.json"))
}

fn font_data_path(id: &str) -> ObjectPath {
    ObjectPath::from(format!("fonts/data/{id}"))
}

/// The id in a metadata object's name, if it is one.
fn id_of(path: &ObjectPath) -> Option<String> {
    path.filename()?.strip_suffix(".json").map(str::to_string)
}

/// The extension a font's copy gets, from its magic bytes.
fn font_extension(bytes: &[u8]) -> &'static str {
    match bytes.get(..4) {
        Some(b"OTTO") => "otf",
        Some(b"ttcf") => "ttc",
        _ => "ttf",
    }
}

impl Store {
    /// Opens `file:///path` or `s3://bucket/prefix`. Credentials and the endpoint
    /// come from the `AWS_*` variables, or the pod's or instance's role, and never
    /// from the URL, which is logged and shown in errors. Reaches nothing yet.
    pub fn open(url: &str) -> std::result::Result<Self, String> {
        let parsed = url::Url::parse(url).map_err(|e| format!("RUSTICDOC_STORE: {e}"))?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(
                "RUSTICDOC_STORE must not hold credentials; use the AWS_* variables".into(),
            );
        }
        if parsed.query().is_some() {
            return Err("RUSTICDOC_STORE takes no query string".into());
        }
        match parsed.scheme() {
            "file" => {
                let path = parsed
                    .to_file_path()
                    .map_err(|()| format!("RUSTICDOC_STORE: '{url}' is not a usable path"))?;
                create_private_dir(&path)
                    .map_err(|e| format!("RUSTICDOC_STORE: cannot create '{url}': {e}"))?;
                let local = LocalFileSystem::new_with_prefix(&path)
                    .map_err(|e| format!("RUSTICDOC_STORE: {e}"))?
                    .with_automatic_cleanup(true);
                Ok(Self::with_objects(
                    Arc::new(local),
                    format!("file://{}", path.display()),
                ))
            }
            "s3" => {
                let bucket = parsed
                    .host_str()
                    .filter(|b| !b.is_empty())
                    .ok_or("RUSTICDOC_STORE: s3:// needs a bucket")?;
                let s3 = AmazonS3Builder::from_env()
                    .with_bucket_name(bucket)
                    // Set key by key: replacing the client options would drop what
                    // from_env read into them, such as AWS_ALLOW_HTTP.
                    .with_config(
                        AmazonS3ConfigKey::Client(ClientConfigKey::ConnectTimeout),
                        "5s",
                    )
                    .with_config(AmazonS3ConfigKey::Client(ClientConfigKey::Timeout), "120s")
                    // A request has a deadline of its own; do not spend it here.
                    .with_retry(RetryConfig {
                        backoff: BackoffConfig::default(),
                        max_retries: 3,
                        retry_timeout: Duration::from_secs(20),
                    })
                    .build()
                    .map_err(|e| format!("RUSTICDOC_STORE: {e}"))?;
                let prefix = parsed.path().trim_matches('/');
                let description = format!("s3://{bucket}");
                if prefix.is_empty() {
                    Ok(Self::with_objects(Arc::new(s3), description))
                } else {
                    let scoped = PrefixStore::new(s3, ObjectPath::from(prefix));
                    Ok(Self::with_objects(Arc::new(scoped), description))
                }
            }
            other => Err(format!(
                "RUSTICDOC_STORE: '{other}' is not supported; use file:// or s3://"
            )),
        }
    }

    fn with_objects(objects: Arc<dyn ObjectStore>, description: String) -> Self {
        Self {
            objects,
            description,
            cache: Mutex::default(),
            known_fonts: Mutex::default(),
            syncing: tokio::sync::Mutex::new(()),
        }
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    async fn read(&self, path: &ObjectPath) -> StoreResult<Bytes> {
        Ok(self.objects.get(path).await?.bytes().await?)
    }

    async fn read_json<T: serde::de::DeserializeOwned>(&self, path: &ObjectPath) -> StoreResult<T> {
        let bytes = self.read(path).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| StoreError::Integrity(format!("{path} cannot be read: {e}")))
    }

    /// Stores a template under a new id. The content goes first, so metadata
    /// never names something that is not there.
    pub async fn put_template(&self, scope: &str, doc: &Document) -> StoreResult<TemplateMeta> {
        let id = uuid::Uuid::now_v7().hyphenated().to_string();
        let meta = TemplateMeta {
            id: id.clone(),
            name: doc.name.clone(),
            format: doc.extension(),
            size: doc.bytes.len() as u64,
            sha256: digest(&doc.bytes),
            created_at: now(),
        };
        let encoded =
            serde_json::to_vec(&meta).map_err(|e| StoreError::Backend(format!("metadata: {e}")))?;
        self.objects
            .put(&template_content_path(scope, &id), doc.bytes.clone().into())
            .await?;
        self.objects
            .put(&template_meta_path(scope, &id), Bytes::from(encoded).into())
            .await?;
        Ok(meta)
    }

    /// A template's metadata. Always from the store, so every replica sees a delete.
    pub async fn template_meta(&self, scope: &str, id: &str) -> StoreResult<TemplateMeta> {
        if !is_template_id(id) {
            return Err(StoreError::NotFound);
        }
        self.read_json(&template_meta_path(scope, id)).await
    }

    /// A template's content, checked against the hash taken when it was stored.
    pub async fn template_content(&self, scope: &str, id: &str) -> StoreResult<Document> {
        let meta = self.template_meta(scope, id).await?;
        let key = format!("{scope}/{id}");
        let cached = self.cache.lock().expect("template cache").get(&key);
        let bytes = match cached {
            Some(bytes) => bytes,
            None => {
                let bytes = match self.read(&template_content_path(scope, id)).await {
                    Err(StoreError::NotFound) => {
                        return Err(StoreError::Integrity(format!(
                            "template {id} has metadata but no content"
                        )));
                    }
                    other => other?,
                };
                if digest(&bytes) != meta.sha256 {
                    return Err(StoreError::Integrity(format!(
                        "template {id} does not match its hash"
                    )));
                }
                self.cache
                    .lock()
                    .expect("template cache")
                    .put(key, bytes.clone());
                bytes
            }
        };
        Ok(Document::new(meta.name, bytes))
    }

    async fn template_ids(&self, scope: &str) -> StoreResult<Vec<String>> {
        let prefix = ObjectPath::from(format!("templates/{scope}/meta"));
        let listed: Vec<_> = self.objects.list(Some(&prefix)).try_collect().await?;
        let mut ids: Vec<String> = listed.iter().filter_map(|o| id_of(&o.location)).collect();
        // Not every store lists in order. Ids sort by when they were made.
        ids.sort_unstable();
        Ok(ids)
    }

    /// A page of a scope's templates, oldest first.
    pub async fn list_templates(
        &self,
        scope: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> StoreResult<TemplatePage> {
        let ids = self.template_ids(scope).await?;
        let start = cursor.map_or(0, |after| ids.partition_point(|id| id.as_str() <= after));
        let page = &ids[start.min(ids.len())..];
        let shown = &page[..limit.min(page.len())];
        let read: Vec<StoreResult<TemplateMeta>> = stream::iter(shown.to_vec())
            .map(|id: String| async move { self.read_json(&template_meta_path(scope, &id)).await })
            .buffered(CONCURRENT_READS)
            .collect()
            .await;
        let mut templates = Vec::with_capacity(read.len());
        for meta in read {
            match meta {
                Ok(meta) => templates.push(meta),
                // Deleted since the listing.
                Err(StoreError::NotFound) => {}
                Err(other) => return Err(other),
            }
        }
        let next_cursor = (page.len() > shown.len())
            .then(|| shown.last().cloned())
            .flatten();
        Ok(TemplatePage {
            templates,
            next_cursor,
        })
    }

    /// What a scope holds, for its quota. Two replicas storing at once can each
    /// pass it, so a quota can be passed by what is in flight.
    pub async fn template_usage(&self, scope: &str) -> StoreResult<Usage> {
        let prefix = ObjectPath::from(format!("templates/{scope}/content"));
        let listed: Vec<_> = self.objects.list(Some(&prefix)).try_collect().await?;
        Ok(Usage {
            count: listed.len(),
            bytes: listed.iter().map(|o| o.size).sum(),
        })
    }

    /// Deletes a template. Metadata goes first, so it is gone for everyone at once.
    pub async fn delete_template(&self, scope: &str, id: &str) -> StoreResult<()> {
        self.template_meta(scope, id).await?;
        self.objects.delete(&template_meta_path(scope, id)).await?;
        self.cache
            .lock()
            .expect("template cache")
            .remove(&format!("{scope}/{id}"));
        match self.objects.delete(&template_content_path(scope, id)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Stores a font under its hash, so storing it twice changes nothing.
    pub async fn put_font(&self, font: &ParsedFont) -> StoreResult<FontMeta> {
        let meta = FontMeta {
            id: font.id.clone(),
            file: font.file.clone(),
            size: font.bytes.len() as u64,
            faces: font.faces.clone(),
            uploaded_at: now(),
        };
        let encoded =
            serde_json::to_vec(&meta).map_err(|e| StoreError::Backend(format!("metadata: {e}")))?;
        self.objects
            .put(&font_data_path(&font.id), font.bytes.clone().into())
            .await?;
        self.objects
            .put(&font_meta_path(&font.id), Bytes::from(encoded).into())
            .await?;
        Ok(meta)
    }

    /// What the store holds in fonts, for its quota.
    pub async fn font_usage(&self) -> StoreResult<Usage> {
        let prefix = ObjectPath::from("fonts/data");
        let listed: Vec<_> = self.objects.list(Some(&prefix)).try_collect().await?;
        Ok(Usage {
            count: listed.len(),
            bytes: listed.iter().map(|o| o.size).sum(),
        })
    }

    /// Whether the store holds a font, whatever the local copy says.
    pub async fn has_font(&self, id: &str) -> StoreResult<bool> {
        if !is_font_id(id) {
            return Ok(false);
        }
        match self.objects.head(&font_meta_path(id)).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Deletes a font. Each replica drops its copy at its next sync.
    pub async fn delete_font(&self, id: &str) -> StoreResult<()> {
        if !self.has_font(id).await? {
            return Err(StoreError::NotFound);
        }
        self.objects.delete(&font_meta_path(id)).await?;
        match self.objects.delete(&font_data_path(id)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// The fonts in the local copy, with what the store said about them.
    pub fn known_fonts(&self) -> HashMap<String, FontMeta> {
        self.known_fonts.lock().expect("known fonts").clone()
    }

    /// Makes `dir` hold exactly the fonts the store holds: fetching what is new,
    /// checking each against its hash, and removing what was deleted. Only files
    /// named as this function names them are ever removed, so a directory set by
    /// mistake loses nothing else.
    pub async fn sync_fonts(&self, dir: &Path) -> StoreResult<SyncReport> {
        let _one_at_a_time = self.syncing.lock().await;
        create_private_dir(dir)
            .map_err(|e| StoreError::Backend(format!("{}: {e}", dir.display())))?;
        let prefix = ObjectPath::from("fonts/meta");
        let listed: Vec<_> = self.objects.list(Some(&prefix)).try_collect().await?;
        let wanted: HashSet<String> = listed
            .iter()
            .filter_map(|o| id_of(&o.location))
            .filter(|id| is_font_id(id))
            .collect();

        let mut report = SyncReport::default();
        let on_disk =
            local_fonts(dir).map_err(|e| StoreError::Backend(format!("{}: {e}", dir.display())))?;
        for (id, path) in &on_disk {
            if !wanted.contains(id) {
                std::fs::remove_file(path)
                    .map_err(|e| StoreError::Backend(format!("{}: {e}", path.display())))?;
                self.known_fonts.lock().expect("known fonts").remove(id);
                report.changed = true;
            }
        }

        let missing: Vec<String> = wanted
            .iter()
            .filter(|id| !on_disk.contains_key(*id))
            .cloned()
            .collect();
        let fetched: Vec<(String, StoreResult<Bytes>)> = stream::iter(missing)
            .map(|id: String| async move {
                let read = self.read(&font_data_path(&id)).await;
                (id, read)
            })
            .buffer_unordered(4)
            .collect()
            .await;
        for (id, bytes) in fetched {
            let bytes = match bytes {
                Ok(bytes) => bytes,
                Err(e) => {
                    tracing::warn!(font = %id, error = %e, "could not fetch a stored font");
                    report.failed += 1;
                    continue;
                }
            };
            if digest(&bytes) != id || crate::fonts::faces_of(&bytes).is_none() {
                tracing::error!(font = %id, "a stored font does not match its id; left out");
                report.corrupt += 1;
                continue;
            }
            write_font(dir, &id, &bytes)
                .map_err(|e| StoreError::Backend(format!("{}: {e}", dir.display())))?;
            report.changed = true;
        }

        // Metadata for what is here, once each: fonts do not change.
        let unknown: Vec<String> = {
            let known = self.known_fonts.lock().expect("known fonts");
            wanted
                .iter()
                .filter(|id| !known.contains_key(*id))
                .cloned()
                .collect()
        };
        let metas: Vec<StoreResult<FontMeta>> = stream::iter(unknown)
            .map(|id: String| async move { self.read_json::<FontMeta>(&font_meta_path(&id)).await })
            .buffer_unordered(CONCURRENT_READS)
            .collect()
            .await;
        for meta in metas {
            match meta {
                Ok(meta) if wanted.contains(&meta.id) => {
                    self.known_fonts
                        .lock()
                        .expect("known fonts")
                        .insert(meta.id.clone(), meta);
                }
                Ok(_) | Err(StoreError::NotFound) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "could not read a stored font's metadata");
                    report.failed += 1;
                }
            }
        }
        report.fonts = wanted.len().saturating_sub(report.corrupt + report.failed);
        Ok(report)
    }
}

/// Creates a directory only this user can enter, if it is not there yet.
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

/// The fonts already in the copy, by id. Leftovers of an interrupted write go.
fn local_fonts(dir: &Path) -> std::io::Result<HashMap<String, PathBuf>> {
    let mut found = HashMap::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(id) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp"))
            && is_font_id(id)
        {
            std::fs::remove_file(&path)?;
            continue;
        }
        let Some((id, ext)) = name.split_once('.') else {
            continue;
        };
        if is_font_id(id) && matches!(ext, "ttf" | "otf" | "ttc") {
            found.insert(id.to_string(), path);
        }
    }
    Ok(found)
}

/// Writes a font so that a worker starting now sees all of it or none of it.
/// fontconfig passes over names that start with a dot.
fn write_font(dir: &Path, id: &str, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = dir.join(format!(".{id}.tmp"));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(
        &temporary,
        dir.join(format!("{id}.{}", font_extension(bytes))),
    )
}
