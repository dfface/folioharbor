use anyhow::Context as _;
use folioharbor_application::ports::{
    IdentityRepository, NewAccount, NewSession, PasswordResetSession,
};
use folioharbor_domain::{
    id::{SessionId, UserId},
    identity::{EmailVerificationToken, NormalizedEmail, PasswordResetToken, SessionToken},
};
use folioharbor_postgres::{identity::PgIdentityRepository, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use secrecy::SecretString;
use sqlx::PgPool;
use time::OffsetDateTime;

#[tokio::test]
async fn password_reset_atomically_revokes_old_sessions_and_issues_replacement()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner).await?;
    let api = PgPool::connect(&database.api_url()?).await?;
    let repository = PgIdentityRepository::new(api.clone());
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let user_id = UserId::new();
    let verification = EmailVerificationToken::from_random_bytes([1; 32]);
    repository
        .register(NewAccount {
            user_id,
            normalized_email: NormalizedEmail::parse("rotation@example.com")?,
            display_email: "rotation@example.com".to_owned(),
            password_hash: "old hash".to_owned(),
            verification_token_hash: verification.hash_for_storage(),
            created_at: now,
            verification_expires_at: now + time::Duration::hours(1),
        })
        .await?;
    repository
        .verify_email(verification.hash_for_storage(), now)
        .await?;
    let old = SessionToken::parse(SecretString::from("old session".to_owned()));
    repository
        .create_session(NewSession {
            session_id: SessionId::new(),
            user_id,
            session_token_hash: old.hash_for_storage(),
            csrf_token_hash: SessionToken::parse(SecretString::from("old csrf".to_owned()))
                .hash_for_storage(),
            created_at: now,
            idle_expires_at: now + time::Duration::minutes(30),
            absolute_expires_at: now + time::Duration::hours(2),
        })
        .await?;
    let reset = PasswordResetToken::from_random_bytes([2; 32]);
    repository
        .issue_password_reset(
            &NormalizedEmail::parse("rotation@example.com")?,
            reset.hash_for_storage(),
            now,
            now + time::Duration::hours(1),
        )
        .await?;
    let replacement = SessionToken::parse(SecretString::from("replacement session".to_owned()));
    let replacement_hash = replacement.hash_for_storage();
    let replacement_id = SessionId::new();

    assert_eq!(
        repository
            .reset_password(
                reset.hash_for_storage(),
                "new hash".to_owned(),
                PasswordResetSession {
                    session_id: replacement_id,
                    session_token_hash: replacement_hash,
                    csrf_token_hash: SessionToken::parse(SecretString::from(
                        "replacement csrf".to_owned()
                    ))
                    .hash_for_storage(),
                    created_at: now,
                    idle_expires_at: now + time::Duration::minutes(30),
                    absolute_expires_at: now + time::Duration::hours(2),
                },
                now,
            )
            .await?,
        Some(user_id)
    );
    assert!(
        repository
            .authenticate_session(
                old.hash_for_storage(),
                now,
                now + time::Duration::minutes(30)
            )
            .await?
            .is_none()
    );
    let principal = repository
        .authenticate_session(replacement_hash, now, now + time::Duration::minutes(30))
        .await?
        .context("replacement session was not issued")?;
    assert_eq!(principal.session_id, replacement_id);

    api.close().await;
    owner.close().await;
    database.cleanup().await.context("cleaning test database")?;
    Ok(())
}
