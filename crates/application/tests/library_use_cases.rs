use async_trait::async_trait;
use folioharbor_application::error::AppError;
use folioharbor_application::libraries::{
    AcceptInvitation, AcceptInvitationCommand, ChangeMemberRole, ChangeMemberRoleCommand,
    InviteMember, InviteMemberCommand, ProvisionPersonalLibrary, ProvisionPersonalLibraryCommand,
    RemoveMember, RemoveMemberCommand, UpdateLibrarySettings, UpdateLibrarySettingsCommand,
};
use folioharbor_application::ports::{
    AcceptInvitationOutcome, AuthorizationRepository, AuthorizationRepositoryError,
    LibraryInvitationContext, LibraryMutationOutcome, LibraryRepository, LibraryRepositoryError,
    MailError, Mailer, NewLibraryInvitation, RandomSource,
};
use folioharbor_application::{
    audit::AuditEvent,
    authorization::{Action, AuthorizationFact, AuthorizationGrant, ResourceRef},
};
use folioharbor_domain::id::{LibraryId, RequestId, UserId};
use folioharbor_domain::identity::{NormalizedEmail, SessionToken, TokenHash};
use folioharbor_domain::libraries::Library;
use folioharbor_domain::libraries::role::{PermissionCode, RoleCode};
use folioharbor_domain::time::OffsetDateTime;
use folioharbor_test_support::clock::FakeClock;
use folioharbor_test_support::random::FixedRandom;
use secrecy::{ExposeSecret as _, SecretString};
use std::sync::Mutex;

#[test]
fn built_in_roles_and_permissions_are_stable_and_closed() {
    assert_eq!(RoleCode::parse("owner"), Some(RoleCode::Owner));
    assert_eq!(RoleCode::parse("editor"), Some(RoleCode::Editor));
    assert_eq!(RoleCode::parse("reader"), Some(RoleCode::Reader));
    assert_eq!(RoleCode::parse("custom"), None);
    assert_eq!(
        PermissionCode::ALL.map(PermissionCode::as_str),
        [
            "library.manage",
            "member.invite",
            "holding.view",
            "holding.edit",
            "item.read",
            "item.download",
        ]
    );
}

#[derive(Default)]
struct MissingLibraryRepository {
    library: Mutex<Option<Library>>,
}

#[async_trait]
impl LibraryRepository for MissingLibraryRepository {
    async fn provision_personal_library(
        &self,
        _: UserId,
        _: OffsetDateTime,
    ) -> Result<Library, LibraryRepositoryError> {
        let mut stored = self
            .library
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(stored
            .get_or_insert_with(|| Library {
                library_id: folioharbor_domain::id::LibraryId::new(),
                name: "Personal Library".to_owned(),
            })
            .clone())
    }
}

#[tokio::test]
async fn personal_library_provisioning_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
    let repository = MissingLibraryRepository::default();
    let clock = FakeClock::new(OffsetDateTime::from_unix_timestamp(1_800_000_000)?);
    let user_id = UserId::new();
    let provision = ProvisionPersonalLibrary::new(&repository, &clock);

    let first = provision
        .execute(ProvisionPersonalLibraryCommand { user_id })
        .await?;
    let second = provision
        .execute(ProvisionPersonalLibraryCommand { user_id })
        .await?;

    assert_eq!(first.library_id, second.library_id);
    Ok(())
}

struct CommandRepository {
    invitation: Mutex<Option<NewLibraryInvitation>>,
    invitation_outcome: LibraryMutationOutcome,
    mutation_outcome: LibraryMutationOutcome,
    accepted_user: UserId,
    accepted_hash: TokenHash,
    accepted_library: LibraryId,
}

impl CommandRepository {
    fn new(invitation_outcome: LibraryMutationOutcome) -> Self {
        Self {
            invitation: Mutex::new(None),
            invitation_outcome,
            mutation_outcome: LibraryMutationOutcome::Applied,
            accepted_user: UserId::new(),
            accepted_hash: TokenHash::from_bytes([0; 32]),
            accepted_library: LibraryId::new(),
        }
    }

    fn denied() -> Self {
        Self {
            mutation_outcome: LibraryMutationOutcome::Forbidden,
            ..Self::new(LibraryMutationOutcome::Forbidden)
        }
    }
}

#[async_trait]
impl LibraryRepository for CommandRepository {
    async fn provision_personal_library(
        &self,
        _: UserId,
        _: OffsetDateTime,
    ) -> Result<Library, LibraryRepositoryError> {
        Err(LibraryRepositoryError)
    }

