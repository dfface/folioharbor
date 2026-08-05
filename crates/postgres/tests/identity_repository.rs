use anyhow::Context;
use folioharbor_application::ports::{
    IdentityRepository, NewAccount, NewSession, PasswordResetSession, RegisterOutcome,
};
use folioharbor_domain::{
    id::{DeviceId, LibraryId, RequestId, SessionId, UserId},
    identity::{
        EmailVerificationToken, NormalizedEmail, PasswordResetToken, SessionRevocationReason,
        SessionToken,
    },
};
use folioharbor_postgres::{
    DatabaseContext, PgTransactionContext, identity::PgIdentityRepository, run_migrations,
};
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
async fn disabled_account_cannot_consume_verification_token() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner).await?;
    let api = PgPool::connect(&database.api_url()?).await?;
    let repository = PgIdentityRepository::new(api.clone());
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let user_id = UserId::new();
    let token = EmailVerificationToken::from_random_bytes([21; 32]);
    repository
        .register(NewAccount {
            user_id,
            normalized_email: NormalizedEmail::parse("disabled@example.com")?,
            display_email: "disabled@example.com".to_owned(),
            password_hash: "hash".to_owned(),
            verification_token_hash: token.hash_for_storage(),
            created_at: now,
            verification_expires_at: now + time::Duration::hours(1),
        })
        .await?;
    sqlx::query("UPDATE folioharbor.user_accounts SET status = 'disabled', disabled_at = $2 WHERE user_id = $1")
        .bind(user_id.as_uuid()).bind(now).execute(&owner).await?;

    assert_eq!(
        repository
            .verify_email(token.hash_for_storage(), now)
            .await?,
        None
    );
    let consumed_at: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT consumed_at FROM folioharbor.email_verification_tokens WHERE user_id = $1",
    )
    .bind(user_id.as_uuid())
    .fetch_one(&owner)
    .await?;
    assert_eq!(consumed_at, None);

    api.close().await;
    owner.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn api_rls_isolates_session_and_device_rows_by_authenticated_user() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner).await?;
    let api = PgPool::connect(&database.api_url()?).await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let user_a = UserId::new();
    let user_b = UserId::new();
    for (user_id, email) in [(user_a, "a@example.com"), (user_b, "b@example.com")] {
        sqlx::query("INSERT INTO folioharbor.user_accounts (user_id, normalized_email, display_email, status, created_at) VALUES ($1, $2, $2, 'verified', $3)")
            .bind(user_id.as_uuid()).bind(email).bind(now).execute(&owner).await?;
    }
    for (index, user_id, session_byte, csrf_byte) in
        [(0, user_a, 31_u8, 41_u8), (1, user_b, 32_u8, 42_u8)]
    {
        sqlx::query("INSERT INTO folioharbor.user_sessions (session_id, user_id, session_token_hash, csrf_token_hash, created_at, last_seen_at, idle_expires_at, absolute_expires_at) VALUES ($1, $2, $3, $4, $5, $5, $6, $7)")
            .bind(SessionId::new().as_uuid()).bind(user_id.as_uuid())
            .bind(vec![session_byte; 32]).bind(vec![csrf_byte; 32]).bind(now)
            .bind(now + time::Duration::hours(1)).bind(now + time::Duration::hours(2)).execute(&owner).await?;
        sqlx::query("INSERT INTO folioharbor.user_devices (device_id, user_id, display_name, created_at, last_seen_at) VALUES ($1, $2, $3, $4, $4)")
            .bind(DeviceId::new().as_uuid()).bind(user_id.as_uuid()).bind(format!("device-{index}")).bind(now).execute(&owner).await?;
    }

    let unset_sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.user_sessions")
        .fetch_one(&api)
        .await?;
    let unset_devices: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.user_devices")
        .fetch_one(&api)
        .await?;
    assert_eq!((unset_sessions, unset_devices), (0, 0));

    let context = DatabaseContext::api(user_a, LibraryId::new(), RequestId::new());
    let mut transaction = api.begin().await?;
    PgTransactionContext::apply(&mut transaction, &context).await?;
    let visible_sessions: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT user_id FROM folioharbor.user_sessions")
            .fetch_all(&mut *transaction)
            .await?;
    let visible_devices: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT user_id FROM folioharbor.user_devices")
            .fetch_all(&mut *transaction)
            .await?;
    assert_eq!(visible_sessions, vec![user_a.as_uuid()]);
    assert_eq!(visible_devices, vec![user_a.as_uuid()]);
    let updated_sessions =
        sqlx::query("UPDATE folioharbor.user_sessions SET last_seen_at = $1 WHERE user_id = $2")
            .bind(now)
            .bind(user_b.as_uuid())
            .execute(&mut *transaction)
            .await?;
    let updated_devices = sqlx::query(
        "UPDATE folioharbor.user_devices SET display_name = 'forbidden' WHERE user_id = $1",
    )
    .bind(user_b.as_uuid())
    .execute(&mut *transaction)
    .await?;
    assert_eq!(
        (
            updated_sessions.rows_affected(),
            updated_devices.rows_affected()
        ),
        (0, 0)
    );
    transaction.commit().await?;

    let mut session_insert = api.begin().await?;
    PgTransactionContext::apply(&mut session_insert, &context).await?;
    let foreign_session = sqlx::query("INSERT INTO folioharbor.user_sessions (session_id, user_id, session_token_hash, csrf_token_hash, created_at, last_seen_at, idle_expires_at, absolute_expires_at) VALUES ($1, $2, $3, $4, $5, $5, $6, $7)")
        .bind(SessionId::new().as_uuid()).bind(user_b.as_uuid()).bind(vec![51_u8; 32]).bind(vec![52_u8; 32])
        .bind(now).bind(now + time::Duration::hours(1)).bind(now + time::Duration::hours(2)).execute(&mut *session_insert).await;
    assert!(foreign_session.is_err());
    session_insert.rollback().await?;

    let mut device_insert = api.begin().await?;
    PgTransactionContext::apply(&mut device_insert, &context).await?;
    let foreign_device = sqlx::query("INSERT INTO folioharbor.user_devices (device_id, user_id, display_name, created_at, last_seen_at) VALUES ($1, $2, 'forbidden', $3, $3)")
        .bind(DeviceId::new().as_uuid()).bind(user_b.as_uuid()).bind(now).execute(&mut *device_insert).await;
    assert!(foreign_device.is_err());
    device_insert.rollback().await?;

    api.close().await;
    owner.close().await;
    database.cleanup().await?;
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
        .revoke_session(
            session_token.hash_for_storage(),
            now,
            SessionRevocationReason::Logout,
        )
        .await?;
    let logout_reason: Option<String> = sqlx::query_scalar(
        "SELECT revocation_reason FROM folioharbor.user_sessions WHERE session_id = $1",
    )
    .bind(session_id.as_uuid())
    .fetch_one(&owner)
    .await?;
    assert_eq!(logout_reason.as_deref(), Some("logout"));
    repository
        .revoke_session(
            session_token.hash_for_storage(),
            now,
            SessionRevocationReason::Logout,
        )
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
            .reset_password(
                reset.hash_for_storage(),
                "new hash".to_owned(),
                PasswordResetSession {
                    session_id: SessionId::new(),
                    session_token_hash: SessionToken::parse(SecretString::from(
                        "new reset session".to_owned(),
                    ))
                    .hash_for_storage(),
                    csrf_token_hash: SessionToken::parse(SecretString::from(
                        "new reset csrf".to_owned(),
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
    let reset_reason: Option<String> = sqlx::query_scalar(
        "SELECT revocation_reason FROM folioharbor.user_sessions WHERE session_token_hash = $1",
    )
    .bind(reset_session.hash_for_storage().as_bytes().as_slice())
    .fetch_one(&owner)
    .await?;
    assert_eq!(reset_reason.as_deref(), Some("password_reset"));
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
            .reset_password(
                reset.hash_for_storage(),
                "another".to_owned(),
                PasswordResetSession {
                    session_id: SessionId::new(),
                    session_token_hash: SessionToken::parse(SecretString::from(
                        "unused session".to_owned(),
                    ))
                    .hash_for_storage(),
                    csrf_token_hash: SessionToken::parse(SecretString::from(
                        "unused csrf".to_owned(),
                    ))
                    .hash_for_storage(),
                    created_at: now,
                    idle_expires_at: now + time::Duration::minutes(30),
                    absolute_expires_at: now + time::Duration::hours(2),
                },
                now,
            )
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
