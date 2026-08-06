use folioharbor_application::ports::{
    IdentityRepository, LeaseMail, LibraryMutationOutcome, LibraryRepository, MailRepository,
    NewAccount, NewLibraryInvitation, NewMailOutboxEntry, RegisterOutcome,
};
use folioharbor_application::{
    audit::AuditEvent,
    authorization::{Action, Authorization, ResourceRef},
};
use folioharbor_domain::{
    id::{InvitationId, RequestId, UserId},
    identity::{NormalizedEmail, PasswordResetToken, TokenHash},
    libraries::role::RoleCode,
};
use folioharbor_postgres::{
    PgAuthorizationRepository, PgMailRepository, PgPools, identity::PgIdentityRepository,
    libraries::PgLibraryRepository, run_migrations,
};
use folioharbor_test_support::postgres::TestPostgres;
use time::OffsetDateTime;
use uuid::Uuid;

fn mail_entry(
    user_id: UserId,
    address: &str,
    template: &'static str,
    locale: &'static str,
    now: OffsetDateTime,
) -> NewMailOutboxEntry {
    NewMailOutboxEntry {
        mail_id: Uuid::now_v7(),
        recipient_account_id: Some(user_id.as_uuid()),
        delivery_address: address.to_owned(),
        template_code: template,
        template_version: 1,
        locale,
        token_ciphertext: vec![7; 48],
        encryption_key_id: "test-key".to_owned(),
        nonce: vec![8; 12],
        idempotency_key: format!("mail:{}", Uuid::now_v7()),
        invitation_library_id: None,
        invitation_role: None,
        next_run_at: now,
        expires_at: now + time::Duration::hours(1),
    }
}

fn account(user_id: UserId, now: OffsetDateTime) -> NewAccount {
    let mut token_hash = [0_u8; 32];
    token_hash[..16].copy_from_slice(user_id.as_uuid().as_bytes());
    token_hash[16..].copy_from_slice(user_id.as_uuid().as_bytes());
    NewAccount {
        user_id,
        normalized_email: NormalizedEmail::parse("atomic@example.com")
            .expect("fixture email is valid"),
        display_email: "atomic@example.com".to_owned(),
        password_hash: "hash".to_owned(),
        verification_token_hash: TokenHash::from_bytes(token_hash),
        created_at: now,
        verification_expires_at: now + time::Duration::hours(1),
    }
}

