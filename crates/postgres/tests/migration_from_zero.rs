use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use folioharbor_postgres::{PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::{Row, migrate::Migrator};
use time::OffsetDateTime;
use uuid::Uuid;

const TASK_23_BASE: &str = "2359f37ab3007791348acf3e243d9a24bae0c0c7";

struct BaseMigrationTree(PathBuf);

impl BaseMigrationTree {
    fn from_committed_task_base() -> anyhow::Result<Self> {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let root = std::env::temp_dir().join(format!("folioharbor-task23-base-{}", Uuid::now_v7()));
        fs::create_dir(&root)?;

        let listing = Command::new("git")
            .args(["ls-tree", "-r", "--name-only", TASK_23_BASE, "migrations"])
            .current_dir(&repository)
            .output()?;
        anyhow::ensure!(
            listing.status.success(),
            "could not read the committed Task 23 base tree"
        );

        for relative in String::from_utf8(listing.stdout)?.lines() {
            if !Path::new(relative)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("sql"))
            {
                continue;
            }
            let contents = Command::new("git")
                .args(["show", &format!("{TASK_23_BASE}:{relative}")])
                .current_dir(&repository)
                .output()?;
            anyhow::ensure!(
                contents.status.success(),
                "could not read a committed Task 23 base migration"
            );
            let destination = root.join(Path::new(relative).file_name().ok_or_else(|| {
                anyhow::anyhow!("committed migration path did not have a file name")
            })?);
            fs::write(destination, contents.stdout)?;
        }
        Ok(Self(root))
    }
}

impl Drop for BaseMigrationTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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

    let migration_started_at = OffsetDateTime::now_utc();
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

    let migrated_after = OffsetDateTime::now_utc();
    let metadata: Vec<(bool, i64, OffsetDateTime)> = sqlx::query_as(
        "SELECT singleton,schema_version,applied_at FROM folioharbor.schema_metadata",
    )
    .fetch_all(&pools.owner)
    .await?;
    assert_eq!(metadata.len(), 1);
    assert!(metadata[0].0);
    assert_eq!(metadata[0].1, 28);
    assert!(metadata[0].2 >= migration_started_at);
    assert!(metadata[0].2 <= migrated_after);

    let final_columns: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns \
         WHERE table_schema='folioharbor' AND table_name='background_jobs' \
           AND column_name IN ('origin_request_id','origin_traceparent')",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(final_columns, 2);
    let reader_permissions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.role_permissions \
         WHERE role_code='reader' AND permission_code IN ('holding.view','item.read')",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(reader_permissions, 2);

    let first_versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&pools.owner)
            .await?;
    assert_eq!(
        first_versions,
        vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28
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
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28
        ]
    );

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn committed_task_base_upgrades_without_sqlx_checksum_drift() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    let base_tree = BaseMigrationTree::from_committed_task_base()?;
    let base = Migrator::new(base_tree.0.clone()).await?;
    base.run(&pools.owner).await?;

    let base_checksums: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT version,checksum FROM _sqlx_migrations WHERE version IN (10,26) ORDER BY version",
    )
    .fetch_all(&pools.owner)
    .await?;
    assert_eq!(base_checksums.len(), 2);

    let report = run_migrations(&pools.owner).await?;
    assert_eq!(report.versions.last(), Some(&28));

    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    let current = Migrator::new(source.as_path()).await?;
    for (version, committed_checksum) in base_checksums {
        let current_checksum = current
            .iter()
            .find(|migration| migration.version == version)
            .ok_or_else(|| anyhow::anyhow!("current migration set omitted a base version"))?
            .checksum
            .as_ref();
        assert_eq!(committed_checksum.as_slice(), current_checksum);
    }

    let schema_version: i64 = sqlx::query_scalar(
        "SELECT schema_version FROM folioharbor.schema_metadata WHERE singleton",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(schema_version, 28);

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn runtime_roles_reject_other_role_credentials() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;

    let api_with_worker_secret =
        database
            .worker_url()?
            .replacen("folioharbor_worker", "folioharbor_api", 1);
    let api_probe = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(&api_with_worker_secret)
        .await;
    assert!(
        api_probe.is_err(),
        "API authentication accepted the Worker credential"
    );

    let worker_with_owner_secret =
        database
            .owner_url()?
            .replacen("folioharbor_owner", "folioharbor_worker", 1);
    let worker_probe = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(&worker_with_owner_secret)
        .await;
    assert!(
        worker_probe.is_err(),
        "Worker authentication accepted the owner credential"
    );

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
