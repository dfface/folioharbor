use crate::{
    error::AppError,
    ports::{AcceptInvitationOutcome, Clock, LibraryRepository},
};
use folioharbor_domain::{id::UserId, identity::SessionToken};
use secrecy::SecretString;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InvitationAcceptance {
    Accepted {
        library_id: folioharbor_domain::id::LibraryId,
    },
    WrongAccount {
        email_hint: String,
    },
    Unverified,
    Expired,
    Consumed,
    Invalid,
}

pub struct AcceptInvitationCommand {
    pub user_id: UserId,
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
    /// Returns a safe, display-ready acceptance state while consuming a valid invitation
    /// atomically. Email hints are masked before leaving the application boundary.
    ///
    /// # Errors
    /// Returns an application error when persistence is unavailable.
    pub async fn execute_detailed(
        &self,
        command: AcceptInvitationCommand,
    ) -> Result<InvitationAcceptance, AppError> {
        let hash = SessionToken::parse(command.token).hash_for_storage();
        let outcome = self
            .repository
            .accept_invitation(command.user_id, hash, self.clock.now())
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })?;
        Ok(match outcome {
            AcceptInvitationOutcome::Accepted(library_id) => {
                InvitationAcceptance::Accepted { library_id }
            }
            AcceptInvitationOutcome::WrongAccount { invited_email } => {
                InvitationAcceptance::WrongAccount {
                    email_hint: mask_email(&invited_email),
                }
            }
            AcceptInvitationOutcome::Unverified => InvitationAcceptance::Unverified,
            AcceptInvitationOutcome::Expired => InvitationAcceptance::Expired,
            AcceptInvitationOutcome::Consumed => InvitationAcceptance::Consumed,
            AcceptInvitationOutcome::Invalid => InvitationAcceptance::Invalid,
        })
    }

    /// Accepts an invitation bound to the authenticated account email.
    ///
    /// # Errors
    /// Returns an application error when validation, invitation checks, or persistence fails.
    pub async fn execute(
        &self,
        command: AcceptInvitationCommand,
    ) -> Result<folioharbor_domain::id::LibraryId, AppError> {
        match self.execute_detailed(command).await? {
            InvitationAcceptance::Accepted { library_id } => Ok(library_id),
            InvitationAcceptance::WrongAccount { .. }
            | InvitationAcceptance::Unverified
            | InvitationAcceptance::Expired
            | InvitationAcceptance::Consumed
            | InvitationAcceptance::Invalid => Err(AppError::Conflict {
                code: "invitation_invalid",
            }),
        }
    }
}

fn mask_email(email: &str) -> String {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return "***".to_owned();
    };
    let first = local.chars().next().map_or("", |character| {
        let length = character.len_utf8();
        &local[..length]
    });
    format!("{first}***@{domain}")
}
