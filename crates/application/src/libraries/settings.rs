use crate::{
    audit::AuditEvent,
    authorization::{Action, Authorization, ResourceRef},
    error::{AppError, FieldViolation},
    ports::{AuthorizationRepository, Clock, LibraryRepository},
};
use folioharbor_domain::id::{LibraryId, RequestId, UserId};

pub struct UpdateLibrarySettingsCommand {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub name: String,
    pub request_id: RequestId,
}
pub struct UpdateLibrarySettings<'a, R, A, C> {
    repository: &'a R,
    authorization: &'a A,
    clock: &'a C,
}
impl<'a, R, A, C> UpdateLibrarySettings<'a, R, A, C> {
    #[must_use]
    pub const fn new(repository: &'a R, authorization: &'a A, clock: &'a C) -> Self {
        Self {
            repository,
            authorization,
            clock,
        }
    }
}
impl<R: LibraryRepository, A: AuthorizationRepository, C: Clock>
    UpdateLibrarySettings<'_, R, A, C>
{
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
        let resource = ResourceRef::Library(c.library_id);
        let grant = Authorization::new(self.authorization)
            .require(c.actor, Action::ManageLibrary, resource)
            .await?;
        let now = self.clock.now();
        let audit =
            AuditEvent::allowed(c.actor, Action::ManageLibrary, resource, c.request_id, now);
        let outcome = self
            .repository
            .update_library_settings(c.actor, c.library_id, &c.name, now, grant, audit)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })?;
        super::mutation_result(outcome)
    }
}
