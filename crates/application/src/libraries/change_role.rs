use crate::{
    audit::AuditEvent,
    authorization::{Action, Authorization, ResourceRef},
    error::AppError,
    ports::{AuthorizationRepository, Clock, LibraryRepository},
};
use folioharbor_domain::{
    id::{LibraryId, RequestId, UserId},
    libraries::role::RoleCode,
};

pub struct ChangeMemberRoleCommand {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub member: UserId,
    pub role: RoleCode,
    pub request_id: RequestId,
}
pub struct ChangeMemberRole<'a, R, A, C> {
    repository: &'a R,
    authorization: &'a A,
    clock: &'a C,
}
impl<'a, R, A, C> ChangeMemberRole<'a, R, A, C> {
    #[must_use]
    pub const fn new(repository: &'a R, authorization: &'a A, clock: &'a C) -> Self {
        Self {
            repository,
            authorization,
            clock,
        }
    }
}
impl<R: LibraryRepository, A: AuthorizationRepository, C: Clock> ChangeMemberRole<'_, R, A, C> {
    /// Changes one active member's role.
    ///
    /// # Errors
    /// Returns an application error when authorization, owner invariants, or persistence fails.
    pub async fn execute(&self, c: ChangeMemberRoleCommand) -> Result<(), AppError> {
        let resource = ResourceRef::Membership {
            library_id: c.library_id,
            user_id: c.member,
        };
        let grant = Authorization::new(self.authorization)
            .require(c.actor, Action::ChangeMemberRole, resource)
            .await?;
        let now = self.clock.now();
        let audit = AuditEvent::allowed(
            c.actor,
            Action::ChangeMemberRole,
            resource,
            c.request_id,
            now,
        );
        let outcome = self
            .repository
            .change_member_role(c.actor, c.library_id, c.member, c.role, now, grant, audit)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })?;
        super::mutation_result(outcome)
    }
}
