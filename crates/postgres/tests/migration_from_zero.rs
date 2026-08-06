use folioharbor_postgres::{PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::Row;

#[tokio::test]
async fn migrations_from_zero_preserve_least_privilege_roles_and_are_idempotent()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;

    run_migrations(&pools.owner).await?;

    let owner: String = sqlx::query_scalar(
        "SELECT schema_owner FROM information_schema.schemata WHERE schema_name = 'folioharbor'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(owner, "folioharbor_owner");

    for role in ["folioharbor_api", "folioharbor_worker"] {
        let attributes = sqlx::query(
            "SELECT rolname, rolcanlogin, rolinherit, rolsuper, rolcreatedb, rolcreaterole, rolbypassrls \
             FROM pg_roles WHERE rolname = $1",
        )
        .bind(role)
        .fetch_one(&pools.owner)
        .await?;
        assert_eq!(attributes.get::<String, _>("rolname"), role);
        assert!(attributes.get::<bool, _>("rolcanlogin"));
        assert!(!attributes.get::<bool, _>("rolinherit"));
        assert!(!attributes.get::<bool, _>("rolsuper"));
        assert!(!attributes.get::<bool, _>("rolcreatedb"));
        assert!(!attributes.get::<bool, _>("rolcreaterole"));
        assert!(!attributes.get::<bool, _>("rolbypassrls"));

        let owned_objects: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ( \
                 SELECT n.nspowner AS owner FROM pg_namespace n WHERE n.nspname = 'folioharbor' \
                 UNION ALL \
                 SELECT c.relowner FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                    WHERE n.nspname = 'folioharbor' \
                 UNION ALL \
                 SELECT p.proowner FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
                    WHERE n.nspname = 'folioharbor' \
             ) objects JOIN pg_roles owners ON owners.oid = objects.owner \
             WHERE owners.rolname = $1",
        )
        .bind(role)
        .fetch_one(&pools.owner)
        .await?;
        assert_eq!(owned_objects, 0);
    }

    let metadata_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.schema_metadata")
        .fetch_one(&pools.owner)
        .await?;
    assert_eq!(metadata_rows, 1);

    let first_versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&pools.owner)
            .await?;
    assert_eq!(first_versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);

    run_migrations(&pools.owner).await?;
    let second_versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&pools.owner)
            .await?;
    assert_eq!(second_versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
