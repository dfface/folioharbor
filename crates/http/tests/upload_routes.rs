#![allow(clippy::expect_used)]

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::CONTENT_TYPE},
};
use folioharbor_application::{
    actor::Actor,
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
    imports::{CreateUploadRequest, GetUploadRequest, ReceiveUploadRequest, UploadApi},
    rate_limit::{CheckRateLimit, RateLimitDecision, RateLimitUseCase},
};
use folioharbor_domain::{
    id::{ItemId, LibraryId, SessionId, UploadId, UserId},
    identity::CsrfToken,
    imports::{
        quota::ByteCount,
        upload::{UploadSession, UploadState},
    },
};
use folioharbor_http::{AppState, router};
use http_body_util::BodyExt as _;
use secrecy::{ExposeSecret as _, SecretString};
use serde_yaml::Value;
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt as _;
use url::Url;

struct Services {
    sessions: HashMap<String, UserId>,
}
fn unused<T>() -> Result<T, AppError> {
    unreachable!("unused identity endpoint")
}
#[async_trait]
impl RegisterAccountUseCase for Services {
    async fn register(&self, _: RegisterAccountCommand) -> Result<PendingAccount, AppError> {
        unused()
    }
}
#[async_trait]
impl VerifyEmailUseCase for Services {
    async fn verify_email(&self, _: VerifyEmailCommand) -> Result<VerifiedAccount, AppError> {
        unused()
    }
}
#[async_trait]
impl LoginUseCase for Services {
    async fn login(&self, _: LoginCommand) -> Result<IssuedSession, AppError> {
        unused()
    }
}
#[async_trait]
impl LogoutUseCase for Services {
    async fn logout(&self, _: LogoutCommand) -> Result<(), AppError> {
        unused()
    }
}
#[async_trait]
impl RequestPasswordResetUseCase for Services {
    async fn request_password_reset(
        &self,
        _: RequestPasswordResetCommand,
    ) -> Result<PasswordResetRequested, AppError> {
        unused()
    }
}
#[async_trait]
impl CompletePasswordResetUseCase for Services {
    async fn complete_password_reset(
        &self,
        _: CompletePasswordResetCommand,
    ) -> Result<PasswordResetComplete, AppError> {
        unused()
    }
}
#[async_trait]
impl CurrentSessionUseCase for Services {
    async fn current_session(&self, _: Actor) -> Result<SafeSession, AppError> {
        unused()
    }
}
#[async_trait]
impl ListSessionsUseCase for Services {
    async fn list_sessions(&self, _: Actor) -> Result<Vec<SafeSession>, AppError> {
        unused()
    }
}
#[async_trait]
impl RevokeSessionUseCase for Services {
    async fn revoke_session(
        &self,
        _: RevokeSessionCommand,
    ) -> Result<RevokeSessionOutcome, AppError> {
        unused()
    }
}
#[async_trait]
impl RateLimitUseCase for Services {
    async fn check_rate_limit(&self, _: CheckRateLimit) -> Result<RateLimitDecision, AppError> {
        unused()
    }
}
#[async_trait]
impl AuthenticateSessionUseCase for Services {
    async fn authenticate_session(
        &self,
        command: AuthenticateSessionCommand,
    ) -> Result<Option<AuthenticatedSession>, AppError> {
        Ok(self
            .sessions
            .get(command.session_token.expose_secret())
            .copied()
            .map(|user_id| AuthenticatedSession {
                actor: Actor {
                    user_id,
                    session_id: SessionId::new(),
                },
                csrf_token_hash: CsrfToken::parse(SecretString::from("upload-csrf".to_owned()))
                    .hash_for_storage(),
            }))
    }
}

struct FakeUploads {
    upload: UploadSession,
    upload_limit_bytes: u64,
}
#[async_trait]
impl UploadApi for FakeUploads {
    async fn create_upload(&self, request: CreateUploadRequest) -> Result<UploadSession, AppError> {
        if request.declared_bytes > self.upload_limit_bytes {
            return Err(AppError::PayloadTooLarge);
        }
        Ok(self.upload.clone())
    }
    async fn receive_upload(
        &self,
        mut request: ReceiveUploadRequest,
    ) -> Result<UploadSession, AppError> {
        use futures_util::StreamExt as _;
        let mut total = 0_usize;
        while let Some(chunk) = request.bytes.next().await {
            total += chunk
                .map_err(|_| AppError::Invalid {
                    code: "upload_interrupted",
                    fields: Vec::new(),
                })?
                .len();
        }
        assert_eq!(total, 4);
        let mut upload = self.upload.clone();
        upload.received_bytes = ByteCount::new(4);
        upload.state = UploadState::Queued;
        Ok(upload)
    }
    async fn get_upload(&self, _: GetUploadRequest) -> Result<UploadSession, AppError> {
        Ok(self.upload.clone())
    }
}

