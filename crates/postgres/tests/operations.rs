#![allow(clippy::expect_used)]

use folioharbor_application::operations::{
    BootstrapAdminOutcome, BootstrapAdminRepository as _, ConsistencyCheck, DatabaseHealth,
    HealthRepository as _, NewSystemAdministrator,
};
use folioharbor_application::ports::BlobStore as _;
use folioharbor_domain::{
    id::{LibraryId, UploadId, UserId},
    identity::NormalizedEmail,
    imports::blob::{
        BlobIdentity, ByteCount, DedupScope, Sha256Digest, StorageKey, StorageNamespace,
    },
    time::OffsetDateTime,
};
use folioharbor_postgres::{PgOperationsRepository, connect_api, connect_owner, run_migrations};
use folioharbor_storage_local::LocalBlobStore;
use folioharbor_test_support::postgres::TestPostgres;
use secrecy::SecretString;
use sha2::{Digest as _, Sha256};
use time::Duration;

#[tokio::test]
async fn bootstrap_is_transactional_idempotent_and_separate_from_library_roles() {
    let database = TestPostgres::provision().await.expect("test database");
    let owner_url = database.owner_url().expect("owner URL");
    let api_url = database.api_url().expect("api URL");
    let owner = connect_owner(&SecretString::from(owner_url))
        .await
        .expect("owner");
    run_migrations(&owner).await.expect("migrations");
    let repository = PgOperationsRepository::new(owner.clone());
    let now = OffsetDateTime::from_unix_timestamp(1_750_000_000).expect("time");

    let outcome = repository
        .bootstrap_admin(administrator(now, "admin@example.com"))
        .await
        .expect("bootstrap");
    assert_eq!(outcome, BootstrapAdminOutcome::Created);
    let rerun = repository
        .bootstrap_admin(administrator(now, "admin@example.com"))
        .await
        .expect("idempotent rerun");
    assert_eq!(rerun, BootstrapAdminOutcome::AlreadyAdministrator);

    let row: (String, i64, i64) = sqlx::query_as(
        "SELECT a.status, \
         (SELECT count(*) FROM folioharbor.system_administrators), \
         (SELECT count(*) FROM folioharbor.library_memberships m WHERE m.user_id=a.user_id) \
         FROM folioharbor.user_accounts a WHERE a.normalized_email='admin@example.com'",
    )
    .fetch_one(&owner)
    .await
    .expect("admin state");
    assert_eq!(row, ("verified".to_owned(), 1, 0));

    let api = connect_api(&SecretString::from(api_url))
        .await
        .expect("api");
    assert_eq!(
        PgOperationsRepository::new(api)
            .database_health()
            .await
            .expect("safe health"),
        DatabaseHealth {
            schema_version: 27,
            system_administrator_exists: true,
        }
    );

    owner.close().await;
    database.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn concurrent_same_email_bootstrap_waits_and_returns_the_idempotent_outcome() {
    let database = TestPostgres::provision().await.expect("test database");
    let owner_url = database.owner_url().expect("owner URL");
    let owner = connect_owner(&SecretString::from(owner_url.clone()))
        .await
        .expect("owner");
    let second_owner = connect_owner(&SecretString::from(owner_url))
        .await
        .expect("second owner connection");
    run_migrations(&owner).await.expect("migrations");
    let now = OffsetDateTime::from_unix_timestamp(1_750_000_000).expect("time");
    let first_admin = administrator(now, "race@example.com");
    let mut first = owner.begin().await.expect("first transaction");
    let first_outcome: String =
        sqlx::query_scalar("SELECT folioharbor.operations_bootstrap_admin($1,$2,$3,$4,$5)")
            .bind(first_admin.user_id.as_uuid())
            .bind(first_admin.normalized_email.as_str())
            .bind(first_admin.display_email)
            .bind(first_admin.password_hash)
            .bind(first_admin.created_at)
            .fetch_one(&mut *first)
            .await
            .expect("first bootstrap");
    assert_eq!(first_outcome, "created");

    let second_repository = PgOperationsRepository::new(second_owner.clone());
    let mut second = tokio::spawn(async move {
        second_repository
            .bootstrap_admin(administrator(now, "race@example.com"))
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut second)
            .await
            .is_err(),
        "the overlapping call must wait for the first transaction"
    );
    first.commit().await.expect("commit first bootstrap");
    assert_eq!(
        second
            .await
            .expect("second task")
            .expect("idempotent retry"),
        BootstrapAdminOutcome::AlreadyAdministrator
    );

    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.user_accounts WHERE normalized_email='race@example.com'), \
                (SELECT count(*) FROM folioharbor.system_administrators administrator \
                 JOIN folioharbor.user_accounts account USING(user_id) \
                 WHERE account.normalized_email='race@example.com')",
    )
    .fetch_one(&owner)
    .await
    .expect("bootstrap counts");
    assert_eq!(counts, (1, 1));

    owner.close().await;
    second_owner.close().await;
    database.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn consistency_inventory_respects_every_physical_owner_lifecycle_state() {
    let database = TestPostgres::provision().await.expect("test database");
    let owner = connect_owner(&SecretString::from(
        database.owner_url().expect("owner URL"),
    ))
    .await
    .expect("owner");
    run_migrations(&owner).await.expect("migrations");
    let directory = tempfile::tempdir().expect("storage root");
    let blobs = LocalBlobStore::new(directory.path());
    let now = OffsetDateTime::now_utc();

    for (index, state) in [
        "ready",
        "quarantined",
        "purge_pending",
        "deleting",
        "purged",
    ]
    .into_iter()
    .enumerate()
    {
        let payload = vec![u8::try_from(index + 1).expect("small fixture"); index + 5];
        let digest = Sha256Digest::from_bytes(Sha256::digest(&payload).into());
        let identity = BlobIdentity::new(
            StorageNamespace::for_scope(DedupScope::Instance, LibraryId::new(), UploadId::new()),
            digest,
            ByteCount::new(u64::try_from(payload.len()).expect("small fixture")),
        );
        let staging = StorageKey::from_opaque(format!("staging:{}", identity.sha256().to_hex()));
        blobs
            .create_staging_for(&staging)
            .await
            .expect("staging file");
        blobs.append(&staging, &payload).await.expect("append");
        let installed = blobs.promote(&staging, &identity).await.expect("promote");

        let pending_at = (state == "purge_pending" || state == "deleting" || state == "purged")
            .then_some(now - Duration::hours(24));
        let purge_after = pending_at.map(|_| now);
        let purged_at = (state == "purged").then_some(now);
        let lease_owner = (state == "deleting").then_some("consistency-test");
        let lease_token = (state == "deleting").then(uuid::Uuid::now_v7);
        let lease_expires_at = (state == "deleting").then_some(now + Duration::minutes(5));
        let blob_id = uuid::Uuid::now_v7();
        sqlx::query(
            "INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) \
             VALUES($1,$2,$3,$4,$5)",
        )
        .bind(blob_id)
        .bind(identity.namespace().as_str())
        .bind(identity.sha256().as_bytes().to_vec())
        .bind(i64::try_from(payload.len()).expect("small fixture"))
        .bind(now)
        .execute(&owner)
        .await
        .expect("blob row");
        sqlx::query(
            "INSERT INTO folioharbor.blob_locations( \
               blob_id,storage_key,state,created_at,updated_at,purge_pending_at,purge_after, \
               purged_at,purge_lease_owner,purge_lease_token,purge_lease_expires_at) \
             VALUES($1,$2,$3,$4,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(blob_id)
        .bind(installed.key.as_str())
        .bind(state)
        .bind(now)
        .bind(pending_at)
        .bind(purge_after)
        .bind(purged_at)
        .bind(lease_owner)
        .bind(lease_token)
        .bind(lease_expires_at)
        .execute(&owner)
        .await
        .expect("lifecycle location");

        if matches!(state, "quarantined" | "purge_pending" | "deleting") {
            std::fs::write(
                object_path(directory.path(), &installed.key),
                vec![b'!'; payload.len()],
            )
            .expect("non-ready bytes may already be corrupt or deleting");
        }
    }

    let report = ConsistencyCheck::new(&PgOperationsRepository::new(owner.clone()), &blobs)
        .execute()
        .await
        .expect("consistency check");
    assert_eq!(report.checked, 1, "only ready bytes require integrity I/O");
    assert_eq!(report.missing_blobs, 0);
    assert_eq!(report.hash_mismatches, 0);
    assert_eq!(
        report.orphan_locations, 1,
        "only the physical file retained after a purged row is orphaned"
    );

    owner.close().await;
    database.cleanup().await.expect("cleanup");
}

fn object_path(root: &std::path::Path, key: &StorageKey) -> std::path::PathBuf {
    let mut components = key.as_str().split(':');
    assert_eq!(components.next(), Some("blob"));
    let namespace = components.next().expect("namespace");
    let hash = components.next().expect("hash");
    let size = components.next().expect("size");
    assert!(components.next().is_none());
    root.join("objects")
        .join(namespace)
        .join(&hash[0..2])
        .join(&hash[2..4])
        .join(format!("{hash}-{size}"))
}

fn administrator(now: OffsetDateTime, email: &str) -> NewSystemAdministrator {
    NewSystemAdministrator {
        user_id: UserId::new(),
        normalized_email: NormalizedEmail::parse(email).expect("email"),
        display_email: email.to_owned(),
        password_hash: "$argon2id$v=19$m=19456,t=2,p=1$test$hash".to_owned(),
        created_at: now,
    }
}
