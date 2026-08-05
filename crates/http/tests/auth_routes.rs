#![allow(clippy::expect_used)]

use std::{
    sync::{Arc, Mutex},
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
        Ok(PendingAccount)
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
        Ok(IssuedSession {
            user_id: actor().user_id,
            session_id: actor().session_id,
            session_token: SecretString::from("opaque-session-secret".to_owned()),
            csrf_token: SecretString::from("csrf-secret".to_owned()),
        })
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
