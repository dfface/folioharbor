#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{
        Request, StatusCode,
        header::{CONTENT_TYPE, ETAG},
    },
};
use folioharbor_application::{
    actor::Actor,
    catalog::{
        BookSummary, CatalogApi, CatalogService, DownloadService, ItemDetail, Page, PageRequest,
    },
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
    libraries::LibraryService,
    mail::{MailIntentSealer, MailMessage, MailOutboxError},
    ports::NewMailOutboxEntry,
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
    PgAuditRepository, PgAuthorizationRepository, PgCatalogRepository, PgDownloadRepository,
    PgPools, libraries::PgLibraryRepository, run_migrations,
};
use folioharbor_storage_local::LocalBlobStore;
use folioharbor_test_support::{clock::FixedClock, postgres::TestPostgres, random::FixedRandom};
use http_body_util::BodyExt as _;
use secrecy::{ExposeSecret as _, SecretString};
use serde_yaml::Value;
use time::OffsetDateTime;
use tower::ServiceExt as _;
use url::Url;

struct Identity(HashMap<String, UserId>);
#[derive(Clone, Copy)]
struct NoopSealer;

impl MailIntentSealer for NoopSealer {
    fn seal(
        &self,
        message: MailMessage,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> Result<NewMailOutboxEntry, MailOutboxError> {
        Ok(NewMailOutboxEntry {
            mail_id: message.mail_id(),
            recipient_account_id: message.recipient_account_id(),
            delivery_address: message.recipient().as_str().to_owned(),
            template_code: message.template().code(),
            template_version: 1,
            locale: message.locale().as_str(),
            token_ciphertext: vec![1],
            encryption_key_id: "test-key".to_owned(),
            nonce: vec![0; 12],
            idempotency_key: message.idempotency_key(),
            invitation_library_id: message.invitation_library_id(),
            invitation_role: message.invitation_role().map(str::to_owned),
            next_run_at: now,
            expires_at,
        })
    }
}
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

fn production_state(
    identity: Arc<Identity>,
    pools: &PgPools,
    now: OffsetDateTime,
    blob_root: &std::path::Path,
) -> AppState {
    state(
        identity,
        Arc::new(
            CatalogService::new(
                PgCatalogRepository::new(pools.api.clone()),
                PgAuthorizationRepository::new(pools.api.clone()),
            )
            .with_lifecycle(),
        ),
    )
    .with_library_api(Arc::new(LibraryService::new(
        PgLibraryRepository::new(pools.api.clone()),
        PgAuthorizationRepository::new(pools.api.clone()),
        PgAuditRepository::new(pools.api.clone()),
        NoopSealer,
        FixedClock::new(now),
        FixedRandom::new(16),
    )))
    .with_download(
        Arc::new(DownloadService::new(PgDownloadRepository::new(
            pools.api.clone(),
        ))),
        Arc::new(LocalBlobStore::new(blob_root)),
    )
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
        .clone()
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

    let malformed_identifier = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/libraries/{}/items/not-a-uuid", library.as_uuid()),
            "allowed",
        ))
        .await
        .expect("response");
    assert_eq!(malformed_identifier.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        malformed_identifier
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json")
    );
    let identifier_problem: serde_json::Value = serde_json::from_slice(
        &malformed_identifier
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("problem JSON");
    assert_eq!(identifier_problem["code"], "invalid_identifier");
    let request_id = identifier_problem["request_id"]
        .as_str()
        .expect("request ID");
    assert_eq!(
        identifier_problem["instance"],
        format!("/problems/{request_id}")
    );

    let malformed = app
        .oneshot(request(
            &format!("/api/v1/libraries/{}/books?limit=abc", library.as_uuid()),
            "allowed",
        ))
        .await
        .expect("response");
    assert_eq!(malformed.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        malformed
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json")
    );
    let problem: serde_json::Value = serde_json::from_slice(
        &malformed
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("problem JSON");
    assert_eq!(problem["code"], "invalid_page");
    let request_id = problem["request_id"].as_str().expect("request ID");
    assert_eq!(problem["instance"], format!("/problems/{request_id}"));
}

#[test]
fn openapi_documents_opaque_pagination_capabilities_etag_and_problems() {
    let document: Value =
        serde_yaml::from_str(include_str!("../../../openapi/folioharbor-v1.yaml"))
            .expect("OpenAPI");
    let list = &document["paths"]["/api/v1/libraries/{library_id}/books"]["get"];
    let detail = &document["paths"]["/api/v1/libraries/{library_id}/items/{item_id}"]["get"];
    let delete = &document["paths"]["/api/v1/libraries/{library_id}/items/{item_id}"]["delete"];
    let restore =
        &document["paths"]["/api/v1/libraries/{library_id}/items/{item_id}/restore"]["post"];
    assert!(
        list["description"]
            .as_str()
            .is_some_and(|text| text.contains("opaque"))
    );
    assert!(detail["responses"]["200"]["headers"]["ETag"].is_mapping());
    for operation in [list, detail] {
        assert_eq!(
            operation["responses"]["400"]["$ref"],
            "#/components/responses/InvalidIdentifierProblem"
        );
    }
    assert_eq!(
        document["components"]["responses"]["InvalidIdentifierProblem"]["content"]["application/problem+json"]
            ["example"]["code"],
        "invalid_identifier"
    );
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
    assert_eq!(delete["operationId"], "deleteLibraryItem");
    assert_eq!(restore["operationId"], "restoreLibraryItem");
    assert_eq!(
        delete["responses"]["204"]["description"],
        "Item is deleted or was already deleted"
    );
    assert!(restore["responses"]["409"].is_mapping());
}

#[test]
fn openapi_item_detail_schema_accepts_the_actual_response_contract() {
    let document: Value =
        serde_yaml::from_str(include_str!("../../../openapi/folioharbor-v1.yaml"))
            .expect("OpenAPI");
    let actual = serde_json::json!({
        "item_id": ItemId::new().as_uuid().to_string(),
        "manifestation_id": ManifestationId::new().as_uuid().to_string(),
        "primary_title": "The Book",
        "authors": ["Writer"],
        "languages": ["en"],
        "identifiers": ["isbn:test"],
        "media_type": "application/epub+zip",
        "can_read": true,
        "can_download": false
    });
    assert_schema_accepts(
        &document["components"]["schemas"]["ItemDetail"],
        &actual,
        &document,
    );
}

fn assert_schema_accepts(schema: &Value, actual: &serde_json::Value, document: &Value) {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/components/schemas/")
            .expect("local schema reference");
        assert_schema_accepts(&document["components"]["schemas"][name], actual, document);
        return;
    }
    if let Some(parts) = schema.get("allOf").and_then(Value::as_sequence) {
        for part in parts {
            assert_schema_accepts(part, actual, document);
        }
        return;
    }
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return;
    }
    let object = actual.as_object().expect("object response");
    let properties = schema["properties"].as_mapping().expect("properties");
    for required in schema["required"].as_sequence().into_iter().flatten() {
        let name = required.as_str().expect("required property name");
        assert!(
            object.contains_key(name),
            "missing required property {name}"
        );
    }
    if schema["additionalProperties"].as_bool() == Some(false) {
        for name in object.keys() {
            assert!(
                properties.contains_key(Value::String(name.clone())),
                "schema rejects actual property {name}"
            );
        }
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
    let item = seed_catalog_item(&pools.owner, library, owner, now).await?;
    let identity = Arc::new(Identity(HashMap::from([
        ("owner".to_owned(), owner),
        ("editor".to_owned(), editor),
        ("reader".to_owned(), reader),
        ("outsider".to_owned(), outsider),
    ])));
    let blob_root = tempfile::tempdir()?;
    let hash = "2a".repeat(32);
    let blob_directory = blob_root.path().join("objects/instance-v1/2a/2a");
    std::fs::create_dir_all(&blob_directory)?;
    std::fs::write(blob_directory.join(format!("{hash}-16")), [42_u8; 16])?;
    let app = router(production_state(identity, &pools, now, blob_root.path()));
    let uri = format!("/api/v1/libraries/{}/books", library.as_uuid());
    let download_uri = format!("/api/v1/items/{}/download", item.as_uuid());
    for (actor, expected_download) in [("owner", true), ("editor", true), ("reader", false)] {
        let response = app.clone().oneshot(request(&uri, actor)).await?;
        assert_eq!(response.status(), StatusCode::OK, "view as {actor}");
        let json: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
        assert_eq!(json["items"][0]["can_read"], true);
        assert_eq!(json["items"][0]["can_download"], expected_download);
        assert_eq!(json["items"][0]["authors"], serde_json::json!(["Writer"]));
        assert_eq!(json["items"][0]["languages"], serde_json::json!(["en"]));
        assert_eq!(json["items"][0]["media_type"], "application/octet-stream");
        let download = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri(&download_uri)
                    .header("Cookie", format!("folioharbor_session={actor}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(
            download.status(),
            if expected_download {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
            "download as {actor}"
        );
    }
    for role in ["owner", "editor"] {
        sqlx::query(
            "DELETE FROM folioharbor.role_permissions WHERE role_code=$1 AND permission_code='item.download'",
        )
        .bind(role)
        .execute(&pools.owner)
        .await?;
        let response = app.clone().oneshot(request(&uri, role)).await?;
        let json: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
        assert_eq!(json["items"][0]["can_download"], false);
        let download = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri(&download_uri)
                    .header("Cookie", format!("folioharbor_session={role}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(download.status(), StatusCode::FORBIDDEN);
        sqlx::query(
            "INSERT INTO folioharbor.role_permissions(role_code,permission_code) VALUES($1,'item.download')",
        )
        .bind(role)
        .execute(&pools.owner)
        .await?;
    }
    let detail_uri = format!(
        "/api/v1/libraries/{}/items/{}",
        library.as_uuid(),
        item.as_uuid()
    );
    let detail = app.clone().oneshot(request(&detail_uri, "reader")).await?;
    let disabled_etag = detail.headers()[ETAG].clone();
    let detail_json: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await?.to_bytes())?;
    assert_eq!(detail_json["identifiers"], serde_json::json!(["isbn:test"]));
    assert_eq!(detail_json["media_type"], "application/octet-stream");
    assert_eq!(detail_json["can_download"], false);
    let hidden = app.clone().oneshot(request(&uri, "outsider")).await?;
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let settings_uri = format!("/api/v1/libraries/{}/settings", library.as_uuid());
    let toggle = |enabled: bool| {
        Request::builder()
            .method("PATCH")
            .uri(&settings_uri)
            .header("Cookie", "folioharbor_session=owner")
            .header("X-CSRF-Token", "catalog-csrf")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "Catalog",
                    "reader_download_enabled": enabled
                })
                .to_string(),
            ))
            .expect("settings request")
    };
    assert_eq!(
        app.clone().oneshot(toggle(true)).await?.status(),
        StatusCode::NO_CONTENT
    );
    let response = app.clone().oneshot(request(&uri, "reader")).await?;
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    assert_eq!(json["items"][0]["can_download"], true);
    sqlx::query(
        "DELETE FROM folioharbor.role_permissions WHERE role_code='reader' AND permission_code='item.download'",
    )
    .execute(&pools.owner)
    .await?;
    let response = app.clone().oneshot(request(&uri, "reader")).await?;
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    assert_eq!(json["items"][0]["can_download"], false);
    let download = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&download_uri)
                .header("Cookie", "folioharbor_session=reader")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(download.status(), StatusCode::FORBIDDEN);
    sqlx::query(
        "INSERT INTO folioharbor.role_permissions(role_code,permission_code) VALUES('reader','item.download')",
    )
    .execute(&pools.owner)
    .await?;
    let detail = app.clone().oneshot(request(&detail_uri, "reader")).await?;
    assert_ne!(detail.headers()[ETAG], disabled_etag);
    let detail_json: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await?.to_bytes())?;
    assert_eq!(detail_json["can_download"], true);
    let download = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&download_uri)
                .header("Cookie", "folioharbor_session=reader")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(download.status(), StatusCode::OK);
    let download_etag = download.headers()[ETAG].clone();
    let unchanged = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&download_uri)
                .header("Cookie", "folioharbor_session=reader")
                .header("If-None-Match", download_etag)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);
    let invalid_range = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&download_uri)
                .header("Cookie", "folioharbor_session=reader")
                .header("Range", "bytes=99-")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(invalid_range.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    let full = app
        .clone()
        .oneshot(request(&download_uri, "reader"))
        .await?;
    assert_eq!(full.status(), StatusCode::OK);
    assert_eq!(full.into_body().collect().await?.to_bytes().len(), 16);
    let ranged = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&download_uri)
                .header("Cookie", "folioharbor_session=reader")
                .header("Range", "bytes=2-7")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(ranged.into_body().collect().await?.to_bytes().len(), 6);
    let allowed_ranges: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM folioharbor.audit_events WHERE action_code='item.download' AND decision='allowed' ORDER BY occurred_at,audit_event_id",
    )
    .fetch_all(&pools.owner)
    .await?;
    assert_eq!(
        allowed_ranges,
        [
            serde_json::json!({"range_start": 0, "range_end": 15}),
            serde_json::json!({"range_start": 2, "range_end": 7}),
        ],
        "only valid GET starts are audited with exact bounds"
    );
    assert_eq!(
        app.clone().oneshot(toggle(false)).await?.status(),
        StatusCode::NO_CONTENT
    );
    let response = app.clone().oneshot(request(&uri, "reader")).await?;
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    assert_eq!(json["items"][0]["can_download"], false);
    let download = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&download_uri)
                .header("Cookie", "folioharbor_session=reader")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(download.status(), StatusCode::FORBIDDEN);
    let lifecycle_request = |method: &str, uri: &str| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Cookie", "folioharbor_session=editor")
            .header("X-CSRF-Token", "catalog-csrf")
            .body(Body::empty())
            .expect("lifecycle request")
    };
    assert_eq!(
        app.clone()
            .oneshot(lifecycle_request("DELETE", &detail_uri))
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.clone()
            .oneshot(request(&detail_uri, "reader"))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.clone()
            .oneshot(lifecycle_request("POST", &format!("{detail_uri}/restore")))
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.oneshot(request(&detail_uri, "reader")).await?.status(),
        StatusCode::OK
    );
    let allowed_starts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.audit_events WHERE action_code='item.download' AND decision='allowed'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(
        allowed_starts, 2,
        "HEAD and denied GET attempts must not add download starts"
    );
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
    let storage_key = format!("blob:instance-v1:{}:16", "2a".repeat(32));
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
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,16,$3)").bind(blob.as_uuid()).bind(&sha).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.blob_locations(blob_id,storage_key,state,created_at,updated_at) VALUES($1,$2,'ready',$3,$3)").bind(blob.as_uuid()).bind(&storage_key).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,received_bytes,state,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'book.epub','application/octet-stream',16,'instance',16,'ready',$4,$5,$6,$6,$6)").bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid()).bind(&storage_key).bind(&sha).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages VALUES($1,$2,$3,'epub-v1',$4)")
        .bind(package.as_uuid())
        .bind(manifestation.as_uuid())
        .bind(blob.as_uuid())
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)").bind(holding.as_uuid()).bind(library.as_uuid()).bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)").bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(upload.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.item_assets(item_id,blob_id,asset_kind,created_at) VALUES($1,$2,'original',$3)").bind(item.as_uuid()).bind(blob.as_uuid()).bind(now).execute(pool).await?;
    Ok(item)
}
