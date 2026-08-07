#![allow(clippy::too_many_lines)]

use folioharbor_domain::{
    id::{InvitationId, LibraryId, RequestId, UserId},
    identity::TokenHash,
};
use folioharbor_postgres::{DatabaseContext, PgPools, PgTransactionContext, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};

const TABLES: [&str; 3] = ["libraries", "library_memberships", "library_invitations"];

async fn read_count(
    pool: &PgPool,
    context: Option<&DatabaseContext>,
    table: &str,
) -> anyhow::Result<i64> {
    let mut tx = pool.begin().await?;
    if let Some(context) = context {
        PgTransactionContext::apply(&mut tx, context).await?;
    }
    let query = format!("SELECT count(*) FROM folioharbor.{table}");
    let count = sqlx::query_scalar(&query).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(count)
}

async fn assert_write_denied(
    pool: &PgPool,
    context: Option<&DatabaseContext>,
    table: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    if let Some(context) = context {
        PgTransactionContext::apply(&mut tx, context).await?;
    }
    let statement = format!("DELETE FROM folioharbor.{table}");
    let result = sqlx::query(&statement).execute(&mut *tx).await;
    assert!(
        result.is_err(),
        "runtime direct write unexpectedly succeeded for {table}"
    );
    tx.rollback().await?;
    Ok(())
}

async fn assert_cannot_disable_rls(pool: &PgPool, table: &str) -> anyhow::Result<()> {
    let statement = format!("ALTER TABLE folioharbor.{table} DISABLE ROW LEVEL SECURITY");
    assert!(sqlx::query(&statement).execute(pool).await.is_err());
    Ok(())
}

async fn assert_no_execute(pool: &PgPool, signatures: &[&str]) -> anyhow::Result<()> {
    for signature in signatures {
        let allowed: bool = sqlx::query_scalar(
            "SELECT CASE WHEN to_regprocedure($1) IS NULL THEN false ELSE has_function_privilege(current_user,to_regprocedure($1),'EXECUTE') END",
        )
                .bind(signature)
                .fetch_one(pool)
                .await?;
        assert!(!allowed, "runtime role can execute {signature}");
    }
    Ok(())
}

async fn assert_obsolete_library_helpers_cannot_execute(
    pool: &PgPool,
    context: &DatabaseContext,
    bob: UserId,
    bob_library: LibraryId,
    bob_version: i64,
    invitation_hash: &TokenHash,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    for (statement, arguments) in [
        (
            "SELECT * FROM folioharbor.library_get_visible($1,$2,$3)",
            (bob.as_uuid(), bob_library.as_uuid(), Some(bob_version)),
        ),
        (
            "SELECT * FROM folioharbor.library_members_visible($1,$2,$3)",
            (bob.as_uuid(), bob_library.as_uuid(), Some(bob_version)),
        ),
    ] {
        let mut tx = pool.begin().await?;
        PgTransactionContext::apply(&mut tx, context).await?;
        let result = sqlx::query(statement)
            .bind(arguments.0)
            .bind(arguments.1)
            .bind(arguments.2)
            .fetch_all(&mut *tx)
            .await;
        assert!(
            result.is_err(),
            "obsolete helper unexpectedly remained API-executable: {statement}"
        );
        tx.rollback().await?;
    }
    let mut tx = pool.begin().await?;
    PgTransactionContext::apply(&mut tx, context).await?;
    let invitation = sqlx::query("SELECT folioharbor.library_accept_invitation($1,$2,$3)")
        .bind(bob.as_uuid())
        .bind(invitation_hash.as_bytes().as_slice())
        .bind(now)
        .fetch_one(&mut *tx)
        .await;
    assert!(
        invitation.is_err(),
        "obsolete invitation helper unexpectedly remained API-executable"
    );
    tx.rollback().await?;
    Ok(())
}

async fn seed_invitation(
    pool: &PgPool,
    library: LibraryId,
    actor: UserId,
    email: &str,
    token_byte: u8,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO folioharbor.library_invitations(invitation_id,library_id,invited_by,normalized_email,display_email,role_code,token_hash,created_at,expires_at) VALUES($1,$2,$3,$4,$4,'reader',$5,$6,$7)")
        .bind(InvitationId::new().as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid())
        .bind(email).bind(TokenHash::from_bytes([token_byte; 32]).as_bytes().as_slice())
        .bind(now).bind(now + Duration::days(1)).execute(pool).await?;
    Ok(())
}

