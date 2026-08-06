use async_trait::async_trait;
use folioharbor_application::{
    authorization::AuthorizationGrant,
    catalog::ImportCatalogResult,
    ports::{
        CatalogQueryRepository, CatalogRepository, CatalogRepositoryError, FinalizeCatalog,
        VisibleCatalogItem,
    },
};
use folioharbor_domain::id::{
    ContentUnitId, ExpressionId, HoldingId, ItemId, LibraryId, ManifestationId,
    PublicationPackageId, RequestId, UserId, WorkId,
};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::{DatabaseContext, PgTransactionContext};

#[derive(Clone, Debug)]
pub struct PgCatalogRepository {
    pool: PgPool,
}

struct CatalogProjectionRow {
    holding_id: Uuid,
    item_id: Uuid,
    package_id: Uuid,
    manifestation_id: Uuid,
    primary_title: String,
    authors: Vec<String>,
    languages: Vec<String>,
    identifiers: Vec<String>,
    media_type: String,
}

impl From<CatalogProjectionRow> for VisibleCatalogItem {
    fn from(row: CatalogProjectionRow) -> Self {
        Self {
            holding_id: HoldingId::from_uuid(row.holding_id),
            item_id: ItemId::from_uuid(row.item_id),
            manifestation_id: ManifestationId::from_uuid(row.manifestation_id),
            package_id: PublicationPackageId::from_uuid(row.package_id),
            primary_title: row.primary_title,
            authors: row.authors,
            languages: row.languages,
            identifiers: row.identifiers,
            media_type: row.media_type,
        }
    }
}

impl PgCatalogRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn finalize_in_transaction(
        connection: &mut PgConnection,
        command: &FinalizeCatalog,
    ) -> Result<ImportCatalogResult, CatalogRepositoryError> {
        lock_identity(connection, command).await?;
        validate_import_identity(connection, command).await?;
        if let Some(item_id) = find_duplicate(connection, command).await? {
            if finish_import(connection, command, item_id).await? != CatalogCompletion::Duplicate {
                return Err(CatalogRepositoryError::Persistence);
            }
            return Ok(ImportCatalogResult::Duplicate { item_id });
        }
        let package = find_package(connection, command).await?;
        let (package_id, manifestation_id) = match package {
            Some(found) => found,
            None => create_publication_aggregate(connection, command).await?,
        };
        let item_id =
            create_library_item(connection, command, package_id, manifestation_id).await?;
        if finish_import(connection, command, item_id).await? != CatalogCompletion::Created {
            return Err(CatalogRepositoryError::Persistence);
        }
        Ok(ImportCatalogResult::Created {
            item_id,
            package_id,
        })
    }
}

#[async_trait]
impl CatalogRepository for PgCatalogRepository {
    async fn finalize(
        &self,
        command: FinalizeCatalog,
    ) -> Result<ImportCatalogResult, CatalogRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(persistence)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::worker(command.request_id, Some(command.library_id)),
        )
        .await
        .map_err(persistence)?;
        let result = Self::finalize_in_transaction(&mut transaction, &command).await?;
        transaction.commit().await.map_err(persistence)?;
        Ok(result)
    }
}

