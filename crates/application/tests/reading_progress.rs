#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use folioharbor_application::{
    ports::{ReadingRepository, ReadingRepositoryError, UpdateProgressRecord},
    reader::{GetReadingProgress, UpdateReadingProgress, UpdateReadingProgressCommand},
};
use folioharbor_domain::{
    id::{DeviceId, ManifestationId, RequestId, UserId},
    reader::{
        DeviceReadingState, LocatorExtensions, LocatorLocations, ReadingProgress,
        ReadingUpdateOutcome, ReadiumLocator,
    },
    time::OffsetDateTime,
};
use uuid::Uuid;

#[derive(Default)]
struct Repository {
    progress: Mutex<Option<ReadingProgress>>,
    seen: Mutex<Vec<UpdateProgressRecord>>,
}

#[async_trait]
impl ReadingRepository for Repository {
    async fn get_progress(
        &self,
        _: UserId,
        _: ManifestationId,
        _: RequestId,
    ) -> Result<Option<ReadingProgress>, ReadingRepositoryError> {
        Ok(self.progress.lock().expect("progress").clone())
    }

    async fn update_progress(
        &self,
        command: UpdateProgressRecord,
    ) -> Result<ReadingUpdateOutcome, ReadingRepositoryError> {
        self.seen.lock().expect("seen").push(command.clone());
        let now = OffsetDateTime::now_utc();
        let global = ReadingProgress {
            manifestation_id: command.manifestation_id,
            package_id: command.package_id,
            content_unit_id: command.content_unit_id,
            locator: command.locator.clone(),
            version: command.base_version + 1,
            updated_at: now,
        };
        *self.progress.lock().expect("progress") = Some(global.clone());
        Ok(ReadingUpdateOutcome::Updated {
            global,
            device: DeviceReadingState {
                device_id: command.device_id,
                locator: command.locator,
                updated_at: now,
            },
        })
    }
}

fn locator(progression: f64) -> ReadiumLocator {
    ReadiumLocator::new(
        "OPS/chapter.xhtml".to_owned(),
        Some("application/xhtml+xml".to_owned()),
        LocatorLocations::new(Some(progression), None, None, Vec::new()).expect("locations"),
        None,
        LocatorExtensions::empty_v1(),
    )
    .expect("locator")
}

#[tokio::test]
async fn update_passes_versioned_device_mutation_to_one_atomic_repository_call() {
    let repository = Arc::new(Repository::default());
    let use_case = UpdateReadingProgress::new(repository.clone());
    let actor = UserId::new();
    let manifestation = ManifestationId::new();
    let command = UpdateReadingProgressCommand {
        actor,
        manifestation_id: manifestation,
        device_id: DeviceId::new(),
        client_mutation_id: Uuid::now_v7(),
        base_version: 0,
        package_id: None,
        content_unit_id: None,
        locator: locator(0.2),
        request_id: RequestId::new(),
    };

    let result = use_case.execute(command.clone()).await.expect("updated");

    assert!(matches!(result, ReadingUpdateOutcome::Updated { .. }));
    assert_eq!(
        repository.seen.lock().expect("seen").as_slice(),
        &[command.into()]
    );
    assert_eq!(
        GetReadingProgress::new(repository)
            .execute(actor, manifestation, RequestId::new())
            .await
            .expect("read")
            .expect("state")
            .version,
        1
    );
}
