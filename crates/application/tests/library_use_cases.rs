use async_trait::async_trait;
use folioharbor_application::error::AppError;
use folioharbor_application::libraries::{
    AcceptInvitation, AcceptInvitationCommand, ChangeMemberRole, ChangeMemberRoleCommand,
    InviteMember, InviteMemberCommand, ProvisionPersonalLibrary, ProvisionPersonalLibraryCommand,
    RemoveMember, RemoveMemberCommand, UpdateLibrarySettings, UpdateLibrarySettingsCommand,
};
use folioharbor_application::mail::{MailIntentSealer, MailOutboxError};
use folioharbor_application::ports::{
    AcceptInvitationOutcome, AuthorizationRepository, AuthorizationRepositoryError,
    LibraryMutationOutcome, LibraryRepository, LibraryRepositoryError, NewLibraryInvitation,
    NewMailOutboxEntry, RandomSource,
};
use folioharbor_application::{
    audit::AuditEvent,
    authorization::{Action, AuthorizationFact, AuthorizationGrant, ResourceRef},
};
use folioharbor_domain::id::{LibraryId, RequestId, UserId};
use folioharbor_domain::identity::{SessionToken, TokenHash};
use folioharbor_domain::libraries::Library;
use folioharbor_domain::libraries::role::{PermissionCode, RoleCode};
use folioharbor_domain::time::OffsetDateTime;
use folioharbor_test_support::clock::FakeClock;
use folioharbor_test_support::random::FixedRandom;
use secrecy::SecretString;
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
    mail: Mutex<Option<NewMailOutboxEntry>>,
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
            mail: Mutex::new(None),
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

    async fn create_invitation_with_mail(
        &self,
        invitation: NewLibraryInvitation,
        _: AuthorizationGrant,
        _: AuditEvent,
        mail: NewMailOutboxEntry,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        if self.invitation_outcome == LibraryMutationOutcome::Applied {
            *self
                .invitation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(invitation);
            *self
                .mail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(mail);
        }
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
    intent: Mutex<Option<(String, uuid::Uuid, String)>>,
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

impl MailIntentSealer for InvitationMailer {
    fn seal(
        &self,
        message: folioharbor_application::mail::MailMessage,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> Result<NewMailOutboxEntry, MailOutboxError> {
        if self.fail {
            return Err(MailOutboxError::Encryption);
        }
        let library_id = message
            .invitation_library_id()
            .ok_or(MailOutboxError::InvalidContext)?;
        let role = message
            .invitation_role()
            .ok_or(MailOutboxError::InvalidContext)?
            .to_owned();
        *self
            .intent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((
            message.recipient().as_str().to_owned(),
            library_id,
            role.clone(),
        ));
        Ok(NewMailOutboxEntry {
            mail_id: message.mail_id(),
            recipient_account_id: None,
            delivery_address: message.recipient().as_str().to_owned(),
            template_code: message.template().code(),
            template_version: 1,
            locale: message.locale().as_str(),
            token_ciphertext: vec![1],
            encryption_key_id: "test".to_owned(),
            nonce: vec![2; 12],
            idempotency_key: message.idempotency_key(),
            invitation_library_id: Some(library_id),
            invitation_role: Some(role),
            next_run_at: now,
            expires_at,
        })
    }
}

fn clock() -> FakeClock {
    FakeClock::new(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1_800_000_000))
}

#[tokio::test]
async fn invitation_and_encrypted_intent_use_one_combined_repository_operation()
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
    let (recipient, context_library, context_role) = mailer
        .intent
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .ok_or(std::io::Error::other("invitation should be sealed"))?;
    assert_eq!(recipient, "Reader@example.com");
    assert_eq!(context_library, library_id.as_uuid());
    assert_eq!(context_role, "reader");
    let stored = repository
        .invitation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .ok_or(std::io::Error::other("invitation hash should be persisted"))?;
    assert_eq!(stored.normalized_email.as_str(), recipient);
    let mail = repository
        .mail
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .ok_or(std::io::Error::other("mail intent should be persisted"))?;
    assert_eq!(mail.invitation_library_id, Some(library_id.as_uuid()));
    Ok(())
}

#[tokio::test]
async fn invitation_sealing_failure_prevents_business_and_outbox_persistence() {
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
        1,
        "sealing happens after generating the one-time token"
    );
    assert!(
        repository
            .mail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none(),
        "a failed sealer must not produce a mail intent"
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
        repository
            .mail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none(),
        "denied invitations must not persist a mail intent"
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
