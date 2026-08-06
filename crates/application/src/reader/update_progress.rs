use std::sync::Arc;

use folioharbor_domain::{
    id::{ContentUnitId, DeviceId, ManifestationId, PublicationPackageId, RequestId, UserId},
    reader::{ReadingUpdateOutcome, ReadiumLocator},
};
use uuid::Uuid;

use crate::{
    error::AppError,
    ports::{ReadingRepository, UpdateProgressRecord},
};

use super::get_progress::map_error;

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateReadingProgressCommand {
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
impl From<UpdateReadingProgressCommand> for UpdateProgressRecord {
    fn from(value: UpdateReadingProgressCommand) -> Self {
        Self {
            actor: value.actor,
            manifestation_id: value.manifestation_id,
            device_id: value.device_id,
            client_mutation_id: value.client_mutation_id,
            base_version: value.base_version,
            package_id: value.package_id,
            content_unit_id: value.content_unit_id,
            locator: value.locator,
            request_id: value.request_id,
        }
    }
}

pub struct UpdateReadingProgress {
    repository: Arc<dyn ReadingRepository>,
}
impl UpdateReadingProgress {
    #[must_use]
    pub fn new(repository: Arc<dyn ReadingRepository>) -> Self {
        Self { repository }
    }
    /// Applies one versioned progress mutation atomically through the repository.
    ///
    /// # Errors
    ///
    /// Returns an application error for hidden targets or repository failure.
    pub async fn execute(
        &self,
        command: UpdateReadingProgressCommand,
    ) -> Result<ReadingUpdateOutcome, AppError> {
        self.repository
            .update_progress(command.into())
            .await
            .map_err(map_error)
    }
}
