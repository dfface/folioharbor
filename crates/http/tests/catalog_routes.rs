#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode, header::ETAG},
};
use folioharbor_application::{
    actor::Actor,
    catalog::{BookSummary, CatalogApi, CatalogService, ItemDetail, Page, PageRequest},
    error::AppError,
    identity::{
        AuthenticateSessionCommand, AuthenticateSessionUseCase, AuthenticatedSession,
        CompletePasswordResetCommand, CompletePasswordResetUseCase, CurrentSessionUseCase,
        IssuedSession, ListSessionsUseCase, LoginCommand, LoginUseCase, LogoutCommand,
        LogoutUseCase, PasswordResetComplete, PasswordResetRequested, PendingAccount,
        RegisterAccountCommand, RegisterAccountUseCase, RequestPasswordResetCommand,
        RequestPasswordResetUseCase, RevokeSessionCommand, RevokeSessionOutcome,
        RevokeSessionUseCase, SafeSession, VerifiedAccount, VerifyEmailCommand, VerifyEmailUseCase,
    },
    rate_limit::{CheckRateLimit, RateLimitDecision, RateLimitUseCase},
};
use folioharbor_domain::{
    id::{
        BlobId, ExpressionId, HoldingId, ItemId, LibraryId, ManifestationId, PublicationPackageId,
        RequestId, SessionId, UploadId, UserId, WorkId,
    },
    identity::CsrfToken,
};
use folioharbor_http::{AppState, router};
use folioharbor_postgres::{
    PgAuthorizationRepository, PgCatalogRepository, PgPools, run_migrations,
};
use folioharbor_test_support::postgres::TestPostgres;
use http_body_util::BodyExt as _;
use secrecy::{ExposeSecret as _, SecretString};
use serde_yaml::Value;
use time::OffsetDateTime;
use tower::ServiceExt as _;
use url::Url;

struct Identity(HashMap<String, UserId>);
fn unused<T>() -> Result<T, AppError> {
    unreachable!("unused identity endpoint")
}

#[async_trait]
impl AuthenticateSessionUseCase for Identity {
    async fn authenticate_session(
        &self,
        command: AuthenticateSessionCommand,
    ) -> Result<Option<AuthenticatedSession>, AppError> {
        Ok(self
            .0
            .get(command.session_token.expose_secret())
            .copied()
            .map(|user_id| AuthenticatedSession {
                actor: Actor {
                    user_id,
                    session_id: SessionId::new(),
                },
                csrf_token_hash: CsrfToken::parse(SecretString::from("catalog-csrf".to_owned()))
                    .hash_for_storage(),
            }))
    }
}
#[async_trait]
impl RegisterAccountUseCase for Identity {
    async fn register(&self, _: RegisterAccountCommand) -> Result<PendingAccount, AppError> {
        unused()
    }
}
#[async_trait]
impl VerifyEmailUseCase for Identity {
    async fn verify_email(&self, _: VerifyEmailCommand) -> Result<VerifiedAccount, AppError> {
        unused()
    }
}
#[async_trait]
impl LoginUseCase for Identity {
    async fn login(&self, _: LoginCommand) -> Result<IssuedSession, AppError> {
        unused()
    }
}
#[async_trait]
impl LogoutUseCase for Identity {
    async fn logout(&self, _: LogoutCommand) -> Result<(), AppError> {
        unused()
    }
}
#[async_trait]
impl RequestPasswordResetUseCase for Identity {
    async fn request_password_reset(
        &self,
        _: RequestPasswordResetCommand,
    ) -> Result<PasswordResetRequested, AppError> {
        unused()
    }
}
#[async_trait]
impl CompletePasswordResetUseCase for Identity {
    async fn complete_password_reset(
        &self,
        _: CompletePasswordResetCommand,
    ) -> Result<PasswordResetComplete, AppError> {
        unused()
    }
}
#[async_trait]
impl CurrentSessionUseCase for Identity {
    async fn current_session(&self, _: Actor) -> Result<SafeSession, AppError> {
        unused()
    }
}
#[async_trait]
impl ListSessionsUseCase for Identity {
    async fn list_sessions(&self, _: Actor) -> Result<Vec<SafeSession>, AppError> {
        unused()
    }
}
#[async_trait]
impl RevokeSessionUseCase for Identity {
    async fn revoke_session(
        &self,
        _: RevokeSessionCommand,
    ) -> Result<RevokeSessionOutcome, AppError> {
        unused()
    }
}
#[async_trait]
impl RateLimitUseCase for Identity {
    async fn check_rate_limit(&self, _: CheckRateLimit) -> Result<RateLimitDecision, AppError> {
        unused()
    }
}

