use folioharbor_application::{
    audit::{AuditDecision, AuditEvent, AuditSource},
    authorization::{Action, ResourceRef},
    ports::AuditSink,
};
use folioharbor_domain::id::{LibraryId, RequestId, UserId};
use folioharbor_postgres::{
    DatabaseContext, PgAuditRepository, PgPools, PgTransactionContext, run_migrations,
};
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
    PgAuditRepository::new(pools.worker.clone())
        .record_denial(AuditEvent {
            actor: None,
            effective_actor: None,
            library_id: library,
            action: Action::ViewLibrary,
            resource: ResourceRef::Library(library),
            decision: AuditDecision::Denied,
            reason_code: Some("library_not_found"),
            request_id: RequestId::new(),
            source: AuditSource::Worker,
            occurred_at: now,
            network_hmac: None,
        })
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.audit_events")
        .fetch_one(&pools.owner)
        .await?;
    assert_eq!(count, 2);
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

#[tokio::test]
async fn denial_audit_rejects_direct_and_adversarial_inserts() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let actor = UserId::new();
    let other = UserId::new();
    let library = LibraryId::new();
    let now = OffsetDateTime::now_utc();
    let request = RequestId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'adversary@test','adversary@test','verified',$2,$2),($3,'other-audit@test','other-audit@test','verified',$2,$2)")
        .bind(actor.as_uuid()).bind(now).bind(other.as_uuid()).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Audit adversary',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;

    let mut tx = pools.api.begin().await?;
    PgTransactionContext::apply(&mut tx, &DatabaseContext::api(actor, library, request)).await?;
    assert!(
        sqlx::query("INSERT INTO folioharbor.audit_events(audit_event_id,actor_id,effective_actor_id,library_id,action_code,resource_type,resource_id,decision,reason_code,request_id,source,occurred_at) VALUES($1,$2,$2,$3,'library.manage','library',$3,'denied','library_action_forbidden',$4,'api',$5)")
            .bind(uuid::Uuid::now_v7()).bind(actor.as_uuid()).bind(library.as_uuid())
            .bind(request.as_ulid().to_string()).bind(now).execute(&mut *tx).await.is_err(),
        "runtime roles must not insert directly into the audit table"
    );
    tx.rollback().await?;

    for (action, reason, supplied_actor, effective) in [
        (
            "arbitrary.action",
            "library_action_forbidden",
            actor,
            Some(actor),
        ),
        ("library.manage", "arbitrary_reason", actor, Some(actor)),
        (
            "library.manage",
            "library_action_forbidden",
            other,
            Some(other),
        ),
        (
            "library.manage",
            "library_action_forbidden",
            actor,
            Some(other),
        ),
        ("library.manage", "library_action_forbidden", actor, None),
        (
            &"x".repeat(256),
            "library_action_forbidden",
            actor,
            Some(actor),
        ),
        ("library.manage", &"x".repeat(256), actor, Some(actor)),
    ] {
        let mut tx = pools.api.begin().await?;
        PgTransactionContext::apply(&mut tx, &DatabaseContext::api(actor, library, request))
            .await?;
        let result = sqlx::query("SELECT folioharbor.audit_record_denial($1,$2,$3,$4,$5,'library',$4,$6,$7,'api',$8,NULL)")
            .bind(uuid::Uuid::now_v7()).bind(supplied_actor.as_uuid()).bind(effective.map(UserId::as_uuid))
            .bind(library.as_uuid()).bind(action).bind(reason)
            .bind(request.as_ulid().to_string()).bind(now).execute(&mut *tx).await;
        assert!(
            result.is_err(),
            "accepted adversarial audit action={action:?} reason={reason:?}"
        );
        tx.rollback().await?;
    }

    let mut tx = pools.api.begin().await?;
    PgTransactionContext::apply(&mut tx, &DatabaseContext::worker(request, Some(library))).await?;
    let spoofed_worker = sqlx::query("SELECT folioharbor.audit_record_denial($1,NULL,NULL,$2,'library.manage','library',$2,'library_action_forbidden',$3,'worker',$4,NULL)")
        .bind(uuid::Uuid::now_v7()).bind(library.as_uuid())
        .bind(request.as_ulid().to_string()).bind(now).execute(&mut *tx).await;
    assert!(
        spoofed_worker.is_err(),
        "API database role impersonated the worker audit source"
    );
    tx.rollback().await?;

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