async fn matrix(reverse: bool) -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let alice = UserId::new();
    let bob = UserId::new();
    let unrelated = UserId::new();
    for (user, email) in [
        (alice, "alice@test"),
        (bob, "bob@test"),
        (unrelated, "other@test"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
            .bind(user.as_uuid()).bind(email).bind(now).execute(&pools.owner).await?;
    }
    let alice_library = LibraryId::new();
    let bob_library = LibraryId::new();
    for (library, user, role) in [
        (alice_library, alice, "owner"),
        (alice_library, bob, "reader"),
        (bob_library, bob, "editor"),
    ] {
        sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,$2,$3,$3) ON CONFLICT DO NOTHING")
            .bind(library.as_uuid()).bind("Library").bind(now).execute(&pools.owner).await?;
        sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,$3,'active',$4)")
            .bind(library.as_uuid()).bind(user.as_uuid()).bind(role).bind(now).execute(&pools.owner).await?;
    }
    seed_invitation(
        &pools.owner,
        alice_library,
        alice,
        "alice-invite@test",
        7,
        now,
    )
    .await?;
    seed_invitation(&pools.owner, bob_library, bob, "bob-invite@test", 8, now).await?;
    let stale_invitation_hash = TokenHash::from_bytes([9; 32]);
    seed_invitation(&pools.owner, bob_library, bob, "bob@test", 9, now).await?;

    let api_correct = DatabaseContext::api(alice, alice_library, RequestId::new());
    let api_wrong = DatabaseContext::api(alice, bob_library, RequestId::new());
    let worker_correct = DatabaseContext::worker(RequestId::new(), Some(alice_library));
    let worker_wrong = DatabaseContext::worker(RequestId::new(), Some(LibraryId::new()));
    let mut cases = vec![
        (&pools.api, None, [0, 0, 0]),
        (&pools.api, Some(&api_wrong), [0, 0, 0]),
        (&pools.api, Some(&api_correct), [1, 1, 1]),
        (&pools.worker, None, [0, 0, 0]),
        (&pools.worker, Some(&worker_wrong), [0, 0, 0]),
        (&pools.worker, Some(&worker_correct), [1, 2, 1]),
    ];
    if reverse {
        cases.reverse();
    }
    for (pool, context, expected) in cases {
        for (index, table) in TABLES.iter().enumerate() {
            assert_eq!(read_count(pool, context, table).await?, expected[index]);
            assert_write_denied(pool, context, table).await?;
        }
    }
    assert_eq!(
        read_count(&pools.api, Some(&worker_correct), "libraries").await?,
        0,
        "API database role impersonated worker RLS context"
    );
    let bob_version: i64 = sqlx::query_scalar(
        "SELECT version FROM folioharbor.library_memberships WHERE library_id=$1 AND user_id=$2 AND status='active'",
    )
    .bind(bob_library.as_uuid())
    .bind(bob.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_obsolete_library_helpers_cannot_execute(
        &pools.api,
        &api_correct,
        bob,
        bob_library,
        bob_version,
        &stale_invitation_hash,
        now,
    )
    .await?;

    for pool in [&pools.api, &pools.worker] {
        for table in TABLES {
            assert_cannot_disable_rls(pool, table).await?;
        }
        assert!(
            sqlx::query("UPDATE folioharbor.audit_events SET reason_code='tampered'")
                .execute(pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM folioharbor.audit_events")
                .execute(pool)
                .await
                .is_err()
        );
    }

    let raw_mutations = [
        "folioharbor.library_create_invitation(uuid,uuid,uuid,text,text,text,bytea,timestamptz,timestamptz)",
        "folioharbor.library_change_role(uuid,uuid,uuid,text,timestamptz)",
        "folioharbor.library_remove_member(uuid,uuid,uuid,timestamptz)",
        "folioharbor.library_update_settings(uuid,uuid,text,timestamptz)",
    ];
    let private_helpers = [
        "folioharbor.library_revalidate_grant(uuid,uuid,text,bigint)",
        "folioharbor.audit_record_allowed(uuid,uuid,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea,text,text,uuid)",
    ];
    assert_no_execute(&pools.api, &raw_mutations).await?;
    assert_no_execute(&pools.api, &private_helpers).await?;
    let mut worker_forbidden = vec![
        "folioharbor.library_provision_personal(uuid,uuid,timestamptz)",
        "folioharbor.library_accept_invitation(uuid,bytea,timestamptz)",
        "folioharbor.library_create_invitation_authorized(uuid,uuid,uuid,text,text,text,bytea,timestamptz,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea)",
        "folioharbor.library_change_role_authorized(uuid,uuid,uuid,text,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea)",
        "folioharbor.library_remove_member_authorized(uuid,uuid,uuid,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea)",
        "folioharbor.library_update_settings_authorized(uuid,uuid,text,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea)",
        "folioharbor.library_list_visible(uuid)",
        "folioharbor.library_get_visible(uuid,uuid,bigint)",
        "folioharbor.library_members_visible(uuid,uuid,bigint)",
    ];
    worker_forbidden.extend(raw_mutations);
    worker_forbidden.extend(private_helpers);
    assert_no_execute(&pools.worker, &worker_forbidden).await?;

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn rls_matrix_alice_first() -> anyhow::Result<()> {
    matrix(false).await
}

#[tokio::test]
async fn rls_matrix_unrelated_first() -> anyhow::Result<()> {
    matrix(true).await
}
