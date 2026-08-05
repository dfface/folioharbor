use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use folioharbor_application::{
    error::AppError,
    identity::{
        CompletePasswordReset, CompletePasswordResetCommand, CurrentSession, ListSessions, Login,
        LoginCommand, Logout, LogoutCommand, PasswordResetRequested, PendingAccount,
        RegisterAccount, RegisterAccountCommand, RequestPasswordReset, RequestPasswordResetCommand,
    },
    ports::{
        Argon2PasswordHasher, IdentityRepository, IdentityRepositoryError, LoginIdentity,
        MailError, Mailer, NewAccount, NewSession, PasswordHashError, PasswordHasher,
        PasswordResetSession, RandomSource, RegisterOutcome, SessionPrincipal, SessionRecord,
    },
};
use folioharbor_domain::{
    id::{SessionId, UserId},
    identity::{
        AccountStatus, CsrfToken, NormalizedEmail, SessionRevocationReason, SessionStatus,
        SessionToken, TokenHash,
    },
    time::OffsetDateTime,
};
use folioharbor_test_support::{clock::FakeClock, random::FixedRandom};
use secrecy::{ExposeSecret, SecretString};

#[test]
fn normalized_email_trims_unicode_whitespace_and_lowercases_ascii_domain()
-> Result<(), Box<dyn std::error::Error>> {
    let email = NormalizedEmail::parse("\u{2003}Alice@EXAMPLE.COM\u{2002}")?;

    assert_eq!(email.as_str(), "Alice@example.com");
    Ok(())
}

#[test]
fn passwords_use_required_argon2id_parameters_and_verify() -> Result<(), PasswordHashError> {
    let hasher = Argon2PasswordHasher::new(FixedRandom::new(7));
    let password = SecretString::from("correct horse battery staple".to_owned());
    let hash = hasher.hash(&password)?;

    assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    assert!(hasher.verify(&password, &hash));
    assert!(!hasher.verify(&SecretString::from("wrong".to_owned()), &hash));
    Ok(())
}

struct FakeRepository {
    register_outcome: RegisterOutcome,
    login: Mutex<Option<LoginIdentity>>,
    reset_exists: bool,
    reset_user_id: Option<UserId>,
    created_session: Mutex<Option<NewSession>>,
    revocations: Mutex<Vec<SessionRevocationReason>>,
    sessions: Mutex<Vec<SessionRecord>>,
    reset_session: Mutex<Option<PasswordResetSession>>,
}

impl FakeRepository {
    fn empty() -> Self {
        Self {
            register_outcome: RegisterOutcome::Existing,
            login: Mutex::new(None),
            reset_exists: false,
            reset_user_id: None,
            created_session: Mutex::new(None),
            revocations: Mutex::new(Vec::new()),
            sessions: Mutex::new(Vec::new()),
            reset_session: Mutex::new(None),
        }
    }
}

#[async_trait]
impl IdentityRepository for FakeRepository {
    async fn register(&self, _: NewAccount) -> Result<RegisterOutcome, IdentityRepositoryError> {
        Ok(self.register_outcome)
    }
    async fn verify_email(
        &self,
        _: TokenHash,
        _: OffsetDateTime,
    ) -> Result<Option<UserId>, IdentityRepositoryError> {
        Ok(self.reset_user_id)
    }
    async fn find_login_identity(
        &self,
        _: &NormalizedEmail,
    ) -> Result<Option<LoginIdentity>, IdentityRepositoryError> {
        Ok(self
            .login
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }
    async fn create_session(&self, session: NewSession) -> Result<(), IdentityRepositoryError> {
        *self
            .created_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session);
        Ok(())
    }
    async fn authenticate_session(
        &self,
        _: TokenHash,
        _: OffsetDateTime,
        _: OffsetDateTime,
    ) -> Result<Option<SessionPrincipal>, IdentityRepositoryError> {
        Ok(None)
    }
    async fn revoke_session(
        &self,
        _: TokenHash,
        _: OffsetDateTime,
        reason: SessionRevocationReason,
    ) -> Result<(), IdentityRepositoryError> {
        self.revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(reason);
        Ok(())
    }
    async fn issue_password_reset(
        &self,
        _: &NormalizedEmail,
        _: TokenHash,
        _: OffsetDateTime,
        _: OffsetDateTime,
    ) -> Result<bool, IdentityRepositoryError> {
        Ok(self.reset_exists)
    }
    async fn reset_password(
        &self,
        _: TokenHash,
        _: String,
        session: PasswordResetSession,
        _: OffsetDateTime,
    ) -> Result<Option<UserId>, IdentityRepositoryError> {
        *self
            .reset_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session);
        Ok(self.reset_user_id)
    }
    async fn list_user_sessions(
        &self,
        _: UserId,
    ) -> Result<Vec<SessionRecord>, IdentityRepositoryError> {
        Ok(self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }
}

