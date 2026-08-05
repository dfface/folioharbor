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
    ports::{
        AuthorizationRepository, Clock, LibraryInvitationContext, LibraryMutationOutcome,
        LibraryRepository, Mailer, NewLibraryInvitation, RandomSource,
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
impl<R: LibraryRepository, A: AuthorizationRepository, M: Mailer, C: Clock, N: RandomSource>
    InviteMember<'_, R, A, M, C, N>
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
        self.mailer
            .preflight_library_invitation()
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "mail_delivery_unavailable",
            })?;
        let mut bytes = [0; 32];
        self.random.fill(&mut bytes);
        let token = SessionToken::from_random_bytes(bytes);
        let now = self.clock.now();
        let audit = AuditEvent::allowed(
            command.actor,
            Action::InviteMember,
            resource,
            command.request_id,
            now,
        );
        let outcome = self
            .repository
            .create_invitation(
                NewLibraryInvitation {
                    invitation_id,
                    library_id: command.library_id,
                    invited_by: command.actor,
                    normalized_email: email.clone(),
                    display_email: command.email,
                    role: command.role,
                    token_hash: token.hash_for_storage(),
                    created_at: now,
                    expires_at: now + INVITATION_LIFETIME,
                },
                grant,
                audit,
            )
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })?;
        match outcome {
            LibraryMutationOutcome::Applied => self
                .mailer
                .send_library_invitation(
                    &email,
                    LibraryInvitationContext {
                        library_id: command.library_id,
                        role: command.role,
                    },
                    token.into_secret(),
                )
                .await
                .map_err(|_| AppError::DependencyUnavailable {
                    code: "mail_delivery_unavailable",
                }),
            other => {
                super::mutation_result(other)?;
                unreachable!()
            }
        }
    }
}