struct FakeCatalog {
    allowed: UserId,
    item: ItemId,
}

#[async_trait]
impl CatalogApi for FakeCatalog {
    async fn list_library_books(
        &self,
        actor: UserId,
        _: LibraryId,
        _: RequestId,
        _: PageRequest,
    ) -> Result<Page<BookSummary>, AppError> {
        if actor != self.allowed {
            return Err(AppError::NotFound {
                code: "library_not_found",
            });
        }
        Ok(Page {
            items: vec![BookSummary {
                item_id: self.item,
                primary_title: "The Book".to_owned(),
                authors: vec!["Writer".to_owned()],
                languages: vec!["en".to_owned()],
                media_type: "application/epub+zip".to_owned(),
                can_read: true,
                can_download: false,
            }],
            next_cursor: Some("opaqueCursorValue1234".to_owned()),
        })
    }
    async fn get_item(
        &self,
        actor: UserId,
        _: LibraryId,
        _: ItemId,
        _: RequestId,
    ) -> Result<ItemDetail, AppError> {
        if actor != self.allowed {
            return Err(AppError::NotFound {
                code: "library_not_found",
            });
        }
        Ok(ItemDetail {
            item_id: self.item,
            manifestation_id: ManifestationId::new(),
            primary_title: "The Book".to_owned(),
            authors: vec!["Writer".to_owned()],
            languages: vec!["en".to_owned()],
            identifiers: vec!["isbn:test".to_owned()],
            media_type: "application/epub+zip".to_owned(),
            can_read: true,
            can_download: false,
            etag: "W/\"catalog-test\"".to_owned(),
        })
    }
}

fn app() -> (axum::Router, LibraryId, ItemId) {
    let allowed = UserId::new();
    let outsider = UserId::new();
    let item = ItemId::new();
    let identity = Arc::new(Identity(HashMap::from([
        ("allowed".to_owned(), allowed),
        ("outsider".to_owned(), outsider),
    ])));
    let state = AppState::new(
        Url::parse("https://library.example").expect("url"),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity,
    )
    .with_catalog_api(Arc::new(FakeCatalog { allowed, item }));
    (router(state), LibraryId::new(), item)
}

fn request(uri: &str, actor: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("Cookie", format!("folioharbor_session={actor}"))
        .body(Body::empty())
        .expect("request")
}

fn state(identity: Arc<Identity>, catalog: Arc<dyn CatalogApi>) -> AppState {
    AppState::new(
        Url::parse("https://library.example").expect("url"),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity,
    )
    .with_catalog_api(catalog)
}

#[tokio::test]
async fn routes_return_safe_catalog_projections_etag_and_anti_enumerating_problem() {
    let (app, library, item) = app();
    let books = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/libraries/{}/books?limit=1", library.as_uuid()),
            "allowed",
        ))
        .await
        .expect("response");
    assert_eq!(books.status(), StatusCode::OK);
    let body = books.into_body().collect().await.expect("body").to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(value["items"][0]["primary_title"], "The Book");
    assert_eq!(value["items"][0]["can_download"], false);
    let serialized = String::from_utf8_lossy(&body);
    for secret in [
        "blob",
        "package_id",
        "storage_key",
        "local_path",
        "audit_id",
    ] {
        assert!(!serialized.contains(secret), "leaked {secret}");
    }

    let detail = app
        .clone()
        .oneshot(request(
            &format!(
                "/api/v1/libraries/{}/items/{}",
                library.as_uuid(),
                item.as_uuid()
            ),
            "allowed",
        ))
        .await
        .expect("response");
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(
        detail
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok()),
        Some("W/\"catalog-test\"")
    );

    let hidden = app
        .oneshot(request(
            &format!("/api/v1/libraries/{}/books", library.as_uuid()),
            "outsider",
        ))
        .await
        .expect("response");
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let hidden_body = hidden.into_body().collect().await.expect("body").to_bytes();
    let hidden_json: serde_json::Value = serde_json::from_slice(&hidden_body).expect("problem");
    assert_eq!(hidden_json["code"], "library_not_found");
    assert!(hidden_json.get("total").is_none());
}

