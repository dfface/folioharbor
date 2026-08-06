#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use folioharbor_application::{
    authorization::{Action, AuthorizationFact, AuthorizationGrant, ResourceRef},
    catalog::{
        DeleteItem, DeleteItemCommand, RestoreItem, RestoreItemCommand,
        garbage_collect::{CollectGarbage, GarbageCollectionError},
    },
    ports::{
        AuthorizationRepository, AuthorizationRepositoryError, BlobPurgeClaim, BlobStore,
        BlobStoreError, GarbageCollectionRepository, GarbageCollectionRepositoryError,
        ItemLifecycleMutation, ItemLifecycleRepository, ItemLifecycleRepositoryError, PromotedBlob,
    },
};
use folioharbor_domain::{
    catalog::ItemLifecycle,
    id::{BlobId, ItemId, LibraryId, RequestId, UserId},
    imports::blob::{BlobIdentity, StorageKey},
    libraries::role::RoleCode,
    time::OffsetDateTime,
};
use time::Duration;

#[test]
fn recovery_window_and_blob_delay_have_exact_boundaries() {
    let deleted_at = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixture time");
    let deleted = ItemLifecycle::Active.delete(deleted_at);

    assert!(
        !deleted.is_accessible(),
        "delete must revoke access immediately"
    );
    assert_eq!(
        deleted
            .clone()
            .restore(deleted_at + Duration::days(7) - Duration::nanoseconds(1)),
        Some(ItemLifecycle::Active),
        "restore remains available immediately before seven days"
    );
    assert_eq!(
        deleted.clone().restore(deleted_at + Duration::days(7)),
        None,
        "purge eligibility starts exactly seven days after deletion"
    );

    let eligible = deleted.advance(deleted_at + Duration::days(7));
    assert!(matches!(eligible, ItemLifecycle::PurgeEligible { .. }));
    let purged = eligible
        .purge(deleted_at + Duration::days(7))
        .expect("eligible item purges");
    assert_eq!(
        purged.blob_purge_after(),
        Some(deleted_at + Duration::days(8)),
        "physical Blob deletion waits a further 24 hours"
    );
}

struct AllowHoldingEdit {
    library_id: LibraryId,
    seen: Mutex<Vec<(Action, ResourceRef)>>,
}

#[async_trait]
impl AuthorizationRepository for AllowHoldingEdit {
    async fn resolve(
        &self,
        _: UserId,
        action: Action,
        resource: ResourceRef,
    ) -> Result<Option<AuthorizationFact>, AuthorizationRepositoryError> {
        self.seen
            .lock()
            .expect("authorization observations")
            .push((action, resource));
        Ok(Some(AuthorizationFact {
            library_id: self.library_id,
            role: RoleCode::Editor,
            membership_version: 3,
            discoverable: true,
            permitted: action.required_permission().as_str() == "holding.edit",
        }))
    }
}

struct LifecycleMemory {
    state: Mutex<ItemLifecycle>,
    mutations: Mutex<Vec<ItemLifecycleMutation>>,
}

#[async_trait]
impl ItemLifecycleRepository for LifecycleMemory {
    async fn delete(
        &self,
        mutation: ItemLifecycleMutation,
    ) -> Result<ItemLifecycle, ItemLifecycleRepositoryError> {
        self.mutations
            .lock()
            .expect("lifecycle observations")
            .push(mutation.clone());
        let mut state = self.state.lock().expect("lifecycle state");
        *state = state.clone().delete(mutation.now);
        Ok(state.clone())
    }

    async fn restore(
        &self,
        mutation: ItemLifecycleMutation,
    ) -> Result<ItemLifecycle, ItemLifecycleRepositoryError> {
        self.mutations
            .lock()
            .expect("lifecycle observations")
            .push(mutation.clone());
        let mut state = self.state.lock().expect("lifecycle state");
        let restored = state
            .clone()
            .restore(mutation.now)
            .ok_or(ItemLifecycleRepositoryError::RecoveryWindowElapsed)?;
        *state = restored;
        Ok(state.clone())
    }
}

