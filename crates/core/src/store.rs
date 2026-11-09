//! Where uploaded templates and fonts live when a service runs as many
//! containers: a directory they share, or an S3-compatible bucket. Nothing here is
//! on until `RUSTICDOC_STORE` names one, because a default on a container's own
//! disk would answer on one replica and not on the next.
//!
//! Each object is written once and never changed, and a metadata object written
//! after the content is what makes it exist, so replicas need no locks or
//! coordination. Aliases and background jobs keep to that too: each change is a
//! new object, and the newest one is what holds. Templates are read from the store. Fonts have to be files for
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

pub fn is_template_id(id: &str) -> bool {
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
        create_owned_private_dir(dir)
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

/// Most versions one alias keeps. Deleting the alias starts it again.
pub const MAX_ALIAS_VERSIONS: usize = 1000;

/// One version of an alias: which template it named from when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasVersion {
    /// 1 for the first template the alias named, counted since it was last
    /// deleted.
    pub version: usize,
    pub template_id: String,
    /// RFC 3339.
    pub set_at: String,
}

/// An alias and the template it names now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasMeta {
    pub name: String,
    pub template_id: String,
    pub version: usize,
    /// RFC 3339: when it was pointed at this template.
    pub set_at: String,
}

/// An alias with every version it has had, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasHistory {
    pub name: String,
    pub template_id: String,
    pub version: usize,
    pub set_at: String,
    pub versions: Vec<AliasVersion>,
}

/// One page of a caller's aliases.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasPage {
    pub aliases: Vec<AliasMeta>,
    /// Pass as `cursor` for the next page. Absent on the last.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// What an alias record says, as stored.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AliasRecord {
    template_id: String,
    set_at: String,
}

/// An alias's records, from a listing: each a time-ordered id, and whether it
/// set the alias (`true`) or deleted it.
#[derive(Debug, Default)]
struct AliasRecords {
    /// Sorted oldest first.
    records: Vec<(String, bool)>,
}

impl AliasRecords {
    /// The records since the last deletion, which make up the alias as it is.
    fn live(&self) -> &[(String, bool)] {
        let start = self
            .records
            .iter()
            .rposition(|(_, set)| !set)
            .map_or(0, |at| at + 1);
        &self.records[start..]
    }
}

/// An alias record's id and kind, from its object's file name.
fn alias_file(file: &str) -> Option<(&str, bool)> {
    let (id, set) = if let Some(id) = file.strip_suffix(".json") {
        (id, true)
    } else {
        (file.strip_suffix(".gone")?, false)
    };
    is_template_id(id).then_some((id, set))
}

/// Where a job is, from the caller's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
}

impl JobStatus {
    /// The state object's name: later states sort after earlier ones.
    fn object(self) -> &'static str {
        match self {
            JobStatus::Queued => "1-queued.json",
            JobStatus::Running => "2-running.json",
            JobStatus::Done => "3-done.json",
            JobStatus::Failed => "3-failed.json",
        }
    }

    pub fn ended(self) -> bool {
        matches!(self, JobStatus::Done | JobStatus::Failed)
    }
}

/// The file a finished job made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobResult {
    pub name: String,
    pub content_type: String,
    pub size: u64,
    /// Hex SHA-256 of the content.
    pub sha256: String,
}

/// A background job, as each of its states records it in full.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub id: String,
    pub status: JobStatus,
    /// RFC 3339, to the millisecond.
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// When it is gone: set once it ends, or for one lost on the way, its
    /// time to run plus the time to keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// A job that has not ended by then never will: the replica running it
    /// stopped.
    pub runs_until: String,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<JobResult>,
    /// What a failed job's request would have answered: `error`, `message`
    /// and `status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
}

