use folioharbor_domain::{id::UserId, libraries::Library};

use crate::{
    error::AppError,
    ports::{Clock, LibraryRepository},
};

pub struct ProvisionPersonalLibraryCommand {
    pub user_id: UserId,
}

pub struct ProvisionPersonalLibrary<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}

impl<'a, R, C> ProvisionPersonalLibrary<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}

impl<R: LibraryRepository, C: Clock> ProvisionPersonalLibrary<'_, R, C> {
    /// Provisions or returns the user's personal library.
    ///
    /// # Errors
    /// Returns a dependency error when persistence is unavailable.
    pub async fn execute(
        &self,
        command: ProvisionPersonalLibraryCommand,
    ) -> Result<Library, AppError> {
        self.repository
            .provision_personal_library(command.user_id, self.clock.now())
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })
    }
}
