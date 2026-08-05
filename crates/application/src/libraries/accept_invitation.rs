use crate::{
    error::{AppError, FieldViolation},
    ports::{AcceptInvitationOutcome, Clock, LibraryRepository},
};
use folioharbor_domain::{
    id::UserId,
    identity::{NormalizedEmail, SessionToken},
};
use secrecy::SecretString;

pub struct AcceptInvitationCommand {
    pub user_id: UserId,
    pub authenticated_email: String,
    pub token: SecretString,
}
pub struct AcceptInvitation<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}
impl<'a, R, C> AcceptInvitation<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}
impl<R: LibraryRepository, C: Clock> AcceptInvitation<'_, R, C> {
    /// Accepts an invitation bound to the authenticated account email.
    ///
    /// # Errors
    /// Returns an application error when validation, invitation checks, or persistence fails.
    pub async fn execute(
        &self,
        command: AcceptInvitationCommand,
    ) -> Result<folioharbor_domain::id::LibraryId, AppError> {
        let email = NormalizedEmail::parse(&command.authenticated_email).map_err(|_| {
            AppError::Invalid {
                code: "invalid_authenticated_email",
                fields: vec![FieldViolation {
                    field: "authenticated_email",
                    code: "invalid_email",
                }],
            }
        })?;
        let hash = SessionToken::parse(command.token).hash_for_storage();
        match self
            .repository
            .accept_invitation(command.user_id, &email, hash, self.clock.now())
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })? {
            AcceptInvitationOutcome::Accepted(id) => Ok(id),
            AcceptInvitationOutcome::Invalid => Err(AppError::Conflict {
                code: "invitation_invalid",
            }),
        }
    }
}
