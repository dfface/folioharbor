#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::{collections::HashMap, sync::Arc};

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
    libraries::LibraryService,
    ports::{LibraryInvitationContext, MailError, Mailer},
    rate_limit::{CheckRateLimit, RateLimitDecision, RateLimitUseCase},
};
use folioharbor_domain::{
    id::{LibraryId, SessionId, UserId},
    identity::{CsrfToken, NormalizedEmail},
};
use folioharbor_http::{AppState, router};
use folioharbor_postgres::{
    PgAuditRepository, PgAuthorizationRepository, PgPools, libraries::PgLibraryRepository,
    run_migrations,
};
use folioharbor_test_support::{clock::FixedClock, postgres::TestPostgres, random::FixedRandom};
use http_body_util::BodyExt as _;
use secrecy::{ExposeSecret as _, SecretString};
use serde_yaml::Value;
use time::OffsetDateTime;
use tower::ServiceExt as _;
use url::Url;

struct RouteAuth(HashMap<String, UserId>);

fn unused<T>() -> Result<T, AppError> {
    unreachable!("identity endpoint is outside this route test")
}

#[async_trait]
impl RegisterAccountUseCase for RouteAuth {
    async fn register(&self, _: RegisterAccountCommand) -> Result<PendingAccount, AppError> {
        unused()
    }
}
#[async_trait]
impl VerifyEmailUseCase for RouteAuth {
    async fn verify_email(&self, _: VerifyEmailCommand) -> Result<VerifiedAccount, AppError> {
        unused()
    }
}
#[async_trait]
impl LoginUseCase for RouteAuth {
    async fn login(&self, _: LoginCommand) -> Result<IssuedSession, AppError> {
        unused()
    }
}
#[async_trait]
impl LogoutUseCase for RouteAuth {
    async fn logout(&self, _: LogoutCommand) -> Result<(), AppError> {
        unused()
    }
}
#[async_trait]
impl RequestPasswordResetUseCase for RouteAuth {
    async fn request_password_reset(
        &self,
        _: RequestPasswordResetCommand,
    ) -> Result<PasswordResetRequested, AppError> {
        unused()
    }
}
#[async_trait]
impl CompletePasswordResetUseCase for RouteAuth {
    async fn complete_password_reset(
        &self,
        _: CompletePasswordResetCommand,
    ) -> Result<PasswordResetComplete, AppError> {
        unused()
    }
}
#[async_trait]
impl AuthenticateSessionUseCase for RouteAuth {
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
                csrf_token_hash: CsrfToken::parse(SecretString::from("route-csrf".to_owned()))
                    .hash_for_storage(),
            }))
    }
}
#[async_trait]
impl CurrentSessionUseCase for RouteAuth {
    async fn current_session(&self, _: Actor) -> Result<SafeSession, AppError> {
        unused()
    }
}
#[async_trait]
impl ListSessionsUseCase for RouteAuth {
    async fn list_sessions(&self, _: Actor) -> Result<Vec<SafeSession>, AppError> {
        unused()
    }
}
#[async_trait]
impl RevokeSessionUseCase for RouteAuth {
    async fn revoke_session(
        &self,
        _: RevokeSessionCommand,
    ) -> Result<RevokeSessionOutcome, AppError> {
        unused()
    }
}
#[async_trait]
impl RateLimitUseCase for RouteAuth {
    async fn check_rate_limit(&self, _: CheckRateLimit) -> Result<RateLimitDecision, AppError> {
        unused()
    }
}

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

fn request(method: &Method, uri: &str, actor: &str, body: Body) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method.clone())
        .uri(uri)
        .header("Cookie", format!("folioharbor_session={actor}"));
    if method != Method::GET {
        builder = builder
            .header(CONTENT_TYPE, "application/json")
            .header("X-CSRF-Token", "route-csrf");
    }
    builder.body(body).expect("route request")
}

async fn problem_request_id(response: axum::response::Response) -> String {
    let body = response
        .into_body()
        .collect()
        .await
        .expect("problem body")
        .to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&body).expect("problem JSON");
    value["request_id"]
        .as_str()
        .expect("correlated request ID")
        .to_owned()
}