#[tokio::test]
async fn delete_and_restore_require_holding_edit_and_carry_allowed_audit() {
    let actor = UserId::new();
    let library = LibraryId::new();
    let item = ItemId::new();
    let request = RequestId::new();
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixture time");
    let authorization = AllowHoldingEdit {
        library_id: library,
        seen: Mutex::new(Vec::new()),
    };
    let repository = LifecycleMemory {
        state: Mutex::new(ItemLifecycle::Active),
        mutations: Mutex::new(Vec::new()),
    };

    let deleted = DeleteItem::new(&repository, &authorization)
        .execute(DeleteItemCommand {
            actor,
            library_id: library,
            item_id: item,
            request_id: request,
            now,
        })
        .await
        .expect("editor deletes item");
    assert!(!deleted.is_accessible());

    let restored = RestoreItem::new(&repository, &authorization)
        .execute(RestoreItemCommand {
            actor,
            library_id: library,
            item_id: item,
            request_id: request,
            now: now + Duration::days(1),
        })
        .await
        .expect("editor restores item in window");
    assert_eq!(restored, ItemLifecycle::Active);

    let seen = authorization
        .seen
        .lock()
        .expect("authorization observations");
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|(_, resource)| {
        *resource
            == ResourceRef::Item {
                library_id: library,
                item_id: item,
            }
    }));
    assert!(
        seen.iter()
            .all(|(action, _)| action.required_permission().as_str() == "holding.edit")
    );
    let mutations = repository.mutations.lock().expect("lifecycle observations");
    assert_eq!(mutations.len(), 2);
    assert!(mutations.iter().all(|mutation| {
        mutation.grant.actor() == actor
            && mutation.grant.membership_version() == 3
            && mutation.audit.actor == Some(actor)
            && mutation.audit.request_id == request
            && mutation.audit.resource
                == ResourceRef::Item {
                    library_id: library,
                    item_id: item,
                }
    }));
}

struct GarbageMemory {
    claim: BlobPurgeClaim,
    available: Mutex<bool>,
    completed: Mutex<u32>,
    released: Mutex<u32>,
}

#[async_trait]
impl GarbageCollectionRepository for GarbageMemory {
    async fn prepare(
        &self,
        _: OffsetDateTime,
        _: u32,
    ) -> Result<u64, GarbageCollectionRepositoryError> {
        Ok(0)
    }

    async fn claim(
        &self,
        _: &str,
        _: OffsetDateTime,
        _: u32,
    ) -> Result<Vec<BlobPurgeClaim>, GarbageCollectionRepositoryError> {
        let mut available = self.available.lock().expect("claim availability");
        if *available {
            *available = false;
            Ok(vec![self.claim.clone()])
        } else {
            Ok(Vec::new())
        }
    }

    async fn complete(
        &self,
        _: &BlobPurgeClaim,
        _: &str,
        _: OffsetDateTime,
    ) -> Result<bool, GarbageCollectionRepositoryError> {
        *self.completed.lock().expect("completion count") += 1;
        Ok(true)
    }

    async fn release(
        &self,
        _: &BlobPurgeClaim,
        _: &str,
        _: OffsetDateTime,
    ) -> Result<bool, GarbageCollectionRepositoryError> {
        *self.released.lock().expect("release count") += 1;
        *self.available.lock().expect("claim availability") = true;
        Ok(true)
    }
}

struct FailOnceBlobs(Mutex<bool>);

#[async_trait]
impl BlobStore for FailOnceBlobs {
    fn candidate_key(&self, _: &BlobIdentity) -> StorageKey {
        StorageKey::from_opaque("unused".to_owned())
    }
    async fn create_staging_for(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        Ok(())
    }
    async fn append(&self, _: &StorageKey, _: &[u8]) -> Result<(), BlobStoreError> {
        Ok(())
    }
    async fn read_range(&self, _: &StorageKey, _: u64, _: u64) -> Result<Vec<u8>, BlobStoreError> {
        Ok(Vec::new())
    }
    async fn promote(
        &self,
        _: &StorageKey,
        _: &BlobIdentity,
    ) -> Result<PromotedBlob, BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
    async fn delete(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        let mut fail = self.0.lock().expect("failure switch");
        if *fail {
            *fail = false;
            return Err(BlobStoreError::Io(std::io::Error::other("transient")));
        }
        Ok(())
    }
    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        Ok(u64::MAX)
    }
}

#[tokio::test]
async fn storage_failure_releases_the_durable_claim_for_retry() {
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixture time");
    let repository = Arc::new(GarbageMemory {
        claim: BlobPurgeClaim {
            blob_id: BlobId::new(),
            storage_key: StorageKey::from_opaque("blob:retryable".to_owned()),
        },
        available: Mutex::new(true),
        completed: Mutex::new(0),
        released: Mutex::new(0),
    });
    let collector = CollectGarbage::new(
        Arc::clone(&repository),
        Arc::new(FailOnceBlobs(Mutex::new(true))),
        "worker-a".to_owned(),
        10,
    )
    .expect("bounded collector");

    assert!(matches!(
        collector.execute(now).await,
        Err(GarbageCollectionError::Storage)
    ));
    assert_eq!(*repository.released.lock().expect("release count"), 1);
    assert_eq!(*repository.completed.lock().expect("completion count"), 0);

    let outcome = collector
        .execute(now + Duration::minutes(1))
        .await
        .expect("retry succeeds");
    assert_eq!(outcome.purged_blobs, 1);
    assert_eq!(*repository.completed.lock().expect("completion count"), 1);
}

// Keep the grant type in this contract: repositories receive a non-forgeable authorization fact.
const _: Option<AuthorizationGrant> = None;
