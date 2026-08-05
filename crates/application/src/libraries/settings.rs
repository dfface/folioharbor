use crate::{
    error::{AppError, FieldViolation},
    ports::{Clock, LibraryRepository},
};
use folioharbor_domain::id::{LibraryId, UserId};
pub struct UpdateLibrarySettingsCommand {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub name: String,
}
pub struct UpdateLibrarySettings<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}
impl<'a, R, C> UpdateLibrarySettings<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}
impl<R: LibraryRepository, C: Clock> UpdateLibrarySettings<'_, R, C> {
    /// Updates owner-managed library settings.
    ///
    /// # Errors
    /// Returns an application error when validation, authorization, or persistence fails.
    pub async fn execute(&self, c: UpdateLibrarySettingsCommand) -> Result<(), AppError> {
        if c.name.trim().is_empty() {
            return Err(AppError::Invalid {
                code: "invalid_library_settings",
                fields: vec![FieldViolation {
                    field: "name",
                    code: "required",
                }],
            });
        }
        let o = self
            .repository
            .update_library_settings(c.actor, c.library_id, &c.name, self.clock.now())
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })?;
        super::mutation_result(o)
    }
}