struct SpyHasher {
    valid: bool,
    dummy_calls: AtomicUsize,
}
struct SequenceRandom(AtomicUsize);
impl RandomSource for SequenceRandom {
    fn fill(&self, destination: &mut [u8]) {
        let value = if self.0.fetch_add(1, Ordering::SeqCst).is_multiple_of(2) {
            73
        } else {
            74
        };
        destination.fill(value);
    }
}
impl PasswordHasher for SpyHasher {
    fn hash(&self, _: &SecretString) -> Result<String, PasswordHashError> {
        Ok("phc".to_owned())
    }
    fn verify(&self, _: &SecretString, _: &str) -> bool {
        self.valid
    }
    fn verify_dummy(&self, _: &SecretString) {
        self.dummy_calls.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct SpyMailer {
    verification: AtomicUsize,
    resets: AtomicUsize,
    fail: bool,
}
#[async_trait]
impl Mailer for SpyMailer {
    async fn send_verification(
        &self,
        _: &NormalizedEmail,
        _: SecretString,
    ) -> Result<(), MailError> {
        self.verification.fetch_add(1, Ordering::SeqCst);
        if self.fail { Err(MailError) } else { Ok(()) }
    }
    async fn send_password_reset(
        &self,
        _: &NormalizedEmail,
        _: SecretString,
    ) -> Result<(), MailError> {
        self.resets.fetch_add(1, Ordering::SeqCst);
        if self.fail { Err(MailError) } else { Ok(()) }
    }
}

#[tokio::test]
async fn failing_mail_delivery_cannot_enumerate_registration_or_reset_accounts() {
    let known = FakeRepository {
        register_outcome: RegisterOutcome::Created,
        reset_exists: true,
        ..FakeRepository::empty()
    };
    let unknown = FakeRepository::empty();
    let hasher = SpyHasher {
        valid: true,
        dummy_calls: AtomicUsize::new(0),
    };
    let mailer = SpyMailer {
        verification: AtomicUsize::new(0),
        resets: AtomicUsize::new(0),
        fail: true,
    };
    let clock = fixture_clock();
    let random = FixedRandom::new(11);

    let known_registration = RegisterAccount::new(&known, &hasher, &mailer, &clock, &random)
        .execute(RegisterAccountCommand {
            email: "known@example.com".to_owned(),
            password: SecretString::from("password".to_owned()),
        })
        .await;
    let unknown_registration = RegisterAccount::new(&unknown, &hasher, &mailer, &clock, &random)
        .execute(RegisterAccountCommand {
            email: "unknown@example.com".to_owned(),
            password: SecretString::from("password".to_owned()),
        })
        .await;
    assert!(matches!(known_registration, Ok(PendingAccount)));
    assert!(matches!(unknown_registration, Ok(PendingAccount)));
    assert_eq!(mailer.verification.load(Ordering::SeqCst), 2);

    let known_reset = RequestPasswordReset::new(&known, &mailer, &clock, &random)
        .execute(RequestPasswordResetCommand {
            email: "known@example.com".to_owned(),
        })
        .await;
    let unknown_reset = RequestPasswordReset::new(&unknown, &mailer, &clock, &random)
        .execute(RequestPasswordResetCommand {
            email: "unknown@example.com".to_owned(),
        })
        .await;
    assert!(matches!(known_reset, Ok(PasswordResetRequested)));
    assert!(matches!(unknown_reset, Ok(PasswordResetRequested)));
    assert_eq!(mailer.resets.load(Ordering::SeqCst), 2);
}

fn fixture_clock() -> FakeClock {
    FakeClock::new(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1_800_000_000))
}

#[tokio::test]
async fn safe_session_status_uses_clock_for_idle_and_absolute_expiry() -> Result<(), AppError> {
    let now = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1_800_000_000);
    let idle = SessionId::new();
    let absolute = SessionId::new();
    let repository = FakeRepository {
        sessions: Mutex::new(vec![
            SessionRecord {
                session_id: idle,
                created_at: now - time::Duration::hours(2),
                last_seen_at: now - time::Duration::hours(1),
                idle_expires_at: now,
                absolute_expires_at: now + time::Duration::hours(1),
                revoked_at: None,
            },
            SessionRecord {
                session_id: absolute,
                created_at: now - time::Duration::hours(3),
                last_seen_at: now - time::Duration::minutes(1),
                idle_expires_at: now + time::Duration::minutes(30),
                absolute_expires_at: now,
                revoked_at: None,
            },
        ]),
        ..FakeRepository::empty()
    };
    let actor = folioharbor_application::actor::Actor {
        user_id: UserId::new(),
        session_id: idle,
    };

