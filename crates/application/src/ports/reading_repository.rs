use async_trait::async_trait;
use folioharbor_domain::{
    id::{ContentUnitId, DeviceId, ManifestationId, PublicationPackageId, RequestId, UserId},
    reader::{ReadingProgress, ReadingUpdateOutcome, ReadiumLocator},
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateProgressRecord {
    pub actor: UserId,
    pub manifestation_id: ManifestationId,
    pub device_id: DeviceId,
    pub client_mutation_id: Uuid,
    pub base_version: u64,
    pub package_id: Option<PublicationPackageId>,
    pub content_unit_id: Option<ContentUnitId>,
    pub locator: ReadiumLocator,
    pub request_id: RequestId,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ReadingRepositoryError {
    #[error("reading target was not found")]
    NotFound,
    #[error("reading mutation does not match its original command")]
    MutationMismatch,
    #[error("reading repository failed")]
    Persistence,
}

#[async_trait]
pub trait ReadingRepository: Send + Sync {
    async fn get_progress(
        &self,
        actor: UserId,
        manifestation_id: ManifestationId,
        request_id: RequestId,
    ) -> Result<Option<ReadingProgress>, ReadingRepositoryError>;

    /// Applies mutation idempotency, device update and the global compare/update atomically.
    async fn update_progress(
        &self,
        command: UpdateProgressRecord,
    ) -> Result<ReadingUpdateOutcome, ReadingRepositoryError>;
}
