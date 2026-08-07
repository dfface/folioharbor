#![allow(clippy::expect_used)]

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{
        Method, Request, StatusCode,
        header::{CONTENT_TYPE, COOKIE, SET_COOKIE},
    },
};
use folioharbor_application::{
    actor::Actor,
    config::{AuthFeatures, ConfigSources, Settings},
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
    operations::{HealthStatus, OperationsApi, RegistrationGate},
    rate_limit::{CheckRateLimit, RateLimitDecision, RateLimitUseCase},
};
use folioharbor_domain::{
    id::{SessionId, UserId},
    identity::{CsrfToken, TokenHash},
};
use folioharbor_http::{AppState, router};
use http_body_util::BodyExt as _;
use secrecy::SecretString;
use tower::ServiceExt as _;
use url::Url;

#[derive(Default)]
struct FakeAuth {
    recovery_emails: Mutex<Vec<String>>,
    rate_limited: Mutex<bool>,
    revoke_calls: AtomicUsize,
    fail_login_dependency: AtomicBool,
    register_calls: AtomicUsize,
}

fn actor() -> Actor {
    Actor {
        user_id: UserId::from_uuid(uuid::Uuid::from_u128(1)),
        session_id: SessionId::from_uuid(uuid::Uuid::from_u128(2)),
    }
}

fn csrf_hash() -> TokenHash {
    CsrfToken::parse(SecretString::from("csrf-secret".to_owned())).hash_for_storage()
}

#[async_trait]
impl RegisterAccountUseCase for FakeAuth {
    async fn register(&self, _: RegisterAccountCommand) -> Result<PendingAccount, AppError> {
        self.register_calls.fetch_add(1, Ordering::SeqCst);
        Ok(PendingAccount)
    }
}

struct MutableBootstrapState(AtomicBool);

#[async_trait]
impl OperationsApi for MutableBootstrapState {
    async fn readiness(&self) -> HealthStatus {
        if self.0.load(Ordering::SeqCst) {
            HealthStatus::Ready
        } else {
            HealthStatus::BootstrapRequired
        }
    }

    async fn registration_gate(&self) -> RegistrationGate {
        if self.0.load(Ordering::SeqCst) {
            RegistrationGate::Available
        } else {
            RegistrationGate::BootstrapRequired
        }
    }
}
#[async_trait]
impl VerifyEmailUseCase for FakeAuth {
    async fn verify_email(&self, _: VerifyEmailCommand) -> Result<VerifiedAccount, AppError> {
        Ok(VerifiedAccount {
            user_id: actor().user_id,
        })
    }
}
#[async_trait]
impl LoginUseCase for FakeAuth {
    async fn login(&self, _: LoginCommand) -> Result<IssuedSession, AppError> {
        if self.fail_login_dependency.load(Ordering::SeqCst) {
            return Err(AppError::DependencyUnavailable {
                code: "personal_library_provisioning_failed",
            });
        }
        Ok(IssuedSession {
            user_id: actor().user_id,
            session_id: actor().session_id,
            session_token: SecretString::from("opaque-session-secret".to_owned()),
            csrf_token: SecretString::from("csrf-secret".to_owned()),
        })
    }
}

