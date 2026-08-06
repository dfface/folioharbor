use std::{borrow::Cow, path::Path};

use folioharbor_postgres::{PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::{Row, migrate::Migrator};
use time::OffsetDateTime;
use uuid::Uuid;

async fn run_through_version(pool: &sqlx::PgPool, version: i64) -> anyhow::Result<()> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    let all = Migrator::new(source.as_path()).await?;
    let selected = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= version)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    selected.run(pool).await?;
    Ok(())
}

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
    assert_eq!(
        first_versions,
        vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21
        ]
    );

    run_migrations(&pools.owner).await?;
    let second_versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&pools.owner)
            .await?;
    assert_eq!(
        second_versions,
        vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21
        ]
    );

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn quota_boundary_migration_backfills_and_rebounds_old_writer_rows() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_through_version(&pools.owner, 18).await?;

    let now = OffsetDateTime::now_utc();
    let user = Uuid::now_v7();
    let device = Uuid::now_v7();
    let manifestation = Uuid::now_v7();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'upgrade@test.invalid','upgrade@test.invalid','verified',$2,$2)")
        .bind(user)
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.user_devices(device_id,user_id,display_name,created_at,last_seen_at) VALUES($1,$2,'upgrade',$3,$3)")
        .bind(device)
        .bind(user)
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.manifestations(manifestation_id,identifiers,created_at) VALUES($1,ARRAY[]::text[],$2)")
        .bind(manifestation)
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query(
        "WITH payload AS (SELECT string_agg(md5(chunk::text),'') AS text FROM generate_series(1,256) chunk) INSERT INTO folioharbor.reading_mutations(user_id,client_mutation_id,manifestation_id,device_id,outcome,global_version,device_locator,device_updated_at,created_at,request_fingerprint) SELECT $1,md5(series::text)::uuid,$2,$3,'conflict',0,jsonb_build_object('href',payload.text),$4,$4,decode(repeat('00',32),'hex') FROM generate_series(1,10001) series CROSS JOIN payload",
    )
    .bind(user)
    .bind(manifestation)
    .bind(device)
    .bind(now)
    .execute(&pools.owner)
    .await?;
    let usage_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.reading_mutation_usage WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(usage_before, 0, "old direct writers can lack a usage row");

    run_migrations(&pools.owner).await?;

    let actual: (i64, i64) = sqlx::query_as(
        "SELECT count(*)::bigint,COALESCE(sum(pg_column_size(m)+COALESCE(pg_column_size(global_locator),0)+pg_column_size(device_locator)),0)::bigint FROM folioharbor.reading_mutations m WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(&pools.owner)
    .await?;
    let recorded: (i64, i64) = sqlx::query_as(
        "SELECT live_count,live_bytes FROM folioharbor.reading_mutation_usage WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(recorded, actual);
    assert!(actual.0 <= 10_000);
    assert!(actual.1 <= 64_i64 * 1_024 * 1_024);
    assert!(
        actual.0 < 10_000,
        "byte rebound must also trim this fixture"
    );

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
