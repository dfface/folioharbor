#![allow(clippy::too_many_lines)]

use async_trait::async_trait;
use folioharbor_application::{
    error::AppError,
    libraries::{
        LibraryApi as _, LibraryService, ListLibrariesRequest, ReadLibraryRequest,
        UpdateSettingsRequest,
    },
    ports::{LibraryInvitationContext, MailError, Mailer},
};
use folioharbor_domain::{
    id::{LibraryId, RequestId, UserId},
    identity::NormalizedEmail,
};
use folioharbor_postgres::{
    PgAuditRepository, PgAuthorizationRepository, PgPools, libraries::PgLibraryRepository,
    run_migrations,
};
use folioharbor_test_support::{clock::FixedClock, postgres::TestPostgres, random::FixedRandom};
use secrecy::SecretString;
use time::OffsetDateTime;

#[derive(Clone, Copy)]
struct NoopMailer;

#[async_trait]
impl Mailer for NoopMailer {
    async fn send_verification(
        &self,
        _: &NormalizedEmail,
        _: SecretString,
    ) -> Result<(), MailError> {
        Ok(())
    }

    async fn send_password_reset(
        &self,
        _: &NormalizedEmail,
        _: SecretString,
    ) -> Result<(), MailError> {
        Ok(())
    }

    async fn send_library_invitation(
        &self,
        _: &NormalizedEmail,
        _: LibraryInvitationContext,
        _: SecretString,
    ) -> Result<(), MailError> {
        Ok(())
    }
}

#[tokio::test]
async fn facade_enforces_owner_editor_reader_and_unrelated_matrix_with_audit() -> anyhow::Result<()>
{
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let library = LibraryId::new();
    let owner = UserId::new();
    let editor = UserId::new();
    let reader = UserId::new();
    let unrelated = UserId::new();

    for (user, email) in [
        (owner, "owner@facade.test"),
        (editor, "editor@facade.test"),
        (reader, "reader@facade.test"),
        (unrelated, "unrelated@facade.test"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
            .bind(user.as_uuid()).bind(email).bind(now).execute(&pools.owner).await?;
    }
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Role Matrix',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    for (user, role) in [(owner, "owner"), (editor, "editor"), (reader, "reader")] {
        sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,$3,'active',$4)")
            .bind(library.as_uuid()).bind(user.as_uuid()).bind(role).bind(now).execute(&pools.owner).await?;
    }

    let service = LibraryService::new(
        PgLibraryRepository::new(pools.api.clone()),
        PgAuthorizationRepository::new(pools.api.clone()),
        PgAuditRepository::new(pools.api.clone()),
        NoopMailer,
        FixedClock::new(now),
        FixedRandom::new(9),
    );

    for actor in [owner, editor, reader] {
        let listed = service
            .list_libraries(ListLibrariesRequest {
                actor,
                request_id: RequestId::new(),
            })
            .await?;
        assert_eq!(listed.len(), 1);
        assert_eq!(
            service
                .get_library(ReadLibraryRequest {
                    actor,
                    request_id: RequestId::new(),
                    library_id: library,
                })
                .await?
                .library_id,
            library
        );
        assert_eq!(
            service
                .list_members(ReadLibraryRequest {
                    actor,
                    request_id: RequestId::new(),
                    library_id: library,
                })
                .await?
                .len(),
            3
        );
    }
    assert!(matches!(
        service
            .get_library(ReadLibraryRequest {
                actor: unrelated,
                request_id: RequestId::new(),
                library_id: library,
            })
            .await,
        Err(AppError::NotFound {
            code: "library_not_found"
        })
    ));

    service
        .update_settings(UpdateSettingsRequest {
            actor: owner,
            request_id: RequestId::new(),
            library_id: library,
            name: "Owner Updated".to_owned(),
        })
        .await?;
    for actor in [editor, reader] {
        assert!(matches!(
            service
                .update_settings(UpdateSettingsRequest {
                    actor,
                    request_id: RequestId::new(),
                    library_id: library,
                    name: "Denied".to_owned(),
                })
                .await,
            Err(AppError::Forbidden {
                code: "library_action_forbidden"
            })
        ));
    }

    let counts: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE decision='allowed'),count(*) FILTER (WHERE decision='denied') FROM folioharbor.audit_events",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(counts, (1, 3));
    let name: String =
        sqlx::query_scalar("SELECT name FROM folioharbor.libraries WHERE library_id=$1")
            .bind(library.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(name, "Owner Updated");

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
