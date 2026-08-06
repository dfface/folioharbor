use async_trait::async_trait;
use folioharbor_application::{
    audit::AuditDecision,
    authorization::{Action, ResourceRef},
    ports::{
        BlobPurgeClaim, GarbageCollectionRepository, GarbageCollectionRepositoryError,
        ItemLifecycleMutation, ItemLifecycleRepository, ItemLifecycleRepositoryError,
    },
};
use folioharbor_domain::{
    catalog::ItemLifecycle,
    id::{BlobId, RequestId},
    imports::blob::StorageKey,
    time::OffsetDateTime,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{DatabaseContext, PgCatalogRepository, PgTransactionContext};

#[derive(Clone, Debug)]
pub struct PgItemLifecycleRepository {
    pool: PgPool,
}

impl PgItemLifecycleRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn mutate(
        &self,
        mutation: ItemLifecycleMutation,
        operation: &'static str,
        expected_action: Action,
    ) -> Result<ItemLifecycle, ItemLifecycleRepositoryError> {
        let resource = ResourceRef::Item {
            library_id: mutation.grant.library_id(),
            item_id: mutation.item_id,
        };
        if mutation.grant.action() != expected_action
            || mutation.grant.resource() != resource
            || mutation.audit.actor != Some(mutation.grant.actor())
            || mutation.audit.effective_actor != Some(mutation.grant.actor())
            || mutation.audit.library_id != mutation.grant.library_id()
            || mutation.audit.action != expected_action
            || mutation.audit.resource != resource
            || mutation.audit.decision != AuditDecision::Allowed
            || mutation.audit.occurred_at != mutation.now
        {
            return Err(ItemLifecycleRepositoryError::Persistence);
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| ItemLifecycleRepositoryError::Persistence)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::api(
                mutation.grant.actor(),
                mutation.grant.library_id(),
                mutation.audit.request_id,
            ),
        )
        .await
        .map_err(|_| ItemLifecycleRepositoryError::Persistence)?;
        let row: (
            String,
            Option<String>,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
        ) = sqlx::query_as(
            "SELECT outcome,item_state,item_deleted_at,item_purge_eligible_at,item_purged_at FROM folioharbor.item_lifecycle_mutate_authorized($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(mutation.grant.actor().as_uuid())
        .bind(mutation.grant.library_id().as_uuid())
        .bind(mutation.item_id.as_uuid())
        .bind(operation)
        .bind(mutation.now)
        .bind(mutation.grant.membership_version())
        .bind(mutation.audit.request_id.as_ulid().to_string())
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| ItemLifecycleRepositoryError::Persistence)?;
        let state = match row.0.as_str() {
            "not_found" => return Err(ItemLifecycleRepositoryError::NotFound),
            "forbidden" => return Err(ItemLifecycleRepositoryError::Forbidden),
            "window_elapsed" => {
                return Err(ItemLifecycleRepositoryError::RecoveryWindowElapsed);
            }
            "applied" => lifecycle(&row)?,
            _ => return Err(ItemLifecycleRepositoryError::Persistence),
        };
        transaction
            .commit()
            .await
            .map_err(|_| ItemLifecycleRepositoryError::Persistence)?;
        Ok(state)
    }
}

fn lifecycle(
    row: &(
        String,
        Option<String>,
        Option<OffsetDateTime>,
        Option<OffsetDateTime>,
        Option<OffsetDateTime>,
    ),
) -> Result<ItemLifecycle, ItemLifecycleRepositoryError> {
    match row.1.as_deref() {
        Some("active") if row.2.is_none() && row.3.is_none() && row.4.is_none() => {
            Ok(ItemLifecycle::Active)
        }
        Some("deleted") => Ok(ItemLifecycle::Deleted {
            deleted_at: row.2.ok_or(ItemLifecycleRepositoryError::Persistence)?,
            purge_eligible_at: row.3.ok_or(ItemLifecycleRepositoryError::Persistence)?,
        }),
        Some("purge_eligible") => Ok(ItemLifecycle::PurgeEligible {
            deleted_at: row.2.ok_or(ItemLifecycleRepositoryError::Persistence)?,
            purge_eligible_at: row.3.ok_or(ItemLifecycleRepositoryError::Persistence)?,
        }),
        Some("purged") => Ok(ItemLifecycle::Purged {
            deleted_at: row.2.ok_or(ItemLifecycleRepositoryError::Persistence)?,
            purge_eligible_at: row.3.ok_or(ItemLifecycleRepositoryError::Persistence)?,
            purged_at: row.4.ok_or(ItemLifecycleRepositoryError::Persistence)?,
        }),
        Some(_) | None => Err(ItemLifecycleRepositoryError::Persistence),
    }
}

#[async_trait]
impl ItemLifecycleRepository for PgItemLifecycleRepository {
    async fn delete(
        &self,
        mutation: ItemLifecycleMutation,
    ) -> Result<ItemLifecycle, ItemLifecycleRepositoryError> {
        self.mutate(mutation, "delete", Action::DeleteItem).await
    }