#[test]
fn openapi_documents_opaque_pagination_capabilities_etag_and_problems() {
    let document: Value =
        serde_yaml::from_str(include_str!("../../../openapi/folioharbor-v1.yaml"))
            .expect("OpenAPI");
    let list = &document["paths"]["/api/v1/libraries/{library_id}/books"]["get"];
    let detail = &document["paths"]["/api/v1/libraries/{library_id}/items/{item_id}"]["get"];
    assert!(
        list["description"]
            .as_str()
            .is_some_and(|text| text.contains("opaque"))
    );
    assert!(detail["responses"]["200"]["headers"]["ETag"].is_mapping());
    for operation in [list, detail] {
        for status in ["401", "404", "503"] {
            assert!(
                operation["responses"][status].is_mapping(),
                "missing {status}"
            );
        }
    }
    for field in ["can_read", "can_download"] {
        assert!(document["components"]["schemas"]["BookSummary"]["properties"][field].is_mapping());
    }
}

#[tokio::test]
async fn real_routes_apply_role_and_reader_download_setting_without_enumerating_outsiders()
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
    let library = LibraryId::new();
    let owner = UserId::new();
    let editor = UserId::new();
    let reader = UserId::new();
    let outsider = UserId::new();
    for (user, email) in [
        (owner, "owner@catalog.test"),
        (editor, "editor@catalog.test"),
        (reader, "reader@catalog.test"),
        (outsider, "outsider@catalog.test"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(user.as_uuid()).bind(email).bind(now).execute(&pools.owner).await?;
    }
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Catalog',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    for (user, role) in [(owner, "owner"), (editor, "editor"), (reader, "reader")] {
        sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,$3,'active',$4)").bind(library.as_uuid()).bind(user.as_uuid()).bind(role).bind(now).execute(&pools.owner).await?;
    }
    seed_catalog_item(&pools.owner, library, owner, now).await?;
    let identity = Arc::new(Identity(HashMap::from([
        ("owner".to_owned(), owner),
        ("editor".to_owned(), editor),
        ("reader".to_owned(), reader),
        ("outsider".to_owned(), outsider),
    ])));
    let uri = format!("/api/v1/libraries/{}/books", library.as_uuid());
    let disabled = router(state(
        identity.clone(),
        Arc::new(CatalogService::new(
            PgCatalogRepository::new(pools.api.clone()),
            PgAuthorizationRepository::new(pools.api.clone()),
            false,
        )),
    ));
    for (actor, expected_download) in [("owner", true), ("editor", true), ("reader", false)] {
        let response = disabled.clone().oneshot(request(&uri, actor)).await?;
        assert_eq!(response.status(), StatusCode::OK, "view as {actor}");
        let json: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
        assert_eq!(json["items"][0]["can_read"], true);
        assert_eq!(json["items"][0]["can_download"], expected_download);
    }
    let hidden = disabled.oneshot(request(&uri, "outsider")).await?;
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let enabled = router(state(
        identity,
        Arc::new(CatalogService::new(
            PgCatalogRepository::new(pools.api.clone()),
            PgAuthorizationRepository::new(pools.api.clone()),
            true,
        )),
    ));
    let response = enabled.oneshot(request(&uri, "reader")).await?;
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    assert_eq!(json["items"][0]["can_download"], true);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

async fn seed_catalog_item(
    pool: &sqlx::PgPool,
    library: LibraryId,
    actor: UserId,
    now: OffsetDateTime,
) -> anyhow::Result<ItemId> {
    let work = WorkId::new();
    let expression = ExpressionId::new();
    let manifestation = ManifestationId::new();
    let blob = BlobId::new();
    let package = PublicationPackageId::new();
    let holding = HoldingId::new();
    let item = ItemId::new();
    let upload = UploadId::new();
    let sha = vec![42_u8; 32];
    sqlx::query("INSERT INTO folioharbor.works VALUES($1,'Visible catalog book',ARRAY['Writer']::text[],$2)").bind(work.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.expressions VALUES($1,$2,ARRAY['en']::text[],$3)")
        .bind(expression.as_uuid())
        .bind(work.as_uuid())
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO folioharbor.manifestations VALUES($1,ARRAY['isbn:test']::text[],$2)")
        .bind(manifestation.as_uuid())
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO folioharbor.manifestation_expressions VALUES($1,$2,0)")
        .bind(manifestation.as_uuid())
        .bind(expression.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,1,$3)").bind(blob.as_uuid()).bind(&sha).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,received_bytes,state,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'book.epub','application/epub+zip',1,'instance',1,'ready',$4,$5,$6,$6,$6)").bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid()).bind(format!("blob:instance-v1:{}:1","2a".repeat(32))).bind(&sha).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages VALUES($1,$2,$3,'epub-v1',$4)")
        .bind(package.as_uuid())
        .bind(manifestation.as_uuid())
        .bind(blob.as_uuid())
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)").bind(holding.as_uuid()).bind(library.as_uuid()).bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)").bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(upload.as_uuid()).bind(now).execute(pool).await?;
    Ok(item)
}