    async fn create_invitation(
        &self,
        invitation: NewLibraryInvitation,
        _: AuthorizationGrant,
        _: AuditEvent,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        *self
            .invitation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(invitation);
        Ok(self.invitation_outcome)
    }

    async fn accept_invitation(
        &self,
        user_id: UserId,
        hash: TokenHash,
        _: OffsetDateTime,
    ) -> Result<AcceptInvitationOutcome, LibraryRepositoryError> {
        if user_id == self.accepted_user && hash == self.accepted_hash {
            Ok(AcceptInvitationOutcome::Accepted(self.accepted_library))
        } else {
            Ok(AcceptInvitationOutcome::Invalid)
        }
    }

    async fn change_member_role(
        &self,
        _: UserId,
        _: LibraryId,
        _: UserId,
        _: RoleCode,
        _: OffsetDateTime,
        _: AuthorizationGrant,
        _: AuditEvent,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        Ok(self.mutation_outcome)
    }

    async fn remove_member(
        &self,
        _: UserId,
        _: LibraryId,
        _: UserId,
        _: OffsetDateTime,
        _: AuthorizationGrant,
        _: AuditEvent,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        Ok(self.mutation_outcome)
    }

    async fn update_library_settings(
        &self,
        _: UserId,
        _: LibraryId,
        _: folioharbor_application::ports::LibrarySettingsUpdate<'_>,
        _: OffsetDateTime,
        _: AuthorizationGrant,
        _: AuditEvent,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        Ok(self.mutation_outcome)
    }
}

#[async_trait]
impl AuthorizationRepository for CommandRepository {
    async fn resolve(
        &self,
        _: UserId,
        _: Action,
        resource: ResourceRef,
    ) -> Result<Option<AuthorizationFact>, AuthorizationRepositoryError> {
        Ok(Some(AuthorizationFact {
            library_id: resource.library_id(),
            role: RoleCode::Owner,
            membership_version: 1,
            discoverable: true,
            permitted: true,
        }))
    }
}

#[derive(Default)]
struct InvitationMailer {
    delivery: Mutex<Option<(String, LibraryInvitationContext, String)>>,
    fail: bool,
}

#[derive(Default)]
struct CountingRandom {
    fills: Mutex<usize>,
}

impl RandomSource for CountingRandom {
    fn fill(&self, destination: &mut [u8]) {
        *self
            .fills
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        destination.fill(92);
    }
}

#[async_trait]
impl Mailer for InvitationMailer {
    async fn preflight_library_invitation(&self) -> Result<(), MailError> {
        if self.fail { Err(MailError) } else { Ok(()) }
    }

    async fn send_verification(
        &self,
        _: &NormalizedEmail,
        _: SecretString,
    ) -> Result<(), MailError> {
        Ok(())
    }

    async fn send_password_reset(
        &self,
        _: &NormalizedEmail,
        _: SecretString,
    ) -> Result<(), MailError> {
        Ok(())
    }

    async fn send_library_invitation(
        &self,
        recipient: &NormalizedEmail,
        context: LibraryInvitationContext,
        token: SecretString,
    ) -> Result<(), MailError> {
        *self
            .delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((
            recipient.as_str().to_owned(),
            context,
            token.expose_secret().to_owned(),
        ));
        if self.fail { Err(MailError) } else { Ok(()) }
    }
}

fn clock() -> FakeClock {
    FakeClock::new(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1_800_000_000))
}

#[tokio::test]
async fn invitation_plaintext_moves_directly_to_mailer_with_bound_context()
-> Result<(), Box<dyn std::error::Error>> {
    let repository = CommandRepository::new(LibraryMutationOutcome::Applied);
    let mailer = InvitationMailer::default();
    let library_id = LibraryId::new();

    let result = InviteMember::new(
        &repository,
        &repository,
        &mailer,
        &clock(),
        &FixedRandom::new(91),
    )
    .execute(InviteMemberCommand {
        actor: UserId::new(),
        library_id,
        email: "Reader@EXAMPLE.COM".to_owned(),
        role: RoleCode::Reader,
        request_id: RequestId::new(),
    })
    .await;

    assert!(result.is_ok());
    let (recipient, context, plaintext) = mailer
        .delivery
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .ok_or(std::io::Error::other("invitation should be delivered"))?;
    assert_eq!(recipient, "Reader@example.com");
    assert_eq!(context.library_id, library_id);
    assert_eq!(context.role, RoleCode::Reader);
    let stored = repository
        .invitation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .ok_or(std::io::Error::other("invitation hash should be persisted"))?;
    assert_eq!(stored.normalized_email.as_str(), recipient);
    assert_eq!(
        stored.token_hash,
        SessionToken::parse(SecretString::from(plaintext)).hash_for_storage()
    );
    Ok(())
}