#[tokio::test]
async fn registration_and_verification_intent_commit_or_roll_back_together() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let repository = PgIdentityRepository::new(pools.api.clone());
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;

    let committed_user = UserId::new();
    assert_eq!(
        repository
            .register_with_verification(
                account(committed_user, now),
                mail_entry(
                    committed_user,
                    "atomic@example.com",
                    "verification",
                    "en",
                    now
                ),
            )
            .await?,
        RegisterOutcome::Created
    );
    let committed: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.user_accounts WHERE user_id=$1), \
                (SELECT count(*) FROM folioharbor.mail_outbox WHERE recipient_account_id=$1)",
    )
    .bind(committed_user.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(committed, (1, 1));

    let rolled_back_user = UserId::new();
    let failed = repository
        .register_with_verification(
            NewAccount {
                normalized_email: NormalizedEmail::parse("rollback@example.com")?,
                display_email: "rollback@example.com".to_owned(),
                ..account(rolled_back_user, now)
            },
            NewMailOutboxEntry {
                delivery_address: "rollback@example.com".to_owned(),
                ..mail_entry(
                    rolled_back_user,
                    "rollback@example.com",
                    "verification",
                    "unsupported",
                    now,
                )
            },
        )
        .await;
    assert!(failed.is_err());
    let rolled_back: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.user_accounts WHERE user_id=$1), \
                (SELECT count(*) FROM folioharbor.mail_outbox WHERE recipient_account_id=$1)",
    )
    .bind(rolled_back_user.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(rolled_back, (0, 0));

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn reset_token_and_reset_intent_commit_or_roll_back_together() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let repository = PgIdentityRepository::new(pools.api.clone());
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;

    let committed_user = UserId::new();
    repository.register(account(committed_user, now)).await?;
    let committed_email = NormalizedEmail::parse("atomic@example.com")?;
    assert_eq!(
        repository
            .mail_recipient_account_id(&committed_email)
            .await?,
        Some(committed_user)
    );
    let committed_token = PasswordResetToken::from_random_bytes([51; 32]);
    assert!(
        repository
            .issue_password_reset_with_mail(
                &committed_email,
                committed_token.hash_for_storage(),
                now,
                now + time::Duration::hours(1),
                mail_entry(
                    committed_user,
                    "atomic@example.com",
                    "password_reset",
                    "en",
                    now,
                ),
            )
            .await?
    );
    let committed: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.password_reset_tokens WHERE user_id=$1), \
                (SELECT count(*) FROM folioharbor.mail_outbox WHERE recipient_account_id=$1 AND template_code='password_reset')",
    )
    .bind(committed_user.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(committed, (1, 1));

    let rolled_back_user = UserId::new();
    repository
        .register(NewAccount {
            normalized_email: NormalizedEmail::parse("reset-rollback@example.com")?,
            display_email: "reset-rollback@example.com".to_owned(),
            ..account(rolled_back_user, now)
        })
        .await?;
    let rolled_back_email = NormalizedEmail::parse("reset-rollback@example.com")?;
    let failed = repository
        .issue_password_reset_with_mail(
            &rolled_back_email,
            PasswordResetToken::from_random_bytes([52; 32]).hash_for_storage(),
            now,
            now + time::Duration::hours(1),
            mail_entry(
                rolled_back_user,
                "reset-rollback@example.com",
                "password_reset",
                "unsupported",
                now,
            ),
        )
        .await;
    assert!(failed.is_err());
    let rolled_back: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.password_reset_tokens WHERE user_id=$1), \
                (SELECT count(*) FROM folioharbor.mail_outbox WHERE recipient_account_id=$1)",
    )
    .bind(rolled_back_user.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(rolled_back, (0, 0));

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn invitation_audit_and_mail_intent_commit_or_roll_back_together() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let identities = PgIdentityRepository::new(pools.api.clone());
    let libraries = PgLibraryRepository::new(pools.api.clone());
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let owner = UserId::new();
    identities.register(account(owner, now)).await?;
    let library = libraries.provision_personal_library(owner, now).await?;

    let committed_id = InvitationId::new();
    let committed_resource = ResourceRef::Invitation {
        library_id: library.library_id,
        invitation_id: committed_id,
    };
    let committed_grant = Authorization::new(&PgAuthorizationRepository::new(pools.api.clone()))
        .require(owner, Action::InviteMember, committed_resource)
        .await?;
    let committed_request = RequestId::new();
    assert_eq!(
        libraries
            .create_invitation_with_mail(
                NewLibraryInvitation {
                    invitation_id: committed_id,
                    library_id: library.library_id,
                    invited_by: owner,
                    normalized_email: NormalizedEmail::parse("invitee@example.com")?,
                    display_email: "invitee@example.com".to_owned(),
                    role: RoleCode::Reader,
                    token_hash: TokenHash::from_bytes([61; 32]),
                    created_at: now,
                    expires_at: now + time::Duration::days(7),
                },
                committed_grant,
                AuditEvent::allowed(
                    owner,
                    Action::InviteMember,
                    committed_resource,
                    committed_request,
                    now,
                ),
                NewMailOutboxEntry {
                    recipient_account_id: None,
                    invitation_library_id: Some(library.library_id.as_uuid()),
                    invitation_role: Some("reader".to_owned()),
                    ..mail_entry(owner, "invitee@example.com", "invitation", "zh-CN", now,)
                },
            )
            .await?,
        LibraryMutationOutcome::Applied
    );
    let committed: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.library_invitations WHERE invitation_id=$1), \
                (SELECT count(*) FROM folioharbor.audit_events WHERE request_id=$2), \
                (SELECT count(*) FROM folioharbor.mail_outbox WHERE invitation_library_id=$3)",
    )
    .bind(committed_id.as_uuid())
    .bind(committed_request.as_ulid().to_string())
    .bind(library.library_id.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(committed, (1, 1, 1));

    let rolled_back_id = InvitationId::new();
    let rolled_back_resource = ResourceRef::Invitation {
        library_id: library.library_id,
        invitation_id: rolled_back_id,
    };
    let rolled_back_grant = Authorization::new(&PgAuthorizationRepository::new(pools.api.clone()))
        .require(owner, Action::InviteMember, rolled_back_resource)
        .await?;
    let rolled_back_request = RequestId::new();
    let failed = libraries
        .create_invitation_with_mail(
            NewLibraryInvitation {
                invitation_id: rolled_back_id,
                library_id: library.library_id,
                invited_by: owner,
                normalized_email: NormalizedEmail::parse("rollback-invite@example.com")?,
                display_email: "rollback-invite@example.com".to_owned(),
                role: RoleCode::Editor,
                token_hash: TokenHash::from_bytes([62; 32]),
                created_at: now,
                expires_at: now + time::Duration::days(7),
            },
            rolled_back_grant,
            AuditEvent::allowed(
                owner,
                Action::InviteMember,
                rolled_back_resource,
                rolled_back_request,
                now,
            ),
            NewMailOutboxEntry {
                recipient_account_id: None,
                invitation_library_id: Some(library.library_id.as_uuid()),
                invitation_role: Some("editor".to_owned()),
                ..mail_entry(
                    owner,
                    "rollback-invite@example.com",
                    "invitation",
                    "unsupported",
                    now,
                )
            },
        )
        .await;
    assert!(failed.is_err());
    let rolled_back: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.library_invitations WHERE invitation_id=$1), \
                (SELECT count(*) FROM folioharbor.audit_events WHERE request_id=$2), \
                (SELECT count(*) FROM folioharbor.mail_outbox WHERE delivery_address='rollback-invite@example.com')",
    )
    .bind(rolled_back_id.as_uuid())
    .bind(rolled_back_request.as_ulid().to_string())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(rolled_back, (0, 0, 0));

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn worker_leases_retry_and_terminal_transitions_wipe_ciphertext() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let api = PgMailRepository::new(pools.api.clone());
    let worker = PgMailRepository::new(pools.worker.clone());
    let mail_id = api
        .enqueue(NewMailOutboxEntry {
            recipient_account_id: None,
            ..mail_entry(
                UserId::new(),
                "delivery@example.com",
                "password_reset",
                "en",
                now,
            )
        })
        .await?;

    let first = worker
        .lease(LeaseMail {
            owner: "worker-a".to_owned(),
            now,
            lease_for: time::Duration::minutes(5),
            limit: 1,
        })
        .await?;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].mail_id, mail_id);
    assert_eq!(first[0].attempt, 1);
    let retry_at = now + time::Duration::minutes(1);
    assert!(
        worker
            .retry(mail_id, "worker-a", now, retry_at, "smtp_451")
            .await?
    );
    let second = worker
        .lease(LeaseMail {
            owner: "worker-b".to_owned(),
            now: retry_at,
            lease_for: time::Duration::minutes(5),
            limit: 1,
        })
        .await?;
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].attempt, 2);
    assert_eq!(second[0].idempotency_key, first[0].idempotency_key);
    assert!(worker.mark_sent(mail_id, "worker-b", retry_at).await?);
    let terminal: (String, Vec<u8>) = sqlx::query_as(
        "SELECT state,token_ciphertext FROM folioharbor.mail_outbox WHERE mail_id=$1",
    )
    .bind(mail_id)
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(terminal, ("sent".to_owned(), Vec::new()));

    let failed_id = api
        .enqueue(NewMailOutboxEntry {
            recipient_account_id: None,
            ..mail_entry(
                UserId::new(),
                "failure@example.com",
                "password_reset",
                "en",
                now,
            )
        })
        .await?;
    let failed_lease = worker
        .lease(LeaseMail {
            owner: "worker-c".to_owned(),
            now,
            lease_for: time::Duration::minutes(5),
            limit: 1,
        })
        .await?;
    assert_eq!(failed_lease[0].mail_id, failed_id);
    assert!(
        worker
            .mark_failed(failed_id, "worker-c", now, "smtp_550")
            .await?
    );
    let failed_ciphertext: Vec<u8> =
        sqlx::query_scalar("SELECT token_ciphertext FROM folioharbor.mail_outbox WHERE mail_id=$1")
            .bind(failed_id)
            .fetch_one(&pools.owner)
            .await?;
    assert!(failed_ciphertext.is_empty());

    let recovery_id = api
        .enqueue(NewMailOutboxEntry {
            recipient_account_id: None,
            ..mail_entry(
                UserId::new(),
                "recovery@example.com",
                "verification",
                "en",
                now,
            )
        })
        .await?;
    let abandoned = worker
        .lease(LeaseMail {
            owner: "worker-d".to_owned(),
            now,
            lease_for: time::Duration::minutes(5),
            limit: 1,
        })
        .await?;
    assert_eq!(abandoned[0].mail_id, recovery_id);
    let recovered = worker
        .lease(LeaseMail {
            owner: "worker-e".to_owned(),
            now: now + time::Duration::minutes(6),
            lease_for: time::Duration::minutes(5),
            limit: 1,
        })
        .await?;
    assert_eq!(recovered[0].mail_id, recovery_id);
    assert_eq!(recovered[0].attempt, 2);

    let expired_id = api
        .enqueue(NewMailOutboxEntry {
            recipient_account_id: None,
            expires_at: now + time::Duration::seconds(1),
            ..mail_entry(
                UserId::new(),
                "expired@example.com",
                "verification",
                "en",
                now,
            )
        })
        .await?;
    let _ = worker
        .lease(LeaseMail {
            owner: "worker-f".to_owned(),
            now: now + time::Duration::seconds(2),
            lease_for: time::Duration::minutes(5),
            limit: 1,
        })
        .await?;
    let expired: (String, Vec<u8>) = sqlx::query_as(
        "SELECT state,token_ciphertext FROM folioharbor.mail_outbox WHERE mail_id=$1",
    )
    .bind(expired_id)
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(expired, ("expired".to_owned(), Vec::new()));

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