#[test]
fn openapi_exposes_the_complete_library_authorization_surface()
-> Result<(), Box<dyn std::error::Error>> {
    let document: Value =
        serde_yaml::from_str(include_str!("../../../openapi/folioharbor-v1.yaml"))?;
    let paths = document
        .get("paths")
        .and_then(Value::as_mapping)
        .ok_or_else(|| std::io::Error::other("paths must be a mapping"))?;
    for path in [
        "/api/v1/libraries",
        "/api/v1/libraries/{library_id}",
        "/api/v1/libraries/{library_id}/settings",
        "/api/v1/libraries/{library_id}/members",
        "/api/v1/libraries/{library_id}/members/{user_id}",
        "/api/v1/libraries/{library_id}/invitations",
    ] {
        assert!(
            paths.contains_key(Value::String(path.to_owned())),
            "missing {path}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn concrete_routes_enforce_role_matrix_and_correlate_denial_audits() -> anyhow::Result<()> {
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
        (owner, "owner@route.test"),
        (editor, "editor@route.test"),
        (reader, "reader@route.test"),
        (unrelated, "unrelated@route.test"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
            .bind(user.as_uuid()).bind(email).bind(now).execute(&pools.owner).await?;
    }
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'HTTP Matrix',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    for (user, role) in [(owner, "owner"), (editor, "editor"), (reader, "reader")] {
        sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,$3,'active',$4)")
            .bind(library.as_uuid()).bind(user.as_uuid()).bind(role).bind(now).execute(&pools.owner).await?;
    }

    let auth = Arc::new(RouteAuth(HashMap::from([
        ("owner".to_owned(), owner),
        ("editor".to_owned(), editor),
        ("reader".to_owned(), reader),
        ("unrelated".to_owned(), unrelated),
    ])));
    let service = Arc::new(LibraryService::new(
        PgLibraryRepository::new(pools.api.clone()),
        PgAuthorizationRepository::new(pools.api.clone()),
        PgAuditRepository::new(pools.api.clone()),
        NoopMailer,
        FixedClock::new(now),
        FixedRandom::new(4),
    ));
    let state = AppState::new(
        Url::parse("https://library.example")?,
        auth.clone(),
        auth.clone(),
        auth.clone(),
        auth.clone(),
        auth.clone(),
        auth.clone(),
        auth.clone(),
        auth.clone(),
        auth.clone(),
        auth.clone(),
        auth,
    )
    .with_library_api(service);
    let app = router(state);
    let detail = format!("/api/v1/libraries/{}", library.as_uuid());
    for actor in ["owner", "editor", "reader"] {
        let response = app
            .clone()
            .oneshot(request(&Method::GET, &detail, actor, Body::empty()))
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let unrelated_response = app
        .clone()
        .oneshot(request(&Method::GET, &detail, "unrelated", Body::empty()))
        .await?;
    assert_eq!(unrelated_response.status(), StatusCode::NOT_FOUND);
    let mut denial_ids = vec![problem_request_id(unrelated_response).await];

    let settings = format!("/api/v1/libraries/{}/settings", library.as_uuid());
    let owner_response = app
        .clone()
        .oneshot(request(
            &Method::PATCH,
            &settings,
            "owner",
            Body::from(r#"{"name":"HTTP Updated"}"#),
        ))
        .await?;
    assert_eq!(owner_response.status(), StatusCode::NO_CONTENT);
    for actor in ["editor", "reader"] {
        let response = app
            .clone()
            .oneshot(request(
                &Method::PATCH,
                &settings,
                actor,
                Body::from(r#"{"name":"Denied"}"#),
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        denial_ids.push(problem_request_id(response).await);
    }

    let audits: Vec<(String, String)> = sqlx::query_as(
        "SELECT decision,request_id FROM folioharbor.audit_events ORDER BY occurred_at,audit_event_id",
    )
    .fetch_all(&pools.owner)
    .await?;
    assert_eq!(audits.iter().filter(|(d, _)| d == "allowed").count(), 1);
    let persisted_denials = audits
        .iter()
        .filter(|(d, _)| d == "denied")
        .map(|(_, request_id)| request_id)
        .collect::<Vec<_>>();
    assert_eq!(persisted_denials.len(), 3);
    assert!(denial_ids.iter().all(|id| persisted_denials.contains(&id)));

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