    async fn restore(
        &self,
        mutation: ItemLifecycleMutation,
    ) -> Result<ItemLifecycle, ItemLifecycleRepositoryError> {
        self.mutate(mutation, "restore", Action::RestoreItem).await
    }
}

#[async_trait]
impl ItemLifecycleRepository for PgCatalogRepository {
    async fn delete(
        &self,
        mutation: ItemLifecycleMutation,
    ) -> Result<ItemLifecycle, ItemLifecycleRepositoryError> {
        PgItemLifecycleRepository::new(self.pool.clone())
            .delete(mutation)
            .await
    }

    async fn restore(
        &self,
        mutation: ItemLifecycleMutation,
    ) -> Result<ItemLifecycle, ItemLifecycleRepositoryError> {
        PgItemLifecycleRepository::new(self.pool.clone())
            .restore(mutation)
            .await
    }
}

#[derive(Clone, Debug)]
pub struct PgGarbageCollectionRepository {
    pool: PgPool,
}

impl PgGarbageCollectionRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn worker_transaction(
        &self,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, GarbageCollectionRepositoryError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| GarbageCollectionRepositoryError)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::worker(RequestId::new(), None),
        )
        .await
        .map_err(|_| GarbageCollectionRepositoryError)?;
        Ok(transaction)
    }
}

#[async_trait]
impl GarbageCollectionRepository for PgGarbageCollectionRepository {
    async fn prepare(
        &self,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<u64, GarbageCollectionRepositoryError> {
        let mut processed = 0_u64;
        for _ in 0..limit {
            let mut transaction = self.worker_transaction().await?;
            let advanced: i64 =
                sqlx::query_scalar("SELECT folioharbor.gc_prepare_items_worker($1,1)")
                    .bind(now)
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(|_| GarbageCollectionRepositoryError)?;
            transaction
                .commit()
                .await
                .map_err(|_| GarbageCollectionRepositoryError)?;
            if advanced == 0 {
                break;
            }
            processed = processed
                .checked_add(u64::try_from(advanced).map_err(|_| GarbageCollectionRepositoryError)?)
                .ok_or(GarbageCollectionRepositoryError)?;
        }
        Ok(processed)
    }

    async fn claim(
        &self,
        worker: &str,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<Vec<BlobPurgeClaim>, GarbageCollectionRepositoryError> {
        let mut transaction = self.worker_transaction().await?;
        let rows: Vec<(Uuid, String, Uuid)> = sqlx::query_as(
            "SELECT blob_id,storage_key,lease_token FROM folioharbor.gc_claim_blobs_worker($1,$2,$3)",
        )
        .bind(worker)
        .bind(now)
        .bind(i64::from(limit))
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| GarbageCollectionRepositoryError)?;
        transaction
            .commit()
            .await
            .map_err(|_| GarbageCollectionRepositoryError)?;
        Ok(rows
            .into_iter()
            .map(|(blob_id, storage_key, lease_token)| BlobPurgeClaim {
                blob_id: BlobId::from_uuid(blob_id),
                storage_key: StorageKey::from_opaque(storage_key),
                lease_token,
            })
            .collect())
    }

    async fn complete(
        &self,
        claim: &BlobPurgeClaim,
        worker: &str,
        now: OffsetDateTime,
    ) -> Result<bool, GarbageCollectionRepositoryError> {
        mutate_claim(
            &self.pool,
            "SELECT folioharbor.gc_complete_blob_worker($1,$2,$3,$4,$5)",
            claim,
            worker,
            now,
        )
        .await
    }

    async fn release(
        &self,
        claim: &BlobPurgeClaim,
        worker: &str,
        now: OffsetDateTime,
    ) -> Result<bool, GarbageCollectionRepositoryError> {
        mutate_claim(
            &self.pool,
            "SELECT folioharbor.gc_release_blob_worker($1,$2,$3,$4,$5)",
            claim,
            worker,
            now,
        )
        .await
    }
}

async fn mutate_claim(
    pool: &PgPool,
    sql: &str,
    claim: &BlobPurgeClaim,
    worker: &str,
    now: OffsetDateTime,
) -> Result<bool, GarbageCollectionRepositoryError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| GarbageCollectionRepositoryError)?;
    PgTransactionContext::apply(
        &mut transaction,
        &DatabaseContext::worker(RequestId::new(), None),
    )
    .await
    .map_err(|_| GarbageCollectionRepositoryError)?;
    let changed: bool = sqlx::query_scalar(sql)
        .bind(claim.blob_id.as_uuid())
        .bind(claim.storage_key.as_str())
        .bind(worker)
        .bind(claim.lease_token)
        .bind(now)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| GarbageCollectionRepositoryError)?;
    transaction
        .commit()
        .await
        .map_err(|_| GarbageCollectionRepositoryError)?;
    Ok(changed)
}
