use folioharbor_application::ports::{
    AcceptInvitationOutcome, LibraryMutationOutcome, LibraryRepository, NewLibraryInvitation,
};
use folioharbor_application::{
    audit::AuditEvent,
    authorization::{Action, Authorization, AuthorizationGrant, ResourceRef},
};
use folioharbor_domain::{
    id::{InvitationId, RequestId, UserId},
    identity::{NormalizedEmail, TokenHash},
    libraries::role::RoleCode,
    time::OffsetDateTime,
};
use folioharbor_postgres::{
    PgAuthorizationRepository, libraries::PgLibraryRepository, run_migrations,
};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::PgPool;

async fn authorized(
    api: &PgPool,
    actor: UserId,
    action: Action,
    resource: ResourceRef,
    now: OffsetDateTime,
) -> Result<(AuthorizationGrant, AuditEvent), folioharbor_application::error::AppError> {
    let grant = Authorization::new(&PgAuthorizationRepository::new(api.clone()))
        .require(actor, action, resource)
        .await?;
    Ok((
        grant,
        AuditEvent::allowed(actor, action, resource, RequestId::new(), now),
    ))
}

#[tokio::test]
async fn migrations_create_library_schema_and_seed_builtin_roles() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner).await?;
    let role_count: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.roles")
        .fetch_one(&owner)
        .await?;
    let permission_count: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.permissions")
        .fetch_one(&owner)
        .await?;
    let mapping_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM folioharbor.role_permissions")
            .fetch_one(&owner)
            .await?;
    assert_eq!((role_count, permission_count, mapping_count), (3, 6, 13));
    owner.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn invitations_are_email_bound_expiring_single_use_and_preserve_personal_libraries()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner_pool = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner_pool).await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let owner = UserId::new();
    let invitee = UserId::new();
    let attacker = UserId::new();
    for (user_id, email) in [
        (owner, "owner@example.com"),
        (invitee, "invitee@example.com"),
        (attacker, "attacker@example.com"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id, normalized_email, display_email, status, created_at) VALUES ($1,$2,$2,'verified',$3)").bind(user_id.as_uuid()).bind(email).bind(now).execute(&owner_pool).await?;
    }
    let api = PgPool::connect(&database.api_url()?).await?;
    let repository = PgLibraryRepository::new(api.clone());
    let shared = repository.provision_personal_library(owner, now).await?;
    let personal = repository.provision_personal_library(invitee, now).await?;
    let invitee_email = NormalizedEmail::parse("invitee@example.com")?;
    let token_hash = TokenHash::from_bytes([11; 32]);
    let invitation_id = InvitationId::new();
    let (grant, audit) = authorized(
        &api,
        owner,
        Action::InviteMember,
        ResourceRef::Invitation {
            library_id: shared.library_id,
            invitation_id,
        },
        now,
    )
    .await?;
    assert_eq!(
        repository
            .create_invitation(
                NewLibraryInvitation {
                    invitation_id,
                    library_id: shared.library_id,
                    invited_by: owner,
                    normalized_email: invitee_email.clone(),
                    display_email: "invitee@example.com".to_owned(),
                    role: RoleCode::Reader,
                    token_hash,
                    created_at: now,
                    expires_at: now + time::Duration::hours(1)
                },
                grant,
                audit
            )
            .await?,
        LibraryMutationOutcome::Applied
    );
    assert_eq!(
        repository
            .accept_invitation(attacker, token_hash, now)
            .await?,
        AcceptInvitationOutcome::WrongAccount {
            invited_email: "invitee@example.com".to_owned()
        }
    );
    let attacker_memberships: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.library_memberships WHERE library_id=$1 AND user_id=$2 AND status='active'",
    )
    .bind(shared.library_id.as_uuid())
    .bind(attacker.as_uuid())
    .fetch_one(&owner_pool)
    .await?;
    let consumed_at: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT consumed_at FROM folioharbor.library_invitations WHERE token_hash=$1",
    )
    .bind(token_hash.as_bytes().as_slice())
    .fetch_one(&owner_pool)
    .await?;
    assert_eq!(attacker_memberships, 0);
    assert!(consumed_at.is_none());
    assert_eq!(
        repository
            .accept_invitation(invitee, token_hash, now)
            .await?,
        AcceptInvitationOutcome::Accepted(shared.library_id)
    );
    assert_eq!(
        repository
            .accept_invitation(invitee, token_hash, now)
            .await?,
        AcceptInvitationOutcome::Consumed
    );
    let personal_after: uuid::Uuid = sqlx::query_scalar(
        "SELECT library_id FROM folioharbor.libraries WHERE personal_owner_id=$1",
    )
    .bind(invitee.as_uuid())
    .fetch_one(&owner_pool)
    .await?;
    assert_eq!(personal_after, personal.library_id.as_uuid());

    let expired_hash = TokenHash::from_bytes([12; 32]);
    let expired_id = InvitationId::new();
    let (grant, audit) = authorized(
        &api,
        owner,
        Action::InviteMember,
        ResourceRef::Invitation {
            library_id: shared.library_id,
            invitation_id: expired_id,
        },
        now,
    )
    .await?;
    repository
        .create_invitation(
            NewLibraryInvitation {
                invitation_id: expired_id,
                library_id: shared.library_id,
                invited_by: owner,
                normalized_email: invitee_email.clone(),
                display_email: "invitee@example.com".to_owned(),
                role: RoleCode::Editor,
                token_hash: expired_hash,
                created_at: now,
                expires_at: now + time::Duration::minutes(1),
            },
            grant,
            audit,
        )
        .await?;
    assert_eq!(
        repository
            .accept_invitation(invitee, expired_hash, now + time::Duration::minutes(2))
            .await?,
        AcceptInvitationOutcome::Expired
    );

    api.close().await;
    owner_pool.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn concurrent_final_owner_changes_allow_at_most_one_commit() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner).await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let first_owner = UserId::new();
    let second_owner = UserId::new();
    for (user_id, email) in [
        (first_owner, "one@example.com"),
        (second_owner, "two@example.com"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id, normalized_email, display_email, status, created_at) VALUES ($1, $2, $2, 'verified', $3)").bind(user_id.as_uuid()).bind(email).bind(now).execute(&owner).await?;
    }
    let api = PgPool::connect(&database.api_url()?).await?;
    let repository = PgLibraryRepository::new(api.clone());
    let library = repository
        .provision_personal_library(first_owner, now)
        .await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'owner','active',$3)").bind(library.library_id.as_uuid()).bind(second_owner.as_uuid()).bind(now).execute(&owner).await?;

    let (remove_grant, remove_audit) = authorized(
        &api,
        first_owner,
        Action::RemoveMember,
        ResourceRef::Membership {
            library_id: library.library_id,
            user_id: second_owner,
        },
        now,
    )
    .await?;
    let (change_grant, change_audit) = authorized(
        &api,
        second_owner,
        Action::ChangeMemberRole,
        ResourceRef::Membership {
            library_id: library.library_id,
            user_id: first_owner,
        },
        now,
    )
    .await?;

    let remove_repository = repository.clone();
    let change_repository = repository.clone();
    let remove = tokio::spawn(async move {
        remove_repository
            .remove_member(
                first_owner,
                library.library_id,
                second_owner,
                now,
                remove_grant,
                remove_audit,
            )
            .await
    });
    let demote = tokio::spawn(async move {
        change_repository
            .change_member_role(
                second_owner,
                library.library_id,
                first_owner,
                RoleCode::Reader,
                now,
                change_grant,
                change_audit,
            )
            .await
    });
    let outcomes = [remove.await??, demote.await??];
    assert!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == LibraryMutationOutcome::Applied)
            .count()
            <= 1
    );
    let active_owners: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.library_memberships WHERE library_id=$1 AND status='active' AND role_code='owner'").bind(library.library_id.as_uuid()).fetch_one(&owner).await?;
    assert!(active_owners >= 1);

    api.close().await;
    owner.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn editors_and_readers_cannot_manage_memberships_or_library_settings() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner_pool = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner_pool).await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let owner = UserId::new();
    let editor = UserId::new();
    let reader = UserId::new();
    for (user_id, email) in [
        (owner, "o@example.com"),
        (editor, "e@example.com"),
        (reader, "r@example.com"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at) VALUES($1,$2,$2,'verified',$3)").bind(user_id.as_uuid()).bind(email).bind(now).execute(&owner_pool).await?;
    }
    let api = PgPool::connect(&database.api_url()?).await?;
    let repository = PgLibraryRepository::new(api.clone());
    let library = repository.provision_personal_library(owner, now).await?;
    for (user_id, role) in [(editor, "editor"), (reader, "reader")] {
        sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,$3,'active',$4)").bind(library.library_id.as_uuid()).bind(user_id.as_uuid()).bind(role).bind(now).execute(&owner_pool).await?;
    }
    let authorization = PgAuthorizationRepository::new(api.clone());
    for (actor, action, resource) in [
        (
            editor,
            Action::ChangeMemberRole,
            ResourceRef::Membership {
                library_id: library.library_id,
                user_id: reader,
            },
        ),
        (
            reader,
            Action::RemoveMember,
            ResourceRef::Membership {
                library_id: library.library_id,
                user_id: editor,
            },
        ),
        (
            editor,
            Action::ManageLibrary,
            ResourceRef::Library(library.library_id),
        ),
        (
            reader,
            Action::InviteMember,
            ResourceRef::Invitation {
                library_id: library.library_id,
                invitation_id: InvitationId::new(),
            },
        ),
    ] {
        assert!(
            Authorization::new(&authorization)
                .require(actor, action, resource)
                .await
                .is_err()
        );
    }
    api.close().await;
    owner_pool.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn personal_library_repository_retry_returns_the_same_library() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner).await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let user_id = UserId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id, normalized_email, display_email, status, created_at) VALUES ($1, 'personal@example.com', 'personal@example.com', 'verified', $2)").bind(user_id.as_uuid()).bind(now).execute(&owner).await?;
    let api = PgPool::connect(&database.api_url()?).await?;
    let repository = PgLibraryRepository::new(api.clone());
    let first = repository.provision_personal_library(user_id, now).await?;
    let second = repository.provision_personal_library(user_id, now).await?;
    assert_eq!(first, second);
    api.close().await;
    owner.close().await;
    database.cleanup().await?;
    Ok(())
}
