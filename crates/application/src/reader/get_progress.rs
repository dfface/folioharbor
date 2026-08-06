use std::sync::Arc;

use folioharbor_domain::{
    id::{ManifestationId, RequestId, UserId},
    reader::ReadingProgress,
};

use crate::{
    error::AppError,
    ports::{ReadingRepository, ReadingRepositoryError},
};

pub struct GetReadingProgress {
    repository: Arc<dyn ReadingRepository>,
}
impl GetReadingProgress {
    #[must_use]
    pub fn new(repository: Arc<dyn ReadingRepository>) -> Self {
        Self { repository }
    }
    /// Reads progress after the repository's fresh content-access gate.
    ///
    /// # Errors
    ///
    /// Returns an application error for hidden targets or repository failure.
    pub async fn execute(
        &self,
        actor: UserId,
        manifestation_id: ManifestationId,
        request_id: RequestId,
    ) -> Result<Option<ReadingProgress>, AppError> {
        self.repository
            .get_progress(actor, manifestation_id, request_id)
            .await
            .map_err(map_error)
    }
}

pub(crate) fn map_error(error: ReadingRepositoryError) -> AppError {
    match error {
        ReadingRepositoryError::NotFound => AppError::NotFound {
            code: "manifestation_not_found",
        },
        ReadingRepositoryError::MutationMismatch => AppError::Conflict {
            code: "progress_mutation_mismatch",
        },
        ReadingRepositoryError::Persistence => AppError::DependencyUnavailable {
            code: "reading_repository_unavailable",
        },
    }
}
