use folioharbor_domain::{
    id::{InvitationId, LibraryId, RequestId, UserId},
    identity::{NormalizedEmail, SessionToken},
    libraries::role::RoleCode,
};
use time::Duration;

use crate::{
    audit::AuditEvent,
    authorization::{Action, Authorization, ResourceRef},
    error::{AppError, FieldViolation},
    mail::{Locale, MailIntentSealer, MailMessage, MailTemplate},
    ports::{
        AuthorizationRepository, Clock, LibraryMutationOutcome, LibraryRepository,
        NewLibraryInvitation, RandomSource,
    },
};

pub const INVITATION_LIFETIME: Duration = Duration::days(7);

pub struct InviteMemberCommand {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub email: String,
    pub role: RoleCode,
    pub request_id: RequestId,
}
pub struct InviteMember<'a, R, A, M, C, N> {
    repository: &'a R,
    authorization: &'a A,
    mailer: &'a M,
    clock: &'a C,
    random: &'a N,
}
impl<'a, R, A, M, C, N> InviteMember<'a, R, A, M, C, N> {
    #[must_use]
    pub const fn new(
        repository: &'a R,
        authorization: &'a A,
        mailer: &'a M,
        clock: &'a C,
        random: &'a N,
    ) -> Self {
        Self {
            repository,
            authorization,
            mailer,
            clock,
            random,
        }
    }
}
impl<
    R: LibraryRepository,
    A: AuthorizationRepository,
    M: MailIntentSealer,
    C: Clock,
    N: RandomSource,
> InviteMember<'_, R, A, M, C, N>
{
    /// Creates an email-bound invitation with a new opaque token.
    ///
    /// # Errors
    /// Returns an application error when validation, authorization, or persistence fails.
    pub async fn execute(&self, command: InviteMemberCommand) -> Result<(), AppError> {
        if command.role == RoleCode::Owner {
            return Err(AppError::Invalid {
                code: "invalid_invitation_role",
                fields: vec![FieldViolation {
                    field: "role",
                    code: "owner_not_invitable",
                }],
            });
        }
        let email = NormalizedEmail::parse(&command.email).map_err(|_| AppError::Invalid {
            code: "invalid_invitation",
            fields: vec![FieldViolation {
                field: "email",
                code: "invalid_email",
            }],
        })?;
        let invitation_id = InvitationId::new();
        let resource = ResourceRef::Invitation {
            library_id: command.library_id,
            invitation_id,
        };
        let grant = Authorization::new(self.authorization)
            .require(command.actor, Action::InviteMember, resource)
            .await?;
        let mut bytes = [0; 32];
        self.random.fill(&mut bytes);
        let token = SessionToken::from_random_bytes(bytes);
        let token_hash = token.hash_for_storage();
        let now = self.clock.now();
        let audit = AuditEvent::allowed(
            command.actor,
            Action::InviteMember,
            resource,
            command.request_id,
            now,
        );
        let mut message = MailMessage::new(
            None,
            email.clone(),
            MailTemplate::Invitation,
            Locale::En,
            token.into_secret(),
        );
        message
            .set_invitation_repository_context(command.library_id.as_uuid(), command.role.as_str());
        let mail = self
            .mailer
            .seal(message, now, now + INVITATION_LIFETIME)
            .map_err(|_| AppError::DependencyUnavailable {
                code: "mail_delivery_unavailable",
            })?;
        let outcome = self
            .repository
            .create_invitation_with_mail(
                NewLibraryInvitation {
                    invitation_id,
                    library_id: command.library_id,
                    invited_by: command.actor,
                    normalized_email: email.clone(),
                    display_email: command.email,
                    role: command.role,
                    token_hash,
                    created_at: now,
                    expires_at: now + INVITATION_LIFETIME,
                },
                grant,
                audit,
                mail,
            )
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })?;
        match outcome {
            LibraryMutationOutcome::Applied => Ok(()),
            other => {
                super::mutation_result(other)?;
                unreachable!()
            }
        }
    }
}
