use async_trait::async_trait;
use folioharbor_application::ports::{
    ReaderCatalogError, ReaderCatalogRepository, ReaderPublication, ReaderResource,
    ReaderSpineEntry, ReaderTocEntry,
};
use folioharbor_domain::{
    id::{BlobId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId, UserId},
    imports::blob::StorageKey,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use uuid::Uuid;

use crate::{DatabaseContext, PgTransactionContext};

const MAX_PROJECTION_CACHE_ENTRIES: usize = 16;
const MAX_PROJECTION_CACHE_BYTES: usize = 8 * 1024 * 1024;

type ProjectionResult = Result<Option<Arc<ReaderPublication>>, ReaderCatalogError>;
type ProjectionSender = tokio::sync::watch::Sender<Option<ProjectionResult>>;
type ProjectionReceiver = tokio::sync::watch::Receiver<Option<ProjectionResult>>;

#[derive(Clone)]
pub struct PgReaderCatalogRepository {
    pool: sqlx::PgPool,
    state: Arc<Mutex<ProjectionState>>,
    access_checks: Arc<AtomicUsize>,
    projection_loads: Arc<AtomicUsize>,
}

impl PgReaderCatalogRepository {
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            state: Arc::new(Mutex::new(ProjectionState {
                cache: ProjectionCache::new(
                    MAX_PROJECTION_CACHE_ENTRIES,
                    MAX_PROJECTION_CACHE_BYTES,
                ),
                inflight: ProjectionInflight::default(),
            })),
            access_checks: Arc::new(AtomicUsize::new(0)),
            projection_loads: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[must_use]
    pub fn cache_metrics(&self) -> ReaderProjectionCacheMetrics {
        let (entries, bytes, inflight_loads) = self.state.lock().map_or((0, 0, 0), |state| {
            (
                state.cache.entries.len(),
                state.cache.bytes,
                state.inflight.len(),
            )
        });
        ReaderProjectionCacheMetrics {
            entries,
            bytes,
            inflight_loads,
            access_checks: self.access_checks.load(Ordering::SeqCst),
            projection_loads: self.projection_loads.load(Ordering::SeqCst),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReaderProjectionCacheMetrics {
    pub entries: usize,
    pub bytes: usize,
    pub inflight_loads: usize,
    pub access_checks: usize,
    pub projection_loads: usize,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ProjectionKey {
    manifestation_id: Uuid,
    package_id: Uuid,
    blob_id: Uuid,
    parser_profile_version: String,
    format_version: u16,
}

struct ProjectionCache {
    entries: HashMap<ProjectionKey, Arc<ReaderPublication>>,
    order: VecDeque<ProjectionKey>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

struct ProjectionState {
    cache: ProjectionCache,
    inflight: ProjectionInflight,
}

#[derive(Default)]
struct ProjectionInflight {
    entries: HashMap<ProjectionKey, ProjectionSender>,
}

impl ProjectionInflight {
    fn subscribe(&self, key: &ProjectionKey) -> Option<ProjectionReceiver> {
        self.entries.get(key).map(ProjectionSender::subscribe)
    }

    fn insert(&mut self, key: ProjectionKey) -> (ProjectionSender, ProjectionReceiver) {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        self.entries.insert(key, sender.clone());
        (sender, receiver)
    }

    fn complete(
        &mut self,
        key: &ProjectionKey,
        sender: &ProjectionSender,
        result: ProjectionResult,
    ) {
        sender.send_replace(Some(result));
        if self
            .entries
            .get(key)
            .is_some_and(|current| current.same_channel(sender))
        {
            self.entries.remove(key);
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

impl ProjectionCache {
    fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_entries,
            max_bytes,
        }
    }

    fn get(&self, key: &ProjectionKey) -> Option<Arc<ReaderPublication>> {
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, key: ProjectionKey, publication: Arc<ReaderPublication>) {
        let weight = projection_weight(&publication);
        if self.entries.contains_key(&key) || self.max_entries == 0 || weight > self.max_bytes {
            return;
        }
        while self.entries.len() >= self.max_entries
            || self.bytes.saturating_add(weight) > self.max_bytes
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(projection_weight(&removed));
            }
        }
        self.order.push_back(key.clone());
        self.bytes = self.bytes.saturating_add(weight);
        self.entries.insert(key, publication);
    }
}

fn projection_weight(publication: &ReaderPublication) -> usize {
    publication.primary_title.len()
        + publication.authors.iter().map(String::len).sum::<usize>()
        + publication.languages.iter().map(String::len).sum::<usize>()
        + publication
            .resources
            .iter()
            .map(|resource| resource.normalized_href.len() + resource.media_type.len())
            .sum::<usize>()
        + publication
            .reading_order
            .iter()
            .map(|entry| entry.normalized_href.len() + 1)
            .sum::<usize>()
        + publication
            .toc
            .iter()
            .map(|entry| entry.label.len() + entry.normalized_href.len())
            .sum::<usize>()
}

#[derive(Clone)]
struct ReaderAccessRow {
    library_id: Uuid,
    item_id: Uuid,
    manifestation_id: Uuid,
    package_id: Uuid,
    blob_id: Uuid,
    storage_key: String,
    parser_profile_version: String,
    membership_version: i64,
}

struct ReaderProjectionRow {
    library_id: Uuid,
    item_id: Uuid,
    manifestation_id: Uuid,
    package_id: Uuid,
    blob_id: Uuid,
    storage_key: String,
    parser_profile_version: String,
    primary_title: String,
    authors: Vec<String>,
    languages: Vec<String>,
    resources: String,
    reading_order: String,
    toc: String,
}

#[async_trait]
impl ReaderCatalogRepository for PgReaderCatalogRepository {
    async fn find_readable_publication(
        &self,
        actor: UserId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<Option<Arc<ReaderPublication>>, ReaderCatalogError> {
        let mut transaction = self.pool.begin().await.map_err(|_| ReaderCatalogError)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::api(actor, LibraryId::from_uuid(Uuid::nil()), request_id),
        )
        .await
        .map_err(|_| ReaderCatalogError)?;
        let access = sqlx::query_as!(
            ReaderAccessRow,
            r#"SELECT library_id AS "library_id!",item_id AS "item_id!",manifestation_id AS "manifestation_id!",package_id AS "package_id!",blob_id AS "blob_id!",storage_key AS "storage_key!",parser_profile_version AS "parser_profile_version!",membership_version AS "membership_version!" FROM folioharbor.reader_item_read_access($1,$2)"#,
            actor.as_uuid(),
            item_id.as_uuid(),
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| ReaderCatalogError)?;
        self.access_checks.fetch_add(1, Ordering::SeqCst);
        let Some(access) = access else {
            transaction.commit().await.map_err(|_| ReaderCatalogError)?;
            return Ok(None);
        };
        transaction.commit().await.map_err(|_| ReaderCatalogError)?;
        if access.membership_version <= 0 {
            return Err(ReaderCatalogError);
        }
        let key = ProjectionKey {
            manifestation_id: access.manifestation_id,
            package_id: access.package_id,
            blob_id: access.blob_id,
            parser_profile_version: access.parser_profile_version.clone(),
            format_version: 1,
        };
        let lookup = {
            let mut state = self.state.lock().map_err(|_| ReaderCatalogError)?;
            if let Some(cached) = state.cache.get(&key) {
                ProjectionLookup::Cached(Some(cached))
            } else if let Some(receiver) = state.inflight.subscribe(&key) {
                ProjectionLookup::Wait(receiver)
            } else {
                let (sender, receiver) = state.inflight.insert(key.clone());
                ProjectionLookup::Load { sender, receiver }
            }
        };
        let projection = match lookup {
            ProjectionLookup::Cached(projection) => projection,
            ProjectionLookup::Wait(receiver) => wait_for_projection(receiver).await?,
            ProjectionLookup::Load { sender, receiver } => {
                self.spawn_projection_loader(
                    key,
                    access.clone(),
                    actor,
                    item_id,
                    request_id,
                    sender,
                );
                wait_for_projection(receiver).await?
            }
        };
        Ok(projection.map(|publication| publication_for_access(&publication, access)))
    }
}

enum ProjectionLookup {
    Cached(Option<Arc<ReaderPublication>>),
    Wait(ProjectionReceiver),
    Load {
        sender: ProjectionSender,
        receiver: ProjectionReceiver,
    },
}

impl PgReaderCatalogRepository {
    fn spawn_projection_loader(
        &self,
        key: ProjectionKey,
        access: ReaderAccessRow,
        actor: UserId,
        item_id: ItemId,
        request_id: RequestId,
        sender: ProjectionSender,
    ) {
        let loader = self.clone();
        let worker = tokio::spawn(async move {
            loader
                .load_projection(access, actor, item_id, request_id)
                .await
        });
        let state = self.state.clone();
        drop(tokio::spawn(async move {
            let result = worker.await.unwrap_or(Err(ReaderCatalogError));
            if let Ok(mut state) = state.lock() {
                if let Ok(Some(publication)) = &result {
                    state.cache.insert(key.clone(), publication.clone());
                }
                state.inflight.complete(&key, &sender, result);
            } else {
                sender.send_replace(Some(Err(ReaderCatalogError)));
            }
        }));
    }

    async fn load_projection(
        &self,
        access: ReaderAccessRow,
        actor: UserId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> ProjectionResult {
        self.projection_loads.fetch_add(1, Ordering::SeqCst);
        let mut transaction = self.pool.begin().await.map_err(|_| ReaderCatalogError)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::api(actor, LibraryId::from_uuid(Uuid::nil()), request_id),
        )
        .await
        .map_err(|_| ReaderCatalogError)?;
        let row = sqlx::query_as!(
            ReaderProjectionRow,
            r#"SELECT library_id AS "library_id!",item_id AS "item_id!",manifestation_id AS "manifestation_id!",package_id AS "package_id!",blob_id AS "blob_id!",storage_key AS "storage_key!",parser_profile_version AS "parser_profile_version!",primary_title AS "primary_title!",authors AS "authors!",languages AS "languages!",resources::text AS "resources!",reading_order::text AS "reading_order!",toc::text AS "toc!" FROM folioharbor.reader_publication_visible($1,$2)"#,
            actor.as_uuid(),
            item_id.as_uuid(),
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| ReaderCatalogError)?;
        transaction.commit().await.map_err(|_| ReaderCatalogError)?;
        let Some(row) = row else {
            return Ok(None);
        };
        if row.library_id != access.library_id
            || row.item_id != access.item_id
            || row.manifestation_id != access.manifestation_id
            || row.package_id != access.package_id
            || row.blob_id != access.blob_id
            || row.storage_key != access.storage_key
            || row.parser_profile_version != access.parser_profile_version
        {
            return Err(ReaderCatalogError);
        }
        Ok(Some(Arc::new(reader_publication(row)?)))
    }
}

fn publication_for_access(
    cached: &ReaderPublication,
    access: ReaderAccessRow,
) -> Arc<ReaderPublication> {
    let mut publication = cached.clone();
    publication.library_id = LibraryId::from_uuid(access.library_id);
    publication.item_id = ItemId::from_uuid(access.item_id);
    publication.blob_id = BlobId::from_uuid(access.blob_id);
    publication.storage_key = StorageKey::from_opaque(access.storage_key);
    Arc::new(publication)
}

async fn wait_for_projection(mut receiver: ProjectionReceiver) -> ProjectionResult {
    loop {
        if let Some(result) = receiver.borrow().clone() {
            return result;
        }
        if receiver.changed().await.is_err() {
            return Err(ReaderCatalogError);
        }
    }
}

fn reader_publication(row: ReaderProjectionRow) -> Result<ReaderPublication, ReaderCatalogError> {
    Ok(ReaderPublication::new(
        LibraryId::from_uuid(row.library_id),
        ItemId::from_uuid(row.item_id),
        ManifestationId::from_uuid(row.manifestation_id),
        PublicationPackageId::from_uuid(row.package_id),
        BlobId::from_uuid(row.blob_id),
        StorageKey::from_opaque(row.storage_key),
        row.parser_profile_version,
        row.primary_title,
        row.authors,
        row.languages,
        parse_resources(parse_json(&row.resources)?)?,
        parse_spine(parse_json(&row.reading_order)?)?,
        parse_toc(parse_json(&row.toc)?)?,
    ))
}

fn parse_json(value: &str) -> Result<serde_json::Value, ReaderCatalogError> {
    serde_json::from_str(value).map_err(|_| ReaderCatalogError)
}

fn parse_resources(value: serde_json::Value) -> Result<Vec<ReaderResource>, ReaderCatalogError> {
    json_objects(value)?
        .map(|object| {
            Ok(ReaderResource {
                normalized_href: json_string(&object, "normalized_href")?,
                media_type: json_string(&object, "media_type")?,
            })
        })
        .collect()
}

fn parse_spine(value: serde_json::Value) -> Result<Vec<ReaderSpineEntry>, ReaderCatalogError> {
    json_objects(value)?
        .map(|object| {
            Ok(ReaderSpineEntry {
                normalized_href: json_string(&object, "normalized_href")?,
                linear: object
                    .get("linear")
                    .and_then(serde_json::Value::as_bool)
                    .ok_or(ReaderCatalogError)?,
            })
        })
        .collect()
}

fn parse_toc(value: serde_json::Value) -> Result<Vec<ReaderTocEntry>, ReaderCatalogError> {
    json_objects(value)?
        .map(|object| {
            Ok(ReaderTocEntry {
                label: json_string(&object, "label")?,
                normalized_href: json_string(&object, "normalized_href")?,
            })
        })
        .collect()
}

fn json_objects(
    value: serde_json::Value,
) -> Result<impl Iterator<Item = serde_json::Map<String, serde_json::Value>>, ReaderCatalogError> {
    let serde_json::Value::Array(values) = value else {
        return Err(ReaderCatalogError);
    };
    Ok(values
        .into_iter()
        .map(|value| value.as_object().cloned().ok_or(ReaderCatalogError))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter())
}

fn json_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, ReaderCatalogError> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or(ReaderCatalogError)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: u128) -> ProjectionKey {
        ProjectionKey {
            manifestation_id: Uuid::from_u128(value),
            package_id: Uuid::from_u128(value + 1),
            blob_id: Uuid::from_u128(value + 2),
            parser_profile_version: "epub-v1".to_owned(),
            format_version: 1,
        }
    }

    fn publication(title: &str) -> Arc<ReaderPublication> {
        Arc::new(ReaderPublication::new(
            LibraryId::new(),
            ItemId::new(),
            ManifestationId::new(),
            PublicationPackageId::new(),
            BlobId::new(),
            StorageKey::from_opaque("blob:test".to_owned()),
            "epub-v1".to_owned(),
            title.to_owned(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ))
    }

    #[test]
    fn projection_cache_rejects_oversize_and_evicts_with_exact_byte_accounting() {
        let oversized = publication("too large");
        let mut tiny = ProjectionCache::new(1, 1);
        tiny.insert(key(1), oversized);
        assert_eq!(tiny.entries.len(), 0);
        assert_eq!(tiny.order.len(), 0);
        assert_eq!(tiny.bytes, 0);

        let first = publication("first");
        let second = publication("second-longer");
        let mut cache = ProjectionCache::new(1, 1024);
        cache.insert(key(10), first);
        cache.insert(key(20), second.clone());
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.order.len(), 1);
        assert!(cache.entries.contains_key(&key(20)));
        assert_eq!(cache.bytes, projection_weight(&second));
    }

    #[test]
    fn projection_inflight_cleans_failed_cancelled_waiter_and_allows_retry() {
        let key = key(30);
        let mut inflight = ProjectionInflight::default();
        let (failed_sender, failed_receiver) = inflight.insert(key.clone());
        drop(failed_receiver);
        inflight.complete(&key, &failed_sender, Err(ReaderCatalogError));
        assert_eq!(inflight.len(), 0);

        let (retry_sender, retry_receiver) = inflight.insert(key.clone());
        let expected = publication("retry");
        inflight.complete(&key, &retry_sender, Ok(Some(expected.clone())));
        assert_eq!(inflight.len(), 0);
        assert_eq!(retry_receiver.borrow().clone(), Some(Ok(Some(expected))));
    }
}
