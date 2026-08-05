use anyhow::Context;
use folioharbor_application::ports::{IdentityRepository, NewAccount, NewSession, RegisterOutcome};
use folioharbor_domain::{
    id::{SessionId, UserId},
    identity::{EmailVerificationToken, NormalizedEmail, PasswordResetToken, SessionToken},
};
use folioharbor_postgres::identity::PgIdentityRepository;
use folioharbor_postgres::run_migrations;
use folioharbor_test_support::postgres::TestPostgres;
use secrecy::SecretString;
use sqlx::PgPool;
use time::OffsetDateTime;

#[tokio::test]
async fn identity_migration_creates_all_six_relations() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner).await?;

    let relations: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = 'folioharbor' AND table_name = ANY($1) ORDER BY table_name",
    )
    .bind(
        &[
            "email_verification_tokens",
            "password_credentials",
            "password_reset_tokens",
            "user_accounts",
            "user_devices",
            "user_sessions",
        ][..],
    )
    .fetch_all(&owner)
    .await?;

    owner.close().await;
    database.cleanup().await.context("cleaning test database")?;
    assert_eq!(relations.len(), 6);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn repository_enforces_uniqueness_single_use_expiry_and_revocation() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner).await?;
    let api = PgPool::connect(&database.api_url()?).await?;
    let repository = PgIdentityRepository::new(api.clone());
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let email = NormalizedEmail::parse("Reader@Example.COM")?;
    let verification = EmailVerificationToken::from_random_bytes([1; 32]);
    let user_id = UserId::new();

    let outcome = repository
        .register(NewAccount {
            user_id,
            normalized_email: email.clone(),
            display_email: "Reader@Example.COM".to_owned(),
            password_hash: "old hash".to_owned(),
            verification_token_hash: verification.hash_for_storage(),
            created_at: now,
            verification_expires_at: now + time::Duration::hours(1),
        })
        .await?;
    assert_eq!(outcome, RegisterOutcome::Created);
    let duplicate = repository
        .register(NewAccount {
            user_id: UserId::new(),
            normalized_email: email.clone(),
            display_email: "other casing".to_owned(),
            password_hash: "different".to_owned(),
            verification_token_hash: EmailVerificationToken::from_random_bytes([2; 32])
                .hash_for_storage(),
            created_at: now,
            verification_expires_at: now + time::Duration::hours(1),
        })
        .await?;
    assert_eq!(duplicate, RegisterOutcome::Existing);

    let hash = verification.hash_for_storage();
    let (first, second) = tokio::join!(
        repository.verify_email(hash, now),
        repository.verify_email(hash, now),
    );
    let consumed = [first?, second?].into_iter().flatten().count();
    assert_eq!(consumed, 1, "exactly one concurrent consumer wins");

    let expired_verification = EmailVerificationToken::from_random_bytes([7; 32]);
    repository
        .register(NewAccount {
            user_id: UserId::new(),
            normalized_email: NormalizedEmail::parse("expired@example.com")?,
            display_email: "expired@example.com".to_owned(),
            password_hash: "hash".to_owned(),
            verification_token_hash: expired_verification.hash_for_storage(),
            created_at: now,
            verification_expires_at: now + time::Duration::minutes(1),
        })
        .await?;
    assert!(
        repository
            .verify_email(
                expired_verification.hash_for_storage(),
                now + time::Duration::minutes(1)
            )
            .await?
            .is_none()
    );

    let session_token = SessionToken::parse(SecretString::from("session secret".to_owned()));
    let session_id = SessionId::new();
    repository
        .create_session(NewSession {
            session_id,
            user_id,
            session_token_hash: session_token.hash_for_storage(),
            csrf_token_hash: SessionToken::parse(SecretString::from("csrf secret".to_owned()))
                .hash_for_storage(),
            created_at: now,
            idle_expires_at: now + time::Duration::minutes(30),
            absolute_expires_at: now + time::Duration::hours(2),
        })
        .await?;
    assert!(
        repository
            .authenticate_session(
                session_token.hash_for_storage(),
                now + time::Duration::minutes(29),
                now + time::Duration::minutes(59)
            )
            .await?
            .is_some()
    );
    assert!(
        repository
            .authenticate_session(
                session_token.hash_for_storage(),
                now + time::Duration::hours(2),
                now + time::Duration::hours(3)
            )
            .await?
            .is_none()
    );
    repository
        .revoke_session(session_token.hash_for_storage(), now, "logout")
        .await?;
    repository
        .revoke_session(session_token.hash_for_storage(), now, "logout")
        .await?;

    let reset = PasswordResetToken::from_random_bytes([3; 32]);
    assert!(
        repository
            .issue_password_reset(
                &email,
                reset.hash_for_storage(),
                now,
                now + time::Duration::hours(1)
            )
            .await?
    );
    let reset_session = SessionToken::parse(SecretString::from("reset session".to_owned()));
    repository
        .create_session(NewSession {
            session_id: SessionId::new(),
            user_id,
            session_token_hash: reset_session.hash_for_storage(),
            csrf_token_hash: SessionToken::parse(SecretString::from("reset csrf".to_owned()))
                .hash_for_storage(),
            created_at: now,
            idle_expires_at: now + time::Duration::minutes(30),
            absolute_expires_at: now + time::Duration::hours(2),
        })
        .await?;
    assert!(
        repository
            .authenticate_session(
                reset_session.hash_for_storage(),
                now,
                now + time::Duration::minutes(30)
            )
            .await?
            .is_some()
    );
    assert_eq!(
        repository
            .reset_password(reset.hash_for_storage(), "new hash".to_owned(), now)
            .await?,
        Some(user_id)
    );
    assert!(
        repository
            .authenticate_session(
                reset_session.hash_for_storage(),
                now,
                now + time::Duration::minutes(30)
            )
            .await?
            .is_none()
    );
    assert_eq!(
        repository
            .reset_password(reset.hash_for_storage(), "another".to_owned(), now)
            .await?,
        None
    );
    let identity = repository
        .find_login_identity(&email)
        .await?
        .ok_or_else(|| anyhow::anyhow!("account disappeared after password reset"))?;
    assert_eq!(identity.password_hash, "new hash");

    api.close().await;
    owner.close().await;
    database.cleanup().await.context("cleaning test database")?;
    Ok(())
}