#[tokio::test]
async fn local_and_invited_login_dependency_failures_never_issue_a_session_cookie() {
    for email in ["local@example.com", "invited@example.com"] {
        let fake = Arc::new(FakeAuth::default());
        fake.fail_login_dependency.store(true, Ordering::SeqCst);
        let response = app(fake)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/login")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        r#"{{"email":"{email}","password":"secret"}}"#
                    )))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().get(SET_COOKIE).is_none());
    }
}
#[async_trait]
impl LogoutUseCase for FakeAuth {
    async fn logout(&self, _: LogoutCommand) -> Result<(), AppError> {
        Ok(())
    }
}
#[async_trait]
impl RequestPasswordResetUseCase for FakeAuth {
    async fn request_password_reset(
        &self,
        command: RequestPasswordResetCommand,
    ) -> Result<PasswordResetRequested, AppError> {
        self.recovery_emails
            .lock()
            .map_err(|_| AppError::DependencyUnavailable { code: "test_lock" })?
            .push(command.email);
        Ok(PasswordResetRequested)
    }
}
#[async_trait]
impl CompletePasswordResetUseCase for FakeAuth {
    async fn complete_password_reset(
        &self,
        _: CompletePasswordResetCommand,
    ) -> Result<PasswordResetComplete, AppError> {
        Ok(PasswordResetComplete {
            user_id: actor().user_id,
            session_id: actor().session_id,
            session_token: SecretString::from("rotated-session-secret".to_owned()),
            csrf_token: SecretString::from("rotated-csrf-secret".to_owned()),
        })
    }
}
#[async_trait]
impl AuthenticateSessionUseCase for FakeAuth {
    async fn authenticate_session(
        &self,
        command: AuthenticateSessionCommand,
    ) -> Result<Option<AuthenticatedSession>, AppError> {
        use secrecy::ExposeSecret as _;
        Ok(
            (command.session_token.expose_secret() == "opaque-session-secret").then(|| {
                AuthenticatedSession {
                    actor: actor(),
                    csrf_token_hash: csrf_hash(),
                }
            }),
        )
    }
}
#[async_trait]
impl CurrentSessionUseCase for FakeAuth {
    async fn current_session(&self, actor: Actor) -> Result<SafeSession, AppError> {
        Ok(SafeSession::active(actor.session_id, true))
    }
}
#[async_trait]
impl ListSessionsUseCase for FakeAuth {
    async fn list_sessions(&self, actor: Actor) -> Result<Vec<SafeSession>, AppError> {
        Ok(vec![SafeSession::active(actor.session_id, true)])
    }
}
#[async_trait]
impl RevokeSessionUseCase for FakeAuth {
    async fn revoke_session(
        &self,
        command: RevokeSessionCommand,
    ) -> Result<RevokeSessionOutcome, AppError> {
        self.revoke_calls.fetch_add(1, Ordering::SeqCst);
        if command.session_id != command.actor.session_id {
            return Err(AppError::NotFound {
                code: "session_not_found",
            });
        }
        Ok(RevokeSessionOutcome {
            revoked_current: true,
        })
    }
}
#[async_trait]
impl RateLimitUseCase for FakeAuth {
    async fn check_rate_limit(&self, _: CheckRateLimit) -> Result<RateLimitDecision, AppError> {
        if *self
            .rate_limited
            .lock()
            .map_err(|_| AppError::DependencyUnavailable { code: "test_lock" })?
        {
            Ok(RateLimitDecision::Denied {
                retry_after: Duration::from_secs(17),
            })
        } else {
            Ok(RateLimitDecision::Allowed)
        }
    }
}

fn app(fake: Arc<FakeAuth>) -> axum::Router {
    router(AppState::new(
        Url::parse("https://library.example").expect("valid test URL"),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake,
    ))
}

fn app_with_auth_features(fake: Arc<FakeAuth>, features: AuthFeatures) -> axum::Router {
    router(
        AppState::new(
            Url::parse("https://library.example").expect("valid test URL"),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake,
        )
        .with_auth_features(features),
    )
}

fn app_with_operations(fake: Arc<FakeAuth>, operations: Arc<dyn OperationsApi>) -> axum::Router {
    router(
        AppState::new(
            Url::parse("https://library.example").expect("valid test URL"),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake,
        )
        .with_auth_features(AuthFeatures::new([true, true, true, true]))
        .with_operations(operations),
    )
}

fn enabled_auth_features() -> AuthFeatures {
    Settings::load(ConfigSources {
        environment: BTreeMap::from([
            (
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET_KEY_ID".to_owned(),
                "route-test".to_owned(),
            ),
            (
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET".to_owned(),
                "0123456789abcdef0123456789abcdef".to_owned(),
            ),
            (
                "FOLIOHARBOR_MAIL_SMTP_URL".to_owned(),
                "smtp://mail.example:2525".to_owned(),
            ),
        ]),
        ..ConfigSources::default()
    })
    .expect("valid enabled mail configuration")
    .auth
    .features()
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("JSON response")
}

