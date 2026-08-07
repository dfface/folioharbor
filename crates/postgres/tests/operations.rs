#![allow(clippy::expect_used)]

use folioharbor_application::operations::{
    BootstrapAdminOutcome, BootstrapAdminRepository as _, DatabaseHealth, HealthRepository as _,
    NewSystemAdministrator,
};
use folioharbor_domain::{id::UserId, identity::NormalizedEmail, time::OffsetDateTime};
use folioharbor_postgres::{PgOperationsRepository, connect_api, connect_owner, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use secrecy::SecretString;

#[tokio::test]
async fn bootstrap_is_transactional_idempotent_and_separate_from_library_roles() {
    let database = TestPostgres::provision().await.expect("test database");
    let owner_url = database.owner_url().expect("owner URL");
    let api_url = database.api_url().expect("api URL");
    let owner = connect_owner(&SecretString::from(owner_url))
        .await
        .expect("owner");
    run_migrations(&owner).await.expect("migrations");
    let repository = PgOperationsRepository::new(owner.clone());
    let now = OffsetDateTime::from_unix_timestamp(1_750_000_000).expect("time");

    let outcome = repository
        .bootstrap_admin(administrator(now))
        .await
        .expect("bootstrap");
    assert_eq!(outcome, BootstrapAdminOutcome::Created);
    let rerun = repository
        .bootstrap_admin(administrator(now))
        .await
        .expect("idempotent rerun");
    assert_eq!(rerun, BootstrapAdminOutcome::AlreadyAdministrator);

    let row: (String, i64, i64) = sqlx::query_as(
        "SELECT a.status, \
         (SELECT count(*) FROM folioharbor.system_administrators), \
         (SELECT count(*) FROM folioharbor.library_memberships m WHERE m.user_id=a.user_id) \
         FROM folioharbor.user_accounts a WHERE a.normalized_email='admin@example.com'",
    )
    .fetch_one(&owner)
    .await
    .expect("admin state");
    assert_eq!(row, ("verified".to_owned(), 1, 0));

    let api = connect_api(&SecretString::from(api_url))
        .await
        .expect("api");
    assert_eq!(
        PgOperationsRepository::new(api)
            .database_health()
            .await
            .expect("safe health"),
        DatabaseHealth {
            schema_version: 27,
            system_administrator_exists: true,
        }
    );

    owner.close().await;
    database.cleanup().await.expect("cleanup");
}

fn administrator(now: OffsetDateTime) -> NewSystemAdministrator {
    NewSystemAdministrator {
        user_id: UserId::new(),
        normalized_email: NormalizedEmail::parse("admin@example.com").expect("email"),
        display_email: "admin@example.com".to_owned(),
        password_hash: "$argon2id$v=19$m=19456,t=2,p=1$test$hash".to_owned(),
        created_at: now,
    }
}