/// A time as jobs record it: RFC 3339 to the millisecond.
pub fn job_time(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn parse_time(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

fn chrono_duration(d: Duration) -> chrono::Duration {
    chrono::Duration::from_std(d).unwrap_or(chrono::Duration::hours(1))
}

impl JobRecord {
    /// A new job, queued now, that should end within `limit`.
    pub fn queued(id: String, request_id: String, limit: Duration) -> Self {
        let now = chrono::Utc::now();
        Self {
            id,
            status: JobStatus::Queued,
            created_at: job_time(now),
            started_at: None,
            finished_at: None,
            expires_at: None,
            runs_until: job_time(now + chrono_duration(limit)),
            request_id,
            result: None,
            error: None,
        }
    }

    /// Marks the job ended now, to be kept for `ttl`.
    pub fn ended(&mut self, status: JobStatus, ttl: Duration) {
        let now = chrono::Utc::now();
        self.status = status;
        self.finished_at = Some(job_time(now));
        self.expires_at = Some(job_time(now + chrono_duration(ttl)));
    }

    /// The job as a caller should see it at `now`, or `None` once it has
    /// expired. One that ran past its time without ending is reported as
    /// failed, since nothing will ever finish it.
    pub fn as_of(mut self, now: chrono::DateTime<chrono::Utc>, ttl: Duration) -> Option<Self> {
        if !self.status.ended() {
            let until = parse_time(&self.runs_until)?;
            if now <= until {
                return Some(self);
            }
            self.status = JobStatus::Failed;
            self.expires_at = Some(job_time(until + chrono_duration(ttl)));
            self.error = Some(serde_json::json!({
                "error": "job_lost",
                "message": "the job did not finish in its time, most likely because the container running it stopped; send the request again",
                "status": 503,
            }));
        }
        let expires = match self.expires_at.as_deref().and_then(parse_time) {
            Some(at) => at,
            None => parse_time(self.finished_at.as_deref()?)? + chrono_duration(ttl),
        };
        (now < expires).then_some(self)
    }
}

fn alias_prefix(scope: &str, name: &str) -> ObjectPath {
    ObjectPath::from(format!("aliases/{scope}/{name}"))
}

fn alias_path(scope: &str, name: &str, id: &str, set: bool) -> ObjectPath {
    let kind = if set { "json" } else { "gone" };
    ObjectPath::from(format!("aliases/{scope}/{name}/{id}.{kind}"))
}

fn job_prefix(scope: &str, id: &str) -> ObjectPath {
    ObjectPath::from(format!("jobs/{scope}/{id}"))
}

fn job_result_path(scope: &str, id: &str) -> ObjectPath {
    ObjectPath::from(format!("jobs/{scope}/{id}/result"))
}

/// An alias: 1 to 64 letters, digits, `.`, `_` and `-`, starting with a letter
/// or digit, and not shaped like a template id, so that wherever an id is
/// accepted an alias can be told from one.
pub fn is_alias_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !is_template_id(name)
}

/// A job id: what this store makes, and nothing that could be a path.
pub fn is_job_id(id: &str) -> bool {
    is_template_id(id)
}

/// A new job id, ordered by time like template ids.
pub fn new_job_id() -> String {
    uuid::Uuid::now_v7().hyphenated().to_string()
}

impl Store {
    /// An alias's records, as listed.
    async fn alias_records(&self, scope: &str, name: &str) -> StoreResult<AliasRecords> {
        let listed: Vec<_> = self
            .objects
            .list(Some(&alias_prefix(scope, name)))
            .try_collect()
            .await?;
        let mut records: Vec<(String, bool)> = listed
            .iter()
            .filter_map(|o| alias_file(o.location.filename()?))
            .map(|(id, set)| (id.to_string(), set))
            .collect();
        // Ids sort by when they were made.
        records.sort_unstable();
        Ok(AliasRecords { records })
    }

    async fn alias_record(&self, scope: &str, name: &str, id: &str) -> StoreResult<AliasRecord> {
        self.read_json(&alias_path(scope, name, id, true)).await
    }

    /// The template an alias names now, or the one its `version` named.
    pub async fn resolve_alias(
        &self,
        scope: &str,
        name: &str,
        version: Option<usize>,
    ) -> StoreResult<String> {
        if !is_alias_name(name) {
            return Err(StoreError::NotFound);
        }
        let records = self.alias_records(scope, name).await?;
        let live = records.live();
        let id = match version {
            None => live.last(),
            Some(v) => v.checked_sub(1).and_then(|at| live.get(at)),
        };
        let Some((id, _)) = id else {
            return Err(StoreError::NotFound);
        };
        Ok(self.alias_record(scope, name, id).await?.template_id)
    }

    /// An alias with its versions, oldest first.
    pub async fn alias_history(&self, scope: &str, name: &str) -> StoreResult<AliasHistory> {
        if !is_alias_name(name) {
            return Err(StoreError::NotFound);
        }
        let records = self.alias_records(scope, name).await?;
        let live: Vec<String> = records.live().iter().map(|(id, _)| id.clone()).collect();
        let read: Vec<StoreResult<AliasRecord>> = stream::iter(live)
            .map(|id: String| async move { self.alias_record(scope, name, &id).await })
            .buffered(CONCURRENT_READS)
            .collect()
            .await;
        let mut versions = Vec::with_capacity(read.len());
        for (at, record) in read.into_iter().enumerate() {
            let record = record?;
            versions.push(AliasVersion {
                version: at + 1,
                template_id: record.template_id,
                set_at: record.set_at,
            });
        }
        let current = versions.last().cloned().ok_or(StoreError::NotFound)?;
        Ok(AliasHistory {
            name: name.to_string(),
            template_id: current.template_id,
            version: current.version,
            set_at: current.set_at,
            versions,
        })
    }

    /// How many versions an alias holds now, for its limit.
    pub async fn alias_versions(&self, scope: &str, name: &str) -> StoreResult<usize> {
        Ok(self.alias_records(scope, name).await?.live().len())
    }

    /// Points an alias at a template, as a new version. Nothing already
    /// written changes: the newest record is the alias.
    pub async fn set_alias(
        &self,
        scope: &str,
        name: &str,
        template_id: &str,
    ) -> StoreResult<AliasMeta> {
        if !is_alias_name(name) {
            return Err(StoreError::NotFound);
        }
        self.template_meta(scope, template_id).await?;
        let record = AliasRecord {
            template_id: template_id.to_string(),
            set_at: now(),
        };
        let encoded = serde_json::to_vec(&record)
            .map_err(|e| StoreError::Backend(format!("metadata: {e}")))?;
        let id = uuid::Uuid::now_v7().hyphenated().to_string();
        self.objects
            .put(
                &alias_path(scope, name, &id, true),
                Bytes::from(encoded).into(),
            )
            .await?;
        let version = self
            .alias_records(scope, name)
            .await?
            .live()
            .iter()
            .position(|(r, _)| *r == id)
            .map_or(1, |at| at + 1);
        Ok(AliasMeta {
            name: name.to_string(),
            template_id: record.template_id,
            version,
            set_at: record.set_at,
        })
    }

    /// Deletes an alias. A marker written in one step ends it for every
    /// replica at once; the records before it are then removed, oldest first,
    /// so that no reader sees an older version come back.
    pub async fn delete_alias(&self, scope: &str, name: &str) -> StoreResult<()> {
        if !is_alias_name(name) {
            return Err(StoreError::NotFound);
        }
        let records = self.alias_records(scope, name).await?;
        if records.live().is_empty() {
            return Err(StoreError::NotFound);
        }
        let marker = uuid::Uuid::now_v7().hyphenated().to_string();
        self.objects
            .put(
                &alias_path(scope, name, &marker, false),
                Bytes::new().into(),
            )
            .await?;
        for (id, set) in &records.records {
            match self
                .objects
                .delete(&alias_path(scope, name, id, *set))
                .await
            {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
                // The marker already ended the alias; what is left is only space.
                Err(e) => {
                    tracing::warn!(alias = %name, error = %e, "an old alias record was not removed");
                }
            }
        }
        Ok(())
    }

    /// Every live alias of a scope, by name, each with its records.
    async fn live_aliases(&self, scope: &str) -> StoreResult<Vec<(String, AliasRecords)>> {
        let prefix = ObjectPath::from(format!("aliases/{scope}"));
        let listed: Vec<_> = self.objects.list(Some(&prefix)).try_collect().await?;
        let mut by_name: std::collections::BTreeMap<String, AliasRecords> = Default::default();
        for object in &listed {
            let parts: Vec<String> = object
                .location
                .parts()
                .map(|p| p.as_ref().to_string())
                .collect();
            let [_, _, name, file] = parts.as_slice() else {
                continue;
            };
            let Some((id, set)) = alias_file(file) else {
                continue;
            };
            if !is_alias_name(name) {
                continue;
            }
            by_name
                .entry(name.clone())
                .or_default()
                .records
                .push((id.to_string(), set));
        }
        Ok(by_name
            .into_iter()
            .filter_map(|(name, mut records)| {
                records.records.sort_unstable();
                (!records.live().is_empty()).then_some((name, records))
            })
            .collect())
    }

    /// How many aliases a scope holds, for its limit.
    pub async fn alias_count(&self, scope: &str) -> StoreResult<usize> {
        Ok(self.live_aliases(scope).await?.len())
    }

    /// A page of a scope's aliases, by name.
    pub async fn list_aliases(
        &self,
        scope: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> StoreResult<AliasPage> {
        let all = self.live_aliases(scope).await?;
        let start = cursor.map_or(0, |after| all.partition_point(|(n, _)| n.as_str() <= after));
        let page = &all[start.min(all.len())..];
        let shown = &page[..limit.min(page.len())];
        let wanted: Vec<(String, String, usize)> = shown
            .iter()
            .filter_map(|(name, records)| {
                let live = records.live();
                let (id, _) = live.last()?;
                Some((name.clone(), id.clone(), live.len()))
            })
            .collect();
        let read: Vec<StoreResult<AliasMeta>> = stream::iter(wanted)
            .map(|(name, id, version): (String, String, usize)| async move {
                let record = self.alias_record(scope, &name, &id).await?;
                Ok(AliasMeta {
                    name,
                    template_id: record.template_id,
                    version,
                    set_at: record.set_at,
                })
            })
            .buffered(CONCURRENT_READS)
            .collect()
            .await;
        let mut aliases = Vec::with_capacity(read.len());
        for meta in read {
            match meta {
                Ok(meta) => aliases.push(meta),
                // Deleted since the listing.
                Err(StoreError::NotFound) => {}
                Err(other) => return Err(other),
            }
        }
        let next_cursor = (page.len() > shown.len())
            .then(|| shown.last().map(|(n, _)| n.clone()))
            .flatten();
        Ok(AliasPage {
            aliases,
            next_cursor,
        })
    }

    /// Records a job's state. Each state is an object of its own, written once,
    /// and the latest holds the whole record.
    pub async fn put_job(&self, scope: &str, record: &JobRecord) -> StoreResult<()> {
        if !is_job_id(&record.id) {
            return Err(StoreError::NotFound);
        }
        let encoded = serde_json::to_vec(record)
            .map_err(|e| StoreError::Backend(format!("metadata: {e}")))?;
        let path = ObjectPath::from(format!(
            "jobs/{scope}/{}/{}",
            record.id,
            record.status.object()
        ));
        self.objects.put(&path, Bytes::from(encoded).into()).await?;
        Ok(())
    }

    /// Stores what a job made. Written before the state that names it.
    pub async fn put_job_result(&self, scope: &str, id: &str, bytes: Bytes) -> StoreResult<()> {
        if !is_job_id(id) {
            return Err(StoreError::NotFound);
        }
        self.objects
            .put(&job_result_path(scope, id), bytes.into())
            .await?;
        Ok(())
    }

    /// A job as its latest state records it.
    pub async fn job(&self, scope: &str, id: &str) -> StoreResult<JobRecord> {
        if !is_job_id(id) {
            return Err(StoreError::NotFound);
        }
        let listed: Vec<_> = self
            .objects
            .list(Some(&job_prefix(scope, id)))
            .try_collect()
            .await?;
        let latest = listed
            .iter()
            .filter_map(|o| {
                let file = o.location.filename()?;
                (file.ends_with(".json") && file.as_bytes()[0].is_ascii_digit())
                    .then(|| (file.to_string(), &o.location))
            })
            .max_by(|a, b| a.0.cmp(&b.0))
            .ok_or(StoreError::NotFound)?;
        self.read_json(latest.1).await
    }

    /// What a finished job made, checked against the hash its record holds.
    pub async fn job_result(&self, scope: &str, record: &JobRecord) -> StoreResult<Bytes> {
        let expected = record.result.as_ref().ok_or(StoreError::NotFound)?;
        let bytes = self.read(&job_result_path(scope, &record.id)).await?;
        if digest(&bytes) != expected.sha256 {
            return Err(StoreError::Integrity(format!(
                "job {} result does not match its hash",
                record.id
            )));
        }
        Ok(bytes)
    }

    /// Removes the jobs of every scope that expired: ended more than `ttl` ago,
    /// or lost and past their time by as much. Every replica runs this, and a
    /// delete that finds nothing is not an error. Returns how many went.
    pub async fn expire_jobs(&self, ttl: Duration) -> StoreResult<usize> {
        let now = chrono::Utc::now();
        let prefix = ObjectPath::from("jobs");
        let listed: Vec<_> = self.objects.list(Some(&prefix)).try_collect().await?;
        let mut jobs: std::collections::BTreeMap<(String, String), Vec<object_store::ObjectMeta>> =
            Default::default();
        for object in listed {
            let parts: Vec<String> = object
                .location
                .parts()
                .map(|p| p.as_ref().to_string())
                .collect();
            if let [_, scope, id, _] = parts.as_slice() {
                jobs.entry((scope.clone(), id.clone()))
                    .or_default()
                    .push(object);
            }
        }
        let mut removed = 0;
        for ((scope, id), mut objects) in jobs {
            let ended = objects
                .iter()
                .find(|o| o.location.filename().is_some_and(|f| f.starts_with("3-")));
            let expired = match ended {
                // Written as the job ended, so its time needs no read; most
                // jobs a sweep sees have ended.
                Some(state) => state.last_modified + chrono_duration(ttl) < now,
                None => match self.job(&scope, &id).await {
                    Ok(record) => record.as_of(now, ttl).is_none(),
                    // Half written or half removed: old enough is enough.
                    Err(_) => objects
                        .iter()
                        .all(|o| o.last_modified + chrono_duration(ttl) < now),
                },
            };
            if !expired {
                continue;
            }
            // Earlier states first and the final one last, so a reader in
            // between still sees how the job ended.
            objects.sort_by_key(|o| match o.location.filename() {
                Some(f) if f.starts_with("3-") => 3,
                Some("result") => 2,
                _ => 1,
            });
            for object in objects {
                match self.objects.delete(&object.location).await {
                    Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            removed += 1;
        }
        Ok(removed)
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

/// Like `create_private_dir`, but refuses a directory that is already there and
/// is not this user's to control: a link, another user's, or writable by others. The
/// default sits in the shared temp directory, where anyone could make it first
/// and so choose the fonts LibreOffice loads.
fn create_owned_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    create_private_dir(path)?;
    let meta = std::fs::symlink_metadata(path)?;
    let refuse = |why: &str| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{why}; remove it, or set RUSTICDOC_FONT_CACHE_DIR to a directory of your own"),
        ))
    };
    if !meta.file_type().is_dir() {
        return refuse("is a link or not a directory");
    }
    // SAFETY: geteuid takes nothing and cannot fail.
    if meta.uid() != unsafe { libc::geteuid() } {
        return refuse("belongs to another user");
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return refuse("can be written by other users");
    }
    Ok(())
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
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let temporary = dir.join(format!(".{id}.tmp"));
    // A new file only, never through a link: whatever is in the way fails the write.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    file.write_all(bytes)?;
    drop(file);
    std::fs::rename(
        &temporary,
        dir.join(format!("{id}.{}", font_extension(bytes))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn font_cache_must_be_private_and_ours() {
        let root = tempfile::tempdir().unwrap();

        let fresh = root.path().join("fresh");
        create_owned_private_dir(&fresh).unwrap();
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        create_owned_private_dir(&fresh).unwrap();

        let readable = root.path().join("readable");
        std::fs::create_dir(&readable).unwrap();
        std::fs::set_permissions(&readable, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_owned_private_dir(&readable).unwrap();

        let open = root.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(create_owned_private_dir(&open).is_err());

        let link = root.path().join("link");
        std::os::unix::fs::symlink(&fresh, &link).unwrap();
        assert!(create_owned_private_dir(&link).is_err());
    }

    #[test]
    fn a_job_reads_as_it_stands_and_expires_after_it_ends() {
        let ttl = Duration::from_secs(60);
        let now = chrono::Utc::now();
        let mut job = JobRecord::queued(new_job_id(), "-".into(), Duration::from_secs(30));
        assert_eq!(
            job.clone().as_of(now, ttl).unwrap().status,
            JobStatus::Queued
        );
        // Past its time without ending: lost, and gone a TTL later.
        let later = now + chrono::Duration::seconds(31);
        let lost = job.clone().as_of(later, ttl).unwrap();
        assert_eq!(lost.status, JobStatus::Failed);
        assert_eq!(lost.error.unwrap()["error"], "job_lost");
        assert!(
            job.clone()
                .as_of(later + chrono::Duration::seconds(61), ttl)
                .is_none()
        );

        job.ended(JobStatus::Done, ttl);
        let ended = parse_time(job.finished_at.as_deref().unwrap()).unwrap();
        assert!(
            job.clone()
                .as_of(ended + chrono::Duration::seconds(59), ttl)
                .is_some()
        );
        assert!(
            job.as_of(ended + chrono::Duration::seconds(61), ttl)
                .is_none()
        );
    }

    #[test]
    fn an_alias_is_its_records_since_the_last_deletion() {
        let id = |n: u8| format!("01966f3e-7c1a-7d3c-9a44-1d2f5a6b7c{n:02x}");
        let records = AliasRecords {
            records: vec![(id(1), true), (id(2), true), (id(3), false), (id(4), true)],
        };
        assert_eq!(records.live(), &[(id(4), true)]);
        let records = AliasRecords {
            records: vec![(id(1), true), (id(2), true)],
        };
        assert_eq!(records.live().len(), 2);
        let records = AliasRecords {
            records: vec![(id(1), true), (id(2), false)],
        };
        assert!(records.live().is_empty());
        assert_eq!(
            alias_file(&format!("{}.json", id(1))),
            Some((id(1).as_str(), true))
        );
        assert_eq!(
            alias_file(&format!("{}.gone", id(1))),
            Some((id(1).as_str(), false))
        );
        assert_eq!(alias_file("../x.json"), None);
        assert!(is_alias_name("invoice-2026.v2"));
        for bad in ["", ".x", "-x", "a/b", "a b", &id(1), &"x".repeat(65)] {
            assert!(!is_alias_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_planted_link_is_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, b"keep").unwrap();
        let id = "a".repeat(64);
        std::os::unix::fs::symlink(&target, dir.path().join(format!(".{id}.tmp"))).unwrap();
        assert!(write_font(dir.path(), &id, b"font").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }
}
