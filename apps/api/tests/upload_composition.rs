#![allow(clippy::expect_used)]

use bytes::Bytes;
use folioharbor_api::{
    build_catalog_api, build_download, build_progress_api, build_reader_api, build_upload_api,
};
use folioharbor_application::{
    actor::Actor,
    catalog::PageRequest,
    config::{ConfigSources, Settings},
    imports::{CreateUploadRequest, ReceiveUploadRequest},
};
use folioharbor_domain::id::{ItemId, LibraryId, ManifestationId, RequestId, SessionId, UserId};
use folioharbor_domain::time::OffsetDateTime;
use folioharbor_postgres::{PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use std::{collections::BTreeMap, fs};

#[tokio::test]
async fn production_upload_composition_uses_postgres_and_local_blob_storage() -> anyhow::Result<()>
{
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let directory = tempfile::tempdir()?;
    let blob_root = directory.path().join("blobs");
    let staging_root = directory.path().join("staging");
    fs::create_dir_all(&blob_root)?;
    fs::create_dir_all(&staging_root)?;
    let settings = Settings::load(ConfigSources {
        environment: BTreeMap::from([
            (
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET".into(),
                "a-secret-value-with-at-least-32-bytes".into(),
            ),
            (
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET_KEY_ID".into(),
                "test-key".into(),
            ),
            (
                "FOLIOHARBOR_MAIL_SMTP_URL".into(),
                "smtp://mail.example:2525".into(),
            ),
            (
                "FOLIOHARBOR_STORAGE_ROOT".into(),
                blob_root.to_string_lossy().into_owned(),
            ),
            (
                "FOLIOHARBOR_STORAGE_STAGING_ROOT".into(),
                staging_root.to_string_lossy().into_owned(),
            ),
        ]),
        ..ConfigSources::default()
    })?;
    let now = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(actor.as_uuid()).bind("composition@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Composition',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)").bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    let api = build_upload_api(&settings, pools.api.clone());
    let upload = api
        .create_upload(CreateUploadRequest {
            actor,
            request_id: RequestId::new(),
            library_id: library,
            file_name: "production.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: 4,
        })
        .await?;
    let queued = api
        .receive_upload(ReceiveUploadRequest {
            actor,
            request_id: RequestId::new(),
            library_id: library,
            upload_id: upload.upload_id,
            bytes: Box::pin(futures_util::stream::iter([Ok(Bytes::from_static(
                b"book",
            ))])),
        })
        .await?;
    assert_eq!(queued.state.as_str(), "queued");
    let catalog = build_catalog_api(&settings, pools.api.clone());
    let page = catalog
        .list_library_books(actor, library, RequestId::new(), PageRequest::default())
        .await?;
    assert!(page.items.is_empty());
    let reader = build_reader_api(&settings, pools.api.clone());
    assert!(matches!(
        reader
            .get_manifest(actor, ItemId::new(), RequestId::new())
            .await,
        Err(folioharbor_application::error::AppError::NotFound {
            code: "item_not_found"
        })
    ));
    assert_download_composition(&settings, pools.api.clone(), actor).await;
    let progress = build_progress_api(pools.api.clone());
    assert!(matches!(
        progress
            .get_progress(actor, ManifestationId::new(), RequestId::new())
            .await,
        Err(folioharbor_application::error::AppError::NotFound {
            code: "manifestation_not_found"
        })
    ));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

async fn assert_download_composition(settings: &Settings, pool: sqlx::PgPool, actor: UserId) {
    let (download, _blobs) = build_download(settings, pool);
    let result = download
        .authorize(
            Actor {
                user_id: actor,
                session_id: SessionId::new(),
            },
            ItemId::new(),
            RequestId::new(),
        )
        .await;
    assert!(matches!(
        result,
        Err(folioharbor_application::error::AppError::NotFound {
            code: "item_not_found"
        })
    ));
}
