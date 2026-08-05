use folioharbor_application::ports::{
    AcceptInvitationOutcome, LibraryMutationOutcome, LibraryRepository, NewLibraryInvitation,
};
use folioharbor_domain::{
    id::{InvitationId, UserId},
    identity::{NormalizedEmail, TokenHash},
    libraries::role::RoleCode,
    time::OffsetDateTime,
};
use folioharbor_postgres::{libraries::PgLibraryRepository, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::PgPool;

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
async fn invitations_are_email_bound_expiring_single_use_and_preserve_personal_libraries()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner_pool = PgPool::connect(&database.owner_url()?).await?;
    run_migrations(&owner_pool).await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let owner = UserId::new();
    let invitee = UserId::new();
    for (user_id, email) in [
        (owner, "owner@example.com"),
        (invitee, "invitee@example.com"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id, normalized_email, display_email, status, created_at) VALUES ($1,$2,$2,'verified',$3)").bind(user_id.as_uuid()).bind(email).bind(now).execute(&owner_pool).await?;
    }
    let api = PgPool::connect(&database.api_url()?).await?;
    let repository = PgLibraryRepository::new(api.clone());
    let shared = repository.provision_personal_library(owner, now).await?;
    let personal = repository.provision_personal_library(invitee, now).await?;
    let invitee_email = NormalizedEmail::parse("invitee@example.com")?;
    let token_hash = TokenHash::from_bytes([11; 32]);
    assert_eq!(
        repository
            .create_invitation(NewLibraryInvitation {
                invitation_id: InvitationId::new(),
                library_id: shared.library_id,
                invited_by: owner,
                normalized_email: invitee_email.clone(),
                display_email: "invitee@example.com".to_owned(),
                role: RoleCode::Reader,
                token_hash,
                created_at: now,
                expires_at: now + time::Duration::hours(1)
            })
            .await?,
        LibraryMutationOutcome::Applied
    );
    assert_eq!(
        repository
            .accept_invitation(
                invitee,
                &NormalizedEmail::parse("wrong@example.com")?,
                token_hash,
                now
            )
            .await?,
        AcceptInvitationOutcome::Invalid
    );
    assert_eq!(
        repository
            .accept_invitation(invitee, &invitee_email, token_hash, now)
            .await?,
        AcceptInvitationOutcome::Accepted(shared.library_id)
    );
    assert_eq!(
        repository
            .accept_invitation(invitee, &invitee_email, token_hash, now)
            .await?,
        AcceptInvitationOutcome::Invalid
    );
    let personal_after: uuid::Uuid = sqlx::query_scalar(
        "SELECT library_id FROM folioharbor.libraries WHERE personal_owner_id=$1",
    )
    .bind(invitee.as_uuid())
    .fetch_one(&owner_pool)
    .await?;
    assert_eq!(personal_after, personal.library_id.as_uuid());

    let expired_hash = TokenHash::from_bytes([12; 32]);
    repository
        .create_invitation(NewLibraryInvitation {
            invitation_id: InvitationId::new(),
            library_id: shared.library_id,
            invited_by: owner,
            normalized_email: invitee_email.clone(),
            display_email: "invitee@example.com".to_owned(),
            role: RoleCode::Editor,
            token_hash: expired_hash,
            created_at: now,
            expires_at: now + time::Duration::minutes(1),
        })
        .await?;
    assert_eq!(
        repository
            .accept_invitation(
                invitee,
                &invitee_email,
                expired_hash,
                now + time::Duration::minutes(2)
            )
            .await?,
        AcceptInvitationOutcome::Invalid
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

    let remove_repository = repository.clone();
    let change_repository = repository.clone();
    let remove = tokio::spawn(async move {
        remove_repository
            .remove_member(first_owner, library.library_id, second_owner, now)
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
    assert_eq!(
        repository
            .change_member_role(editor, library.library_id, reader, RoleCode::Editor, now)
            .await?,
        LibraryMutationOutcome::Forbidden
    );
    assert_eq!(
        repository
            .remove_member(reader, library.library_id, editor, now)
            .await?,
        LibraryMutationOutcome::Forbidden
    );
    assert_eq!(
        repository
            .update_library_settings(editor, library.library_id, "Nope", now)
            .await?,
        LibraryMutationOutcome::Forbidden
    );
    assert_eq!(
        repository
            .create_invitation(NewLibraryInvitation {
                invitation_id: InvitationId::new(),
                library_id: library.library_id,
                invited_by: reader,
                normalized_email: NormalizedEmail::parse("next@example.com")?,
                display_email: "next@example.com".to_owned(),
                role: RoleCode::Reader,
                token_hash: TokenHash::from_bytes([33; 32]),
                created_at: now,
                expires_at: now + time::Duration::hours(1)
            })
            .await?,
        LibraryMutationOutcome::Forbidden
    );
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