    let clock = FakeClock::new(now);
    let current = CurrentSession::new(&repository, &clock)
        .execute(actor)
        .await?;
    let listed = ListSessions::new(&repository, &clock)
        .execute(actor)
        .await?;

    assert_eq!(current.status, SessionStatus::IdleExpired);
    assert_eq!(listed[0].status, SessionStatus::IdleExpired);
    assert_eq!(listed[1].status, SessionStatus::AbsolutelyExpired);
    Ok(())
}

#[tokio::test]
async fn password_reset_issues_fresh_opaque_session_and_csrf_tokens() -> Result<(), AppError> {
    let user_id = UserId::new();
    let repository = FakeRepository {
        reset_user_id: Some(user_id),
        ..FakeRepository::empty()
    };
    let hasher = SpyHasher {
        valid: true,
        dummy_calls: AtomicUsize::new(0),
    };
    let clock = fixture_clock();
    let random = SequenceRandom(AtomicUsize::new(0));

    let issued = CompletePasswordReset::new(&repository, &hasher, &clock, &random)
        .execute(CompletePasswordResetCommand {
            token: SecretString::from("reset-token".to_owned()),
            new_password: SecretString::from("new-password".to_owned()),
        })
        .await?;

    assert_eq!(issued.user_id, user_id);
    assert_eq!(issued.session_token.expose_secret().len(), 43);
    assert_eq!(issued.csrf_token.expose_secret().len(), 43);
    assert_ne!(
        issued.session_token.expose_secret(),
        issued.csrf_token.expose_secret()
    );
    let stored = repository
        .reset_session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let stored = stored.as_ref().ok_or(AppError::DependencyUnavailable {
        code: "test_reset_session_missing",
    })?;
    assert_eq!(
        stored.session_token_hash,
        SessionToken::parse(SecretString::from(
            issued.session_token.expose_secret().to_owned()
        ))
        .hash_for_storage()
    );
    assert_eq!(
        stored.csrf_token_hash,
        CsrfToken::parse(SecretString::from(
            issued.csrf_token.expose_secret().to_owned()
        ))
        .hash_for_storage()
    );
    Ok(())
}

