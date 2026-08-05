use crate::{
    error::AppError,
    ports::{Clock, LibraryRepository},
};
use folioharbor_domain::id::{LibraryId, UserId};
pub struct RemoveMemberCommand {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub member: UserId,
}
pub struct RemoveMember<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}
impl<'a, R, C> RemoveMember<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}
impl<R: LibraryRepository, C: Clock> RemoveMember<'_, R, C> {
    /// Removes one active library member.
    ///
    /// # Errors
    /// Returns an application error when authorization, owner invariants, or persistence fails.
    pub async fn execute(&self, c: RemoveMemberCommand) -> Result<(), AppError> {
        let o = self
            .repository
            .remove_member(c.actor, c.library_id, c.member, self.clock.now())
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })?;
        super::mutation_result(o)
    }
}