#[tokio::test]
async fn request_validation_failures_are_correlated_problem_details() {
    let cases = [
        (
            Some("application/json"),
            r#"{"email":"reader@example.com""#,
            StatusCode::BAD_REQUEST,
            "malformed_json",
        ),
        (
            Some("application/json"),
            r#"{"email":"reader@example.com"}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_json_body",
        ),
        (
            Some("text/plain"),
            r#"{"email":"reader@example.com","password":"secret"}"#,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
        ),
    ];

    for (content_type, body, expected_status, expected_code) in cases {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/login");
        if let Some(content_type) = content_type {
            request = request.header(CONTENT_TYPE, content_type);
        }
        let response = app(Arc::new(FakeAuth::default()))
            .oneshot(request.body(Body::from(body)).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), expected_status);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/problem+json")
        );
        let json = response_json(response).await;
        assert_eq!(json["code"], expected_code);
        let request_id = json["request_id"].as_str().expect("request ID");
        assert_eq!(request_id.len(), 26);
        assert_eq!(json["instance"], format!("/problems/{request_id}"));
    }
}

#[tokio::test]
async fn disabled_auth_features_remove_all_optional_auth_routes() {
    for path in [
        "/api/v1/auth/register",
        "/api/v1/auth/verify-email",
        "/api/v1/auth/forgot-password",
        "/api/v1/auth/reset-password",
    ] {
        let response = app_with_auth_features(
            Arc::new(FakeAuth::default()),
            AuthFeatures::new([false, false, false, false]),
        )
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await
        .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "route {path}");
    }

    let login = app_with_auth_features(
        Arc::new(FakeAuth::default()),
        AuthFeatures::new([false, false, false, false]),
    )
    .oneshot(
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/login")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .expect("request"),
    )
    .await
    .expect("response");
    assert_ne!(login.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn enabled_auth_features_keep_all_optional_auth_routes() {
    for path in [
        "/api/v1/auth/register",
        "/api/v1/auth/verify-email",
        "/api/v1/auth/forgot-password",
        "/api/v1/auth/reset-password",
    ] {
        let response =
            app_with_auth_features(Arc::new(FakeAuth::default()), enabled_auth_features())
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from("{}"))
                        .expect("request"),
                )
                .await
                .expect("response");

        assert_ne!(response.status(), StatusCode::NOT_FOUND, "route {path}");
    }
}

#[tokio::test]
async fn registration_opens_only_after_system_administrator_bootstrap() {
    let fake = Arc::new(FakeAuth::default());
    let operations = Arc::new(MutableBootstrapState(AtomicBool::new(false)));
    let app = app_with_operations(fake.clone(), operations.clone());
    let request = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/register")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"email":"reader@example.com","password":"secret"}"#,
            ))
            .expect("request")
    };

    let blocked = app.clone().oneshot(request()).await.expect("response");
    assert_eq!(blocked.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response_json(blocked).await["code"], "bootstrap_required");
    assert_eq!(fake.register_calls.load(Ordering::SeqCst), 0);

    operations.0.store(true, Ordering::SeqCst);
    let available = app.oneshot(request()).await.expect("response");
    assert_eq!(available.status(), StatusCode::ACCEPTED);
    assert_eq!(fake.register_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mixed_auth_features_mount_only_the_independently_enabled_routes() {
    let features = AuthFeatures::new([false, true, false, false]);
    for (path, expected) in [
        ("/api/v1/auth/register", StatusCode::NOT_FOUND),
        (
            "/api/v1/auth/verify-email",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        ("/api/v1/auth/forgot-password", StatusCode::NOT_FOUND),
        ("/api/v1/auth/reset-password", StatusCode::NOT_FOUND),
    ] {
        let response = app_with_auth_features(Arc::new(FakeAuth::default()), features)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), expected, "route {path}");
    }
}

#[tokio::test]
async fn login_sets_secure_opaque_cookie_without_returning_tokens_in_json() {
    let response = app(Arc::new(FakeAuth::default()))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/login")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"reader@example.com","password":"correct horse"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get(SET_COOKIE)
        .expect("session cookie")
        .to_str()
        .expect("ASCII cookie");
    assert!(cookie.starts_with("folioharbor_session=opaque-session-secret;"));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Secure"));
    assert!(cookie.contains("SameSite=Lax"));
    assert!(cookie.contains("Path=/"));
    let json = response_json(response).await;
    assert!(json.get("session_token").is_none());
    assert!(json.get("csrf_token").is_none());
}

