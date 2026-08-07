#![allow(clippy::expect_used)]

use std::{collections::BTreeMap, io::Cursor, sync::Mutex};

use async_trait::async_trait;
use folioharbor_application::{
    operations::{
        BlobInventoryEntry, ConsistencyCheck, ConsistencyRepository, ConsistencyRepositoryError,
        DatabaseHealth, HealthRepository, HealthRepositoryError, HealthService, HealthStatus,
        OperationsApi as _,
    },
    ports::{
        BlobDisposition, BlobStore, BlobStoreError, BlobStoreInventory, PromotedBlob,
        PublicationSource,
    },
};
use folioharbor_domain::imports::blob::{BlobIdentity, StorageKey};
use sha2::Digest as _;

struct FakeHealthRepository(Result<DatabaseHealth, HealthRepositoryError>);

#[async_trait]
impl HealthRepository for FakeHealthRepository {
    async fn database_health(&self) -> Result<DatabaseHealth, HealthRepositoryError> {
        self.0
    }
}

struct FakeBlobStore {
    free_bytes: Result<u64, ()>,
    write_probe: Result<(), ()>,
    inventory: BlobStoreInventory,
    files: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl FakeBlobStore {
    fn health(free_bytes: Result<u64, ()>) -> Self {
        Self {
            free_bytes,
            write_probe: Ok(()),
            inventory: BlobStoreInventory::default(),
            files: Mutex::new(BTreeMap::new()),
        }
    }
}

#[async_trait]
impl BlobStore for FakeBlobStore {
    fn candidate_key(&self, _: &BlobIdentity) -> StorageKey {
        StorageKey::from_opaque("unused".to_owned())
    }

    async fn create_staging_for(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }

    async fn append(&self, _: &StorageKey, _: &[u8]) -> Result<(), BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }

    async fn read_range(&self, _: &StorageKey, _: u64, _: u64) -> Result<Vec<u8>, BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }

    async fn promote(
        &self,
        _: &StorageKey,
        _: &BlobIdentity,
    ) -> Result<PromotedBlob, BlobStoreError> {
        Ok(PromotedBlob {
            key: StorageKey::from_opaque("unused".to_owned()),
            disposition: BlobDisposition::Reused,
        })
    }

    async fn delete(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }

    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        self.free_bytes
            .map_err(|()| BlobStoreError::Io(std::io::Error::other("test capacity failure")))
    }

    async fn probe_write(&self) -> Result<(), BlobStoreError> {
        self.write_probe
            .map_err(|()| BlobStoreError::Io(std::io::Error::other("test write failure")))
    }

    async fn inventory(&self) -> Result<BlobStoreInventory, BlobStoreError> {
        Ok(self.inventory.clone())
    }

    async fn open_publication(
        &self,
        key: &StorageKey,
    ) -> Result<Box<dyn PublicationSource>, BlobStoreError> {
        self.files
            .lock()
            .expect("files")
            .get(key.as_str())
            .cloned()
            .map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn PublicationSource>)
            .ok_or_else(|| {
                BlobStoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "test missing file",
                ))
            })
    }
}

#[tokio::test]
async fn readiness_requires_exact_schema_admin_configuration_and_storage_reserve() {
    let cases = [
        (28, true, true, Ok(1_000), Ok(()), HealthStatus::Ready),
        (27, true, true, Ok(1_000), Ok(()), HealthStatus::Unavailable),
        (
            28,
            false,
            true,
            Ok(1_000),
            Ok(()),
            HealthStatus::BootstrapRequired,
        ),
        (
            28,
            true,
            false,
            Ok(1_000),
            Ok(()),
            HealthStatus::Unavailable,
        ),
        (28, true, true, Ok(99), Ok(()), HealthStatus::Unavailable),
        (28, true, true, Err(()), Ok(()), HealthStatus::Unavailable),
        (
            28,
            true,
            true,
            Ok(1_000),
            Err(()),
            HealthStatus::Unavailable,
        ),
    ];
    for (schema_version, admin, configuration, free_bytes, write_probe, expected) in cases {
        let mut blobs = FakeBlobStore::health(free_bytes);
        blobs.write_probe = write_probe;
        let service = HealthService::new(
            std::sync::Arc::new(FakeHealthRepository(Ok(DatabaseHealth {
                schema_version,
                system_administrator_exists: admin,
            }))),
            std::sync::Arc::new(blobs),
            100,
            configuration,
        );
        assert_eq!(service.readiness().await, expected);
    }
}

struct FakeInventory(Vec<BlobInventoryEntry>);

#[async_trait]
impl ConsistencyRepository for FakeInventory {
    async fn blob_inventory(&self) -> Result<Vec<BlobInventoryEntry>, ConsistencyRepositoryError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn consistency_check_classifies_missing_orphan_and_hash_mismatch_without_identifiers() {
    let clean = b"clean".to_vec();
    let clean_digest: [u8; 32] = sha2::Sha256::digest(&clean).into();
    let store = FakeBlobStore {
        free_bytes: Ok(1_000),
        write_probe: Ok(()),
        inventory: BlobStoreInventory {
            keys: vec![
                StorageKey::from_opaque("clean".to_owned()),
                StorageKey::from_opaque("mismatch".to_owned()),
                StorageKey::from_opaque("filesystem-extra".to_owned()),
            ],
            invalid_locations: 1,
        },
        files: Mutex::new(BTreeMap::from([
            ("clean".to_owned(), clean),
            ("mismatch".to_owned(), b"wrong".to_vec()),
        ])),
    };
    let inventory = FakeInventory(vec![
        entry("clean", true, clean_digest, 5),
        entry("missing", true, [1; 32], 1),
        entry("orphan", false, [2; 32], 1),
        entry("mismatch", true, [3; 32], 5),
    ]);

    let report = ConsistencyCheck::new(&inventory, &store)
        .execute()
        .await
        .expect("check");
    assert_eq!(report.checked, 4);
    assert_eq!(report.missing_blobs, 1);
    assert_eq!(report.orphan_locations, 3);
    assert_eq!(report.hash_mismatches, 1);
    assert!(!report.is_clean());
}

fn entry(
    key: &str,
    location_is_canonical: bool,
    expected_sha256: [u8; 32],
    expected_byte_size: u64,
) -> BlobInventoryEntry {
    BlobInventoryEntry {
        storage_key: StorageKey::from_opaque(key.to_owned()),
        location_is_canonical,
        integrity_required: true,
        expected_sha256,
        expected_byte_size,
    }
}