#[async_trait]
impl CatalogQueryRepository for PgCatalogRepository {
    async fn list_visible_items(
        &self,
        grant: AuthorizationGrant,
        library_id: LibraryId,
        after: Option<HoldingId>,
        limit: u32,
        request_id: RequestId,
    ) -> Result<Vec<VisibleCatalogItem>, CatalogRepositoryError> {
        if grant.actor() == UserId::from_uuid(Uuid::nil())
            || grant.library_id() != library_id
            || grant.resource().library_id() != library_id
        {
            return Err(CatalogRepositoryError::Persistence);
        }
        let mut transaction = self.pool.begin().await.map_err(persistence)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::api(grant.actor(), library_id, request_id),
        )
        .await
        .map_err(persistence)?;
        let rows = if let Some(after) = after {
            sqlx::query_as!(
                CatalogProjectionRow,
                r#"SELECT visible.holding_id AS "holding_id!",visible.item_id AS "item_id!",visible.package_id AS "package_id!",visible.manifestation_id AS "manifestation_id!",visible.primary_title AS "primary_title!",visible.authors AS "authors!",visible.languages AS "languages!",visible.identifiers AS "identifiers!",visible.media_type AS "media_type!" FROM folioharbor.holdings holding CROSS JOIN LATERAL (SELECT candidate.item_id FROM folioharbor.items candidate WHERE candidate.holding_id=holding.holding_id AND candidate.state='active' ORDER BY candidate.created_at DESC,candidate.item_id DESC LIMIT 1) selected CROSS JOIN LATERAL folioharbor.catalog_item_projection_visible($1,$2,selected.item_id,$3) visible WHERE holding.library_id=$2 AND holding.state='active' AND holding.holding_id<$4 ORDER BY holding.holding_id DESC LIMIT $5"#,
                grant.actor().as_uuid(),
                library_id.as_uuid(),
                grant.membership_version(),
                after.as_uuid(),
                i64::from(limit),
            )
            .fetch_all(&mut *transaction)
            .await
            .map_err(persistence)?
        } else {
            sqlx::query_as!(
                CatalogProjectionRow,
                r#"SELECT visible.holding_id AS "holding_id!",visible.item_id AS "item_id!",visible.package_id AS "package_id!",visible.manifestation_id AS "manifestation_id!",visible.primary_title AS "primary_title!",visible.authors AS "authors!",visible.languages AS "languages!",visible.identifiers AS "identifiers!",visible.media_type AS "media_type!" FROM folioharbor.holdings holding CROSS JOIN LATERAL (SELECT candidate.item_id FROM folioharbor.items candidate WHERE candidate.holding_id=holding.holding_id AND candidate.state='active' ORDER BY candidate.created_at DESC,candidate.item_id DESC LIMIT 1) selected CROSS JOIN LATERAL folioharbor.catalog_item_projection_visible($1,$2,selected.item_id,$3) visible WHERE holding.library_id=$2 AND holding.state='active' ORDER BY holding.holding_id DESC LIMIT $4"#,
                grant.actor().as_uuid(),
                library_id.as_uuid(),
                grant.membership_version(),
                i64::from(limit),
            )
            .fetch_all(&mut *transaction)
            .await
            .map_err(persistence)?
        };
        transaction.commit().await.map_err(persistence)?;
        Ok(rows.into_iter().map(VisibleCatalogItem::from).collect())
    }

    async fn find_visible_item(
        &self,
        grant: AuthorizationGrant,
        library_id: LibraryId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<Option<VisibleCatalogItem>, CatalogRepositoryError> {
        if grant.library_id() != library_id || grant.resource().library_id() != library_id {
            return Err(CatalogRepositoryError::Persistence);
        }
        let mut transaction = self.pool.begin().await.map_err(persistence)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::api(grant.actor(), library_id, request_id),
        )
        .await
        .map_err(persistence)?;
        let row = sqlx::query_as!(
            CatalogProjectionRow,
            r#"SELECT visible.holding_id AS "holding_id!",visible.item_id AS "item_id!",visible.package_id AS "package_id!",visible.manifestation_id AS "manifestation_id!",visible.primary_title AS "primary_title!",visible.authors AS "authors!",visible.languages AS "languages!",visible.identifiers AS "identifiers!",visible.media_type AS "media_type!" FROM folioharbor.catalog_item_projection_visible($1,$2,$3,$4) visible"#,
            grant.actor().as_uuid(),
            library_id.as_uuid(),
            item_id.as_uuid(),
            grant.membership_version(),
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(persistence)?;
        transaction.commit().await.map_err(persistence)?;
        Ok(row.map(VisibleCatalogItem::from))
    }
}

async fn lock_identity(
    connection: &mut PgConnection,
    command: &FinalizeCatalog,
) -> Result<(), CatalogRepositoryError> {
    let blob = command.original_blob_id.as_uuid().to_string();
    let library_blob = format!("{}:{blob}", command.library_id.as_uuid());
    sqlx::query!("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", blob)
        .execute(&mut *connection)
        .await
        .map_err(persistence)?;
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended($1,1))",
        library_blob
    )
    .execute(&mut *connection)
    .await
    .map_err(persistence)?;
    Ok(())
}

