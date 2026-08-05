use crate::{
    error::AppError,
    ports::{Clock, LibraryRepository},
};
use folioharbor_domain::{
    id::{LibraryId, UserId},
    libraries::role::RoleCode,
};
pub struct ChangeMemberRoleCommand {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub member: UserId,
    pub role: RoleCode,
}
pub struct ChangeMemberRole<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}
impl<'a, R, C> ChangeMemberRole<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}
impl<R: LibraryRepository, C: Clock> ChangeMemberRole<'_, R, C> {
    /// Changes one active member's role.
    ///
    /// # Errors
    /// Returns an application error when authorization, owner invariants, or persistence fails.
    pub async fn execute(&self, c: ChangeMemberRoleCommand) -> Result<(), AppError> {
        let o = self
            .repository
            .change_member_role(c.actor, c.library_id, c.member, c.role, self.clock.now())
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })?;
        super::mutation_result(o)
    }
}