fn app_with_state(state: UploadState) -> (axum::Router, LibraryId, UploadId, ItemId) {
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    let result_item = ItemId::new();
    let services = Arc::new(Services {
        sessions: HashMap::from([("actor".to_owned(), actor)]),
    });
    let uploads = Arc::new(FakeUploads {
        upload_limit_bytes: 4,
        upload: UploadSession {
            upload_id: upload,
            library_id: library,
            file_name: "book.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(4),
            received_bytes: ByteCount::new(0),
            state,
            storage_key: None,
            error_code: None,
            item_id: (state == UploadState::Duplicate).then_some(result_item),
        },
    });
    let state = AppState::new(
        Url::parse("https://library.example").expect("url"),
        services.clone(),
        services.clone(),
        services.clone(),
        services.clone(),
        services.clone(),
        services.clone(),
        services.clone(),
        services.clone(),
        services.clone(),
        services.clone(),
        services,
    )
    .with_upload_api(uploads);
    (router(state), library, upload, result_item)
}

fn app() -> (axum::Router, LibraryId, UploadId) {
    let (app, library, upload, _) = app_with_state(UploadState::Created);
    (app, library, upload)
}
fn request(method: Method, uri: String, content_type: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", "folioharbor_session=actor")
        .header("X-CSRF-Token", "upload-csrf")
        .header(CONTENT_TYPE, content_type)
        .body(body)
        .expect("request")
}

#[tokio::test]
async fn upload_routes_return_accepted_status_resources_and_reject_limit_before_body() {
    let (app, library, upload) = app();
    let create = request(
        Method::POST,
        format!("/api/v1/libraries/{}/uploads", library.as_uuid()),
        "application/json",
        Body::from(
            r#"{"file_name":"book.epub","media_type":"application/epub+zip","declared_bytes":4}"#,
        ),
    );
    let response = app.clone().oneshot(create).await.expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(response.headers().get("location").is_some());
    let put = request(
        Method::PUT,
        format!(
            "/api/v1/libraries/{}/uploads/{}/content",
            library.as_uuid(),
            upload.as_uuid()
        ),
        "application/epub+zip",
        Body::from("epub"),
    );
    assert_eq!(
        app.clone().oneshot(put).await.expect("response").status(),
        StatusCode::ACCEPTED
    );
    let oversized = request(
        Method::POST,
        format!("/api/v1/libraries/{}/uploads", library.as_uuid()),
        "application/json",
        Body::from(
            r#"{"file_name":"book.epub","media_type":"application/epub+zip","declared_bytes":1073741825}"#,
        ),
    );
    let response = app.oneshot(oversized).await.expect("response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("payload_too_large"));
}

#[tokio::test]
async fn duplicate_upload_status_exposes_the_existing_item_target() {
    let (app, library, upload, result_item) = app_with_state(UploadState::Duplicate);
    let response = app
        .oneshot(request(
            Method::GET,
            format!(
                "/api/v1/libraries/{}/uploads/{}",
                library.as_uuid(),
                upload.as_uuid()
            ),
            "application/json",
            Body::empty(),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&body).expect("JSON status");
    let expected_item = result_item.as_uuid().to_string();
    assert_eq!(
        value["item_id"].as_str(),
        Some(expected_item.as_str()),
        "Duplicate must identify the existing visible Item"
    );
}

#[test]
fn openapi_documents_deployment_configured_upload_limit_states_media_and_retry_contract() {
    let document: Value =
        serde_yaml::from_str(include_str!("../../../openapi/folioharbor-v1.yaml"))
            .expect("valid OpenAPI YAML");
    for path in [
        "/api/v1/libraries/{library_id}/uploads",
        "/api/v1/libraries/{library_id}/uploads/{upload_id}",
        "/api/v1/libraries/{library_id}/uploads/{upload_id}/content",
    ] {
        assert!(document["paths"].get(path).is_some(), "missing {path}");
    }
    assert_eq!(
        document["components"]["schemas"]["CreateUploadRequest"]["properties"]
            ["declared_bytes"]["description"]
            .as_str(),
        Some("Maximum is deployment-configured.")
    );
    let states = document["components"]["schemas"]["UploadStatus"]["properties"]["state"]["enum"]
        .as_sequence()
        .expect("states");
    for state in [
        "created",
        "receiving",
        "received",
        "queued",
        "validating",
        "importing",
        "ready",
        "duplicate",
        "failed",
        "expired",
        "retry_wait",
    ] {
        assert!(
            states.iter().any(|value| value.as_str() == Some(state)),
            "missing {state}"
        );
    }
    assert_eq!(
        document["components"]["schemas"]["UploadStatus"]["properties"]["item_id"]["format"]
            .as_str(),
        Some("uuid")
    );
    let description=document["paths"]["/api/v1/libraries/{library_id}/uploads/{upload_id}/content"]["put"]["description"].as_str().expect("retry description");
    assert!(description.contains("same upload_id"));
}
