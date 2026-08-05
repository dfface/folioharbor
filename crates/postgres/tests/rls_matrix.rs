use folioharbor_domain::id::{LibraryId, RequestId, UserId};
use folioharbor_postgres::{DatabaseContext, PgPools, PgTransactionContext, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::PgPool;
use time::OffsetDateTime;

async fn read_count(pool: &PgPool, context: DatabaseContext, table: &str) -> anyhow::Result<i64> {
    let mut tx = pool.begin().await?;
    PgTransactionContext::apply(&mut tx, &context).await?;
    let query = format!("SELECT count(*) FROM folioharbor.{table}");
    let count = sqlx::query_scalar(&query).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(count)
}

#[allow(clippy::too_many_lines)]
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

    let no_context: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.libraries")
        .fetch_one(&pools.api)
        .await?;
    assert_eq!(no_context, 0);

    let contexts = if reverse {
        [(unrelated, bob_library), (alice, alice_library)]
    } else {
        [(alice, alice_library), (unrelated, bob_library)]
    };
    for (user, library) in contexts {
        let expected = i64::from(user == alice);
        assert_eq!(
            read_count(
                &pools.api,
                DatabaseContext::api(user, library, RequestId::new()),
                "libraries"
            )
            .await?,
            expected
        );
    }
    assert_eq!(
        read_count(
            &pools.api,
            DatabaseContext::api(alice, bob_library, RequestId::new()),
            "libraries"
        )
        .await?,
        0
    );
    assert_eq!(
        read_count(
            &pools.worker,
            DatabaseContext::worker(RequestId::new(), Some(alice_library)),
            "libraries"
        )
        .await?,
        1
    );
    assert_eq!(
        read_count(
            &pools.worker,
            DatabaseContext::worker(RequestId::new(), Some(bob_library)),
            "library_memberships"
        )
        .await?,
        1
    );

    assert!(
        sqlx::query("ALTER TABLE folioharbor.libraries DISABLE ROW LEVEL SECURITY")
            .execute(&pools.api)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE folioharbor.audit_events SET reason_code='tampered'")
            .execute(&pools.api)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM folioharbor.audit_events")
            .execute(&pools.worker)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("SELECT folioharbor.library_update_settings($1,$2,$3,$4)")
            .bind(alice.as_uuid())
            .bind(alice_library.as_uuid())
            .bind("Bypass")
            .bind(now)
            .execute(&pools.api)
            .await
            .is_err()
    );

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