#[tokio::test]
async fn registration_and_reset_have_fixed_public_results_for_known_and_unknown_email()
-> Result<(), AppError> {
    let created = FakeRepository {
        register_outcome: RegisterOutcome::Created,
        reset_exists: true,
        ..FakeRepository::empty()
    };
    let existing = FakeRepository::empty();
    let hasher = SpyHasher {
        valid: true,
        dummy_calls: AtomicUsize::new(0),
    };
    let mailer = SpyMailer::default();
    let clock = fixture_clock();
    let random = FixedRandom::new(9);

    let first = RegisterAccount::new(&created, &hasher, &mailer, &clock, &random)
        .execute(RegisterAccountCommand {
            email: "Reader@Example.com".to_owned(),
            password: SecretString::from("password".to_owned()),
        })
        .await?;
    let second = RegisterAccount::new(&existing, &hasher, &mailer, &clock, &random)
        .execute(RegisterAccountCommand {
            email: "Reader@Example.com".to_owned(),
            password: SecretString::from("password".to_owned()),
        })
        .await?;
    assert_eq!(first, second);

    let known = RequestPasswordReset::new(&created, &mailer, &clock, &random)
        .execute(RequestPasswordResetCommand {
            email: "Reader@Example.com".to_owned(),
        })
        .await?;
    let unknown = RequestPasswordReset::new(&existing, &mailer, &clock, &random)
        .execute(RequestPasswordResetCommand {
            email: "Nobody@Example.com".to_owned(),
        })
        .await?;
    assert_eq!(known, unknown);
    assert_eq!(mailer.verification.load(Ordering::SeqCst), 2);
    assert_eq!(mailer.resets.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn login_is_non_enumerating_requires_verification_and_hashes_returned_tokens()
-> Result<(), Box<dyn std::error::Error>> {
    let repository = FakeRepository::empty();
    let hasher = SpyHasher {
        valid: false,
        dummy_calls: AtomicUsize::new(0),
    };
    let clock = fixture_clock();
    let random = FixedRandom::new(4);
    let unknown = Login::new(&repository, &hasher, &clock, &random)
        .execute(LoginCommand {
            email: "missing@example.com".to_owned(),
            password: SecretString::from("wrong".to_owned()),
        })
        .await;
    assert!(matches!(unknown, Err(AppError::Unauthenticated)));
    assert_eq!(hasher.dummy_calls.load(Ordering::SeqCst), 1);

    *repository
        .login
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(LoginIdentity {
        user_id: UserId::new(),
        password_hash: "phc".to_owned(),
        status: AccountStatus::PendingVerification,
    });
    let valid_hasher = SpyHasher {
        valid: true,
        dummy_calls: AtomicUsize::new(0),
    };
    let pending = Login::new(&repository, &valid_hasher, &clock, &random)
        .execute(LoginCommand {
            email: "reader@example.com".to_owned(),
            password: SecretString::from("right".to_owned()),
        })
        .await;
    assert!(matches!(
        pending,
        Err(AppError::Forbidden {
            code: "email_verification_required"
        })
    ));

    {
        let mut login = repository
            .login
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(identity) = login.as_mut() {
            identity.status = AccountStatus::Verified;
        }
    }
    let issued = Login::new(&repository, &valid_hasher, &clock, &random)
        .execute(LoginCommand {
            email: "reader@example.com".to_owned(),
            password: SecretString::from("right".to_owned()),
        })
        .await?;
    let stored = repository
        .created_session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let stored = stored.as_ref().ok_or("session was not persisted")?;
    let returned_hash = SessionToken::parse(SecretString::from(
        issued.session_token.expose_secret().to_owned(),
    ))
    .hash_for_storage();
    assert_eq!(stored.session_token_hash, returned_hash);
    assert_ne!(
        stored.session_token_hash.as_bytes().as_slice(),
        issued.session_token.expose_secret().as_bytes()
    );
    Ok(())
}

#[tokio::test]
async fn logout_is_idempotent_at_the_use_case_boundary() -> Result<(), AppError> {
    let repository = FakeRepository::empty();
    let clock = fixture_clock();
    let logout = Logout::new(&repository, &clock);
    for _ in 0..2 {
        logout
            .execute(LogoutCommand {
                session_token: SecretString::from("same token".to_owned()),
            })
            .await?;
    }
    assert_eq!(
        repository
            .revocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        [
            SessionRevocationReason::Logout,
            SessionRevocationReason::Logout
        ]
    );
    Ok(())
}
