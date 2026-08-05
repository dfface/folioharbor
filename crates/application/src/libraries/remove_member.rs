use crate::{
    audit::AuditEvent,
    authorization::{Action, Authorization, ResourceRef},
    error::AppError,
    ports::{AuthorizationRepository, Clock, LibraryRepository},
};
use folioharbor_domain::id::{LibraryId, RequestId, UserId};

pub struct RemoveMemberCommand {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub member: UserId,
    pub request_id: RequestId,
}
pub struct RemoveMember<'a, R, A, C> {
    repository: &'a R,
    authorization: &'a A,
    clock: &'a C,
}
impl<'a, R, A, C> RemoveMember<'a, R, A, C> {
    #[must_use]
    pub const fn new(repository: &'a R, authorization: &'a A, clock: &'a C) -> Self {
        Self {
            repository,
            authorization,
            clock,
        }
    }
}
impl<R: LibraryRepository, A: AuthorizationRepository, C: Clock> RemoveMember<'_, R, A, C> {
    /// Removes one active library member.
    ///
    /// # Errors
    /// Returns an application error when authorization, owner invariants, or persistence fails.
    pub async fn execute(&self, c: RemoveMemberCommand) -> Result<(), AppError> {
        let resource = ResourceRef::Membership {
            library_id: c.library_id,
            user_id: c.member,
        };
        let grant = Authorization::new(self.authorization)
            .require(c.actor, Action::RemoveMember, resource)
            .await?;
        let now = self.clock.now();
        let audit = AuditEvent::allowed(c.actor, Action::RemoveMember, resource, c.request_id, now);
        let outcome = self
            .repository
            .remove_member(c.actor, c.library_id, c.member, now, grant, audit)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })?;
        super::mutation_result(outcome)
    }
}
