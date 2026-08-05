use folioharbor_application::{
    audit::AuditEvent,
    authorization::{Action, Authorization, ResourceRef},
    ports::{LibraryRepository as _, NewLibraryInvitation},
};
use folioharbor_domain::{
    id::{InvitationId, LibraryId, RequestId, UserId},
    identity::{NormalizedEmail, TokenHash},
    libraries::role::RoleCode,
};
use folioharbor_postgres::libraries::PgLibraryRepository;
use folioharbor_postgres::{PgAuthorizationRepository, PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use time::{Duration, OffsetDateTime};

#[tokio::test]
async fn invitation_and_allowed_audit_commit_atomically_after_locked_revalidation()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'owner@test','owner@test','verified',$2,$2)").bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Library',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'owner','active',$3)").bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    let invitation = InvitationId::new();
    let resource = ResourceRef::Invitation {
        library_id: library,
        invitation_id: invitation,
    };
    let authorization = PgAuthorizationRepository::new(pools.api.clone());
    let grant = Authorization::new(&authorization)
        .require(actor, Action::InviteMember, resource)
        .await?;
    let request = RequestId::new();
    let outcome = PgLibraryRepository::new(pools.api.clone())
        .create_invitation(
            NewLibraryInvitation {
                invitation_id: invitation,
                library_id: library,
                invited_by: actor,
                normalized_email: NormalizedEmail::parse("reader@example.com")?,
                display_email: "reader@example.com".to_owned(),
                role: RoleCode::Reader,
                token_hash: TokenHash::from_bytes([7; 32]),
                created_at: now,
                expires_at: now + Duration::days(7),
            },
            grant,
            AuditEvent::allowed(actor, Action::InviteMember, resource, request, now),
        )
        .await?;
    assert_eq!(
        outcome,
        folioharbor_application::ports::LibraryMutationOutcome::Applied
    );
    let pair:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM folioharbor.library_invitations),(SELECT count(*) FROM folioharbor.audit_events)").fetch_one(&pools.owner).await?;
    assert_eq!(pair, (1, 1));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
