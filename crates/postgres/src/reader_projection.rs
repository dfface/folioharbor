use async_trait::async_trait;
use folioharbor_application::ports::{
    ReaderCatalogError, ReaderCatalogRepository, ReaderPublication, ReaderResource,
    ReaderSpineEntry, ReaderTocEntry,
};
use folioharbor_domain::{
    id::{BlobId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId, UserId},
    imports::blob::StorageKey,
};
use uuid::Uuid;

use crate::{DatabaseContext, PgCatalogRepository, PgTransactionContext};

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
impl ReaderCatalogRepository for PgCatalogRepository {
    async fn find_readable_publication(
        &self,
        actor: UserId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<Option<ReaderPublication>, ReaderCatalogError> {
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
        row.map(reader_publication).transpose()
    }
}

fn reader_publication(row: ReaderProjectionRow) -> Result<ReaderPublication, ReaderCatalogError> {
    Ok(ReaderPublication {
        library_id: LibraryId::from_uuid(row.library_id),
        item_id: ItemId::from_uuid(row.item_id),
        manifestation_id: ManifestationId::from_uuid(row.manifestation_id),
        package_id: PublicationPackageId::from_uuid(row.package_id),
        blob_id: BlobId::from_uuid(row.blob_id),
        storage_key: StorageKey::from_opaque(row.storage_key),
        parser_profile_version: row.parser_profile_version,
        primary_title: row.primary_title,
        authors: row.authors,
        languages: row.languages,
        resources: parse_resources(parse_json(&row.resources)?)?,
        reading_order: parse_spine(parse_json(&row.reading_order)?)?,
        toc: parse_toc(parse_json(&row.toc)?)?,
    })
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