#[tokio::test]
async fn password_reset_sets_fresh_secure_cookies_without_tokens_in_json() {
    let response = app(Arc::new(FakeAuth::default()))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/reset-password")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"token":"reset-token","new_password":"new-password"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(cookies.iter().any(|cookie| {
        cookie.starts_with("folioharbor_session=rotated-session-secret;")
            && cookie.contains("HttpOnly")
            && cookie.contains("Secure")
            && cookie.contains("SameSite=Lax")
            && cookie.contains("Path=/")
    }));
    assert!(cookies.iter().any(
        |cookie| cookie.starts_with("folioharbor_csrf=rotated-csrf-secret;")
            && cookie.contains("Secure")
            && cookie.contains("SameSite=Lax")
            && cookie.contains("Path=/")
    ));
    let json = response_json(response).await;
    assert_eq!(json["user_id"], actor().user_id.as_uuid().to_string());
    assert_eq!(json["session_id"], actor().session_id.as_uuid().to_string());
    assert!(json.get("session_token").is_none());
    assert!(json.get("csrf_token").is_none());
}

#[tokio::test]
async fn unsafe_authenticated_route_requires_matching_csrf_but_safe_route_does_not() {
    let app = app(Arc::new(FakeAuth::default()));
    let safe = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/session")
                .header(COOKIE, "folioharbor_session=opaque-session-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(safe.status(), StatusCode::OK);
    assert_eq!(
        response_json(safe).await["user_id"],
        actor().user_id.as_uuid().to_string()
    );
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/auth/sessions/{}/revoke",
                    actor().session_id.as_uuid()
                ))
                .header(COOKIE, "folioharbor_session=opaque-session-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        denied
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json")
    );
    assert_eq!(response_json(denied).await["code"], "csrf_failed");
    let allowed = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/auth/sessions/{}/revoke",
                    actor().session_id.as_uuid()
                ))
                .header(COOKIE, "folioharbor_session=opaque-session-secret")
                .header("X-CSRF-Token", "csrf-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(allowed.status(), StatusCode::NO_CONTENT);
    assert!(allowed.headers().get_all(SET_COOKIE).iter().any(|value| {
        value
            .to_str()
            .is_ok_and(|cookie| cookie.contains("Max-Age=0"))
    }));
}

#[tokio::test]
async fn password_recovery_responses_are_indistinguishable() {
    let app = app(Arc::new(FakeAuth::default()));
    let mut observed = Vec::new();
    for email in ["known@example.com", "unknown@example.com"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/forgot-password")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"email":"{email}"}}"#)))
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        observed.push((status, headers.get(CONTENT_TYPE).cloned(), body));
    }
    assert_eq!(observed[0], observed[1]);
}

#[tokio::test]
async fn rate_limit_returns_problem_and_retry_after() {
    let fake = Arc::new(FakeAuth::default());
    *fake.rate_limited.lock().expect("test lock") = true;
    let response = app(fake)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/login")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"reader@example.com","password":"guess"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response
            .headers()
            .get("Retry-After")
            .and_then(|v| v.to_str().ok()),
        Some("17")
    );
    assert_eq!(response_json(response).await["code"], "rate_limited");
}

#[tokio::test]
async fn session_listing_contains_only_safe_metadata_and_revoke_is_owner_scoped() {
    let app = app(Arc::new(FakeAuth::default()));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/sessions")
                .header(COOKIE, "folioharbor_session=opaque-session-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let json = response_json(response).await;
    assert!(json.to_string().contains("session_id"));
    assert!(!json.to_string().contains("token"));
    assert!(!json.to_string().contains("hash"));
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/auth/sessions/{}/revoke",
                    uuid::Uuid::from_u128(99)
                ))
                .header(COOKIE, "folioharbor_session=opaque-session-secret")
                .header("X-CSRF-Token", "csrf-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn malformed_revoke_session_id_is_correlated_problem_without_use_case_invocation() {
    let fake = Arc::new(FakeAuth::default());
    let response = app(fake.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/sessions/not-a-uuid/revoke")
                .header(COOKIE, "folioharbor_session=opaque-session-secret")
                .header("X-CSRF-Token", "csrf-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json")
    );
    let problem = response_json(response).await;
    assert_eq!(problem["code"], "invalid_session_id");
    let request_id = problem["request_id"].as_str().expect("request ID");
    assert_eq!(request_id.len(), 26);
    assert_eq!(problem["instance"], format!("/problems/{request_id}"));
    assert_eq!(fake.revoke_calls.load(Ordering::SeqCst), 0);
}