#[tokio::test]
async fn unavailable_invitation_delivery_prevents_token_generation_and_persistence() {
    let repository = CommandRepository::new(LibraryMutationOutcome::Applied);
    let mailer = InvitationMailer {
        fail: true,
        ..InvitationMailer::default()
    };
    let random = CountingRandom::default();

    let result = InviteMember::new(&repository, &repository, &mailer, &clock(), &random)
        .execute(InviteMemberCommand {
            actor: UserId::new(),
            library_id: LibraryId::new(),
            email: "reader@example.com".to_owned(),
            role: RoleCode::Reader,
            request_id: RequestId::new(),
        })
        .await;

    assert!(matches!(
        result,
        Err(AppError::DependencyUnavailable {
            code: "mail_delivery_unavailable"
        })
    ));
    assert!(
        repository
            .invitation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none(),
        "an unavailable mailer must prevent invitation persistence"
    );
    assert_eq!(
        *random
            .fills
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        0,
        "an unavailable mailer must prevent token generation"
    );
    assert!(
        mailer
            .delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none(),
        "an unavailable mailer must not receive invitation plaintext"
    );
}

#[tokio::test]
async fn library_commands_preserve_owner_only_denials() {
    let repository = CommandRepository::denied();
    let mailer = InvitationMailer::default();
    let actor = UserId::new();
    let member = UserId::new();
    let library_id = LibraryId::new();
    let clock = clock();
    let forbidden = |result: Result<(), AppError>| {
        assert!(matches!(
            result,
            Err(AppError::Forbidden {
                code: "library_owner_required"
            })
        ));
    };

    forbidden(
        InviteMember::new(
            &repository,
            &repository,
            &mailer,
            &clock,
            &FixedRandom::new(1),
        )
        .execute(InviteMemberCommand {
            actor,
            library_id,
            email: "member@example.com".to_owned(),
            role: RoleCode::Editor,
            request_id: RequestId::new(),
        })
        .await,
    );
    assert!(
        mailer
            .delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none(),
        "denied invitations must not disclose a token to the mailer"
    );
    forbidden(
        ChangeMemberRole::new(&repository, &repository, &clock)
            .execute(ChangeMemberRoleCommand {
                actor,
                library_id,
                member,
                role: RoleCode::Reader,
                request_id: RequestId::new(),
            })
            .await,
    );
    forbidden(
        RemoveMember::new(&repository, &repository, &clock)
            .execute(RemoveMemberCommand {
                actor,
                library_id,
                member,
                request_id: RequestId::new(),
            })
            .await,
    );
    forbidden(
        UpdateLibrarySettings::new(&repository, &repository, &clock)
            .execute(UpdateLibrarySettingsCommand {
                actor,
                library_id,
                name: "Renamed".to_owned(),
                reader_download_enabled: None,
                request_id: RequestId::new(),
            })
            .await,
    );
}

#[tokio::test]
async fn invitation_acceptance_uses_only_trusted_user_id_and_token() -> Result<(), AppError> {
    let token = "single-use-invitation";
    let accepted_library = LibraryId::new();
    let user_id = UserId::new();
    let repository = CommandRepository {
        accepted_user: user_id,
        accepted_hash: SessionToken::parse(SecretString::from(token.to_owned())).hash_for_storage(),
        accepted_library,
        ..CommandRepository::new(LibraryMutationOutcome::Applied)
    };
    let clock = clock();
    let accept = AcceptInvitation::new(&repository, &clock);

    assert_eq!(
        accept
            .execute(AcceptInvitationCommand {
                user_id,
                token: SecretString::from(token.to_owned()),
            })
            .await?,
        accepted_library
    );
    assert!(matches!(
        accept
            .execute(AcceptInvitationCommand {
                user_id,
                token: SecretString::from("wrong-token".to_owned()),
            })
            .await,
        Err(AppError::Conflict {
            code: "invitation_invalid"
        })
    ));
    Ok(())
}
