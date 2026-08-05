use folioharbor_application::{
    audit::{AuditDecision, AuditEvent, AuditSource},
    authorization::{Action, ResourceRef},
    ports::AuditSink,
};
use folioharbor_domain::id::{LibraryId, RequestId, UserId};
use folioharbor_postgres::{PgAuditRepository, PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use time::OffsetDateTime;

#[tokio::test]
async fn denial_audit_is_append_only_for_runtime_roles() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let actor = UserId::new();
    let library = LibraryId::new();
    let now = OffsetDateTime::now_utc();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'audit@test','audit@test','verified',$2,$2)")
        .bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Audit',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'reader','active',$3)")
        .bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    PgAuditRepository::new(pools.api.clone())
        .record_denial(AuditEvent {
            actor: Some(actor),
            effective_actor: Some(actor),
            library_id: library,
            action: Action::ManageLibrary,
            resource: ResourceRef::Library(library),
            decision: AuditDecision::Denied,
            reason_code: Some("library_action_forbidden"),
            request_id: RequestId::new(),
            source: AuditSource::Api,
            occurred_at: now,
            network_hmac: None,
        })
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.audit_events")
        .fetch_one(&pools.owner)
        .await?;
    assert_eq!(count, 1);
    assert!(
        sqlx::query("UPDATE folioharbor.audit_events SET reason_code='x'")
            .execute(&pools.api)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM folioharbor.audit_events")
            .execute(&pools.api)
            .await
            .is_err()
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