async fn validate_import_identity(
    connection: &mut PgConnection,
    command: &FinalizeCatalog,
) -> Result<(), CatalogRepositoryError> {
    let logical = i64::try_from(command.logical_bytes.get())
        .map_err(|_| CatalogRepositoryError::Persistence)?;
    let valid: bool = sqlx::query_scalar!(
        "SELECT folioharbor.catalog_validate_import($1,$2,$3,$4,$5,$6) AS \"valid!\"",
        command.library_id.as_uuid(),
        command.upload_id.as_uuid(),
        command.actor_id.as_uuid(),
        command.original_blob_id.as_uuid(),
        logical,
        command.request_id.as_ulid().to_string()
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(persistence)?;
    if !valid {
        return Err(CatalogRepositoryError::ReservationNotActive);
    }
    Ok(())
}

async fn find_duplicate(
    connection: &mut PgConnection,
    command: &FinalizeCatalog,
) -> Result<Option<ItemId>, CatalogRepositoryError> {
    let id: Option<Uuid> = sqlx::query_scalar!(
        "SELECT item.item_id AS \"item_id!\" FROM folioharbor.holdings holding JOIN folioharbor.items item USING(holding_id) JOIN folioharbor.item_assets asset USING(item_id) WHERE holding.library_id=$1 AND holding.state='active' AND item.state='active' AND asset.asset_kind='original' AND asset.blob_id=$2 ORDER BY item.created_at LIMIT 1",
        command.library_id.as_uuid(),
        command.original_blob_id.as_uuid()
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(persistence)?;
    Ok(id.map(ItemId::from_uuid))
}

async fn find_package(
    connection: &mut PgConnection,
    command: &FinalizeCatalog,
) -> Result<Option<(PublicationPackageId, ManifestationId)>, CatalogRepositoryError> {
    let found = sqlx::query!(
        "SELECT package_id,manifestation_id FROM folioharbor.publication_packages WHERE blob_id=$1 AND parser_profile_version=$2",
        command.original_blob_id.as_uuid(),
        command.parser_profile_version
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(persistence)?;
    Ok(found.map(|row| {
        (
            PublicationPackageId::from_uuid(row.package_id),
            ManifestationId::from_uuid(row.manifestation_id),
        )
    }))
}

async fn create_publication_aggregate(
    connection: &mut PgConnection,
    command: &FinalizeCatalog,
) -> Result<(PublicationPackageId, ManifestationId), CatalogRepositoryError> {
    let work = WorkId::new();
    let expression = ExpressionId::new();
    let manifestation = ManifestationId::new();
    let package = PublicationPackageId::new();
    let metadata = command.publication.metadata();
    let authors: Vec<String> = metadata.authors().map(str::to_owned).collect();
    let languages: Vec<String> = metadata.languages().map(str::to_owned).collect();
    let identifiers: Vec<String> = metadata.identifiers().map(str::to_owned).collect();
    sqlx::query!("INSERT INTO folioharbor.works(work_id,primary_title,authors,created_at) VALUES($1,$2,$3,$4)", work.as_uuid(), metadata.primary_title(), &authors, command.now)
        .execute(&mut *connection).await.map_err(persistence)?;
    sqlx::query!("INSERT INTO folioharbor.expressions(expression_id,work_id,languages,created_at) VALUES($1,$2,$3,$4)", expression.as_uuid(), work.as_uuid(), &languages, command.now)
        .execute(&mut *connection).await.map_err(persistence)?;
    sqlx::query!("INSERT INTO folioharbor.manifestations(manifestation_id,identifiers,created_at) VALUES($1,$2,$3)", manifestation.as_uuid(), &identifiers, command.now)
        .execute(&mut *connection).await.map_err(persistence)?;
    sqlx::query!("INSERT INTO folioharbor.manifestation_expressions(manifestation_id,expression_id,expression_order) VALUES($1,$2,0)", manifestation.as_uuid(), expression.as_uuid())
        .execute(&mut *connection).await.map_err(persistence)?;
    sqlx::query!("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,$4,$5)", package.as_uuid(), manifestation.as_uuid(), command.original_blob_id.as_uuid(), command.parser_profile_version, command.now)
        .execute(&mut *connection).await.map_err(persistence)?;
    insert_package_structure(connection, command, package, manifestation).await?;
    Ok((package, manifestation))
}

async fn insert_package_structure(
    connection: &mut PgConnection,
    command: &FinalizeCatalog,
    package: PublicationPackageId,
    manifestation: ManifestationId,
) -> Result<(), CatalogRepositoryError> {
    for (order, resource) in command.publication.resources().iter().enumerate() {
        sqlx::query!("INSERT INTO folioharbor.publication_resources(package_id,resource_order,normalized_href,media_type,source_blob_id) VALUES($1,$2,$3,$4,$5)", package.as_uuid(), order_i32(order)?, resource.href(), resource.media_type(), command.original_blob_id.as_uuid())
            .execute(&mut *connection).await.map_err(persistence)?;
    }
    for (order, spine) in command.publication.spine().iter().enumerate() {
        let unit = ContentUnitId::new();
        sqlx::query!("INSERT INTO folioharbor.content_units(content_unit_id,package_id,locator_href,created_at) VALUES($1,$2,$3,$4)", unit.as_uuid(), package.as_uuid(), spine.href(), command.now)
            .execute(&mut *connection).await.map_err(persistence)?;
        sqlx::query!("INSERT INTO folioharbor.manifestation_units(manifestation_id,package_id,content_unit_id,spine_order,linear) VALUES($1,$2,$3,$4,$5)", manifestation.as_uuid(), package.as_uuid(), unit.as_uuid(), order_i32(order)?, spine.is_linear())
            .execute(&mut *connection).await.map_err(persistence)?;
    }
    for (order, entry) in command.publication.toc().iter().enumerate() {
        sqlx::query!("INSERT INTO folioharbor.package_toc_entries(package_id,toc_order,label,locator_href) VALUES($1,$2,$3,$4)", package.as_uuid(), order_i32(order)?, entry.label(), entry.href())
            .execute(&mut *connection).await.map_err(persistence)?;
    }
    sqlx::query!("INSERT INTO folioharbor.manifestation_assets(manifestation_id,blob_id,asset_kind,locator_href,created_at) VALUES($1,$2,'original',NULL,$3)", manifestation.as_uuid(), command.original_blob_id.as_uuid(), command.now)
        .execute(&mut *connection).await.map_err(persistence)?;
    if let Some(cover) = command.publication.cover_href() {
        sqlx::query!("INSERT INTO folioharbor.manifestation_assets(manifestation_id,blob_id,asset_kind,locator_href,created_at) VALUES($1,$2,'cover',$3,$4)", manifestation.as_uuid(), command.original_blob_id.as_uuid(), cover, command.now)
            .execute(&mut *connection).await.map_err(persistence)?;
    }
    Ok(())
}

async fn create_library_item(
    connection: &mut PgConnection,
    command: &FinalizeCatalog,
    package: PublicationPackageId,
    manifestation: ManifestationId,
) -> Result<ItemId, CatalogRepositoryError> {
    let holding = HoldingId::new();
    let item = ItemId::new();
    sqlx::query!("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)", holding.as_uuid(), command.library_id.as_uuid(), manifestation.as_uuid(), command.now)
        .execute(&mut *connection).await.map_err(persistence)?;
    sqlx::query!("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)", item.as_uuid(), holding.as_uuid(), manifestation.as_uuid(), package.as_uuid(), command.upload_id.as_uuid(), command.now)
        .execute(&mut *connection).await.map_err(persistence)?;
    sqlx::query!("INSERT INTO folioharbor.item_assets(item_id,blob_id,asset_kind,created_at) VALUES($1,$2,'original',$3)", item.as_uuid(), command.original_blob_id.as_uuid(), command.now)
        .execute(&mut *connection).await.map_err(persistence)?;
    Ok(item)
}

async fn finish_import(
    connection: &mut PgConnection,
    command: &FinalizeCatalog,
    item: ItemId,
) -> Result<CatalogCompletion, CatalogRepositoryError> {
    let outcome: String = sqlx::query_scalar!(
        "SELECT folioharbor.catalog_finish_import($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) AS \"outcome!\"",
        command.library_id.as_uuid(),
        command.upload_id.as_uuid(),
        command.actor_id.as_uuid(),
        command.original_blob_id.as_uuid(),
        i64::try_from(command.logical_bytes.get())
            .map_err(|_| CatalogRepositoryError::Persistence)?,
        command.parser_profile_version.as_str(),
        item.as_uuid(),
        Uuid::now_v7(),
        command.request_id.as_ulid().to_string(),
        command.now
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(persistence)?;
    match outcome.as_str() {
        "created" => Ok(CatalogCompletion::Created),
        "duplicate" => Ok(CatalogCompletion::Duplicate),
        _ => Err(CatalogRepositoryError::ReservationNotActive),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CatalogCompletion {
    Created,
    Duplicate,
}

fn order_i32(order: usize) -> Result<i32, CatalogRepositoryError> {
    i32::try_from(order).map_err(|_| CatalogRepositoryError::Persistence)
}

fn persistence(_: sqlx::Error) -> CatalogRepositoryError {
    CatalogRepositoryError::Persistence
}
