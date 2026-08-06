#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::CONTENT_TYPE},
};
use folioharbor_application::{
    actor::Actor,
    config::AuthFeatures,
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
    id::{LibraryId, SessionId, UserId},
    identity::CsrfToken,
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
struct NoopSealer;
impl MailIntentSealer for NoopSealer {
    fn seal(
        &self,
        message: MailMessage,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> Result<NewMailOutboxEntry, MailOutboxError> {
        Ok(test_mail_entry(&message, now, expires_at))
    }
}

#[derive(Clone, Copy)]
struct UnavailableSealer;
impl MailIntentSealer for UnavailableSealer {
    fn seal(
        &self,
        _: MailMessage,
        _: OffsetDateTime,
        _: OffsetDateTime,
    ) -> Result<NewMailOutboxEntry, MailOutboxError> {
        Err(MailOutboxError::Encryption)
    }
}

fn test_mail_entry(
    message: &MailMessage,
    now: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> NewMailOutboxEntry {
    NewMailOutboxEntry {
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
    let unavailable = &document["paths"]["/api/v1/libraries/{library_id}/invitations"]["post"]["responses"]
        ["503"];
    assert_eq!(
        unavailable["content"]["application/problem+json"]["schema"]["$ref"],
        "#/components/schemas/ProblemDetails"
    );
    assert_eq!(
        unavailable["content"]["application/problem+json"]["example"]["code"],
        "mail_delivery_unavailable"
    );
    assert_eq!(
        unavailable["content"]["application/problem+json"]["example"]["status"],
        503
    );
    assert!(
        unavailable["content"]["application/problem+json"]["example"]["request_id"]
            .as_str()
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn disabled_invitation_feature_removes_its_route_without_removing_library_routes()
-> anyhow::Result<()> {
    let owner = UserId::new();
    let auth = Arc::new(RouteAuth(HashMap::from([("owner".to_owned(), owner)])));
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
    .with_auth_features(AuthFeatures::new(false, false, false, false));
    let app = router(state);
    let invitation = app
        .clone()
        .oneshot(request(
            &Method::POST,
            &format!(
                "/api/v1/libraries/{}/invitations",
                LibraryId::new().as_uuid()
            ),
            "owner",
            Body::from(r#"{"email":"reader@example.com","role":"reader"}"#),
        ))
        .await?;
    assert_eq!(invitation.status(), StatusCode::NOT_FOUND);

    let libraries = app
        .oneshot(request(
            &Method::GET,
            "/api/v1/libraries",
            "owner",
            Body::empty(),
        ))
        .await?;
    assert_eq!(libraries.status(), StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn unavailable_invitation_delivery_returns_correlated_503_without_persistence_or_allowed_audit()
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
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'owner@unavailable.test','owner@unavailable.test','verified',$2,$2)")
        .bind(owner.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Unavailable Mail',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'owner','active',$3)")
        .bind(library.as_uuid()).bind(owner.as_uuid()).bind(now).execute(&pools.owner).await?;

    let auth = Arc::new(RouteAuth(HashMap::from([("owner".to_owned(), owner)])));
    let service = Arc::new(LibraryService::new(
        PgLibraryRepository::new(pools.api.clone()),
        PgAuthorizationRepository::new(pools.api.clone()),
        PgAuditRepository::new(pools.api.clone()),
        UnavailableSealer,
        FixedClock::new(now),
        FixedRandom::new(5),
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
    let response = router(state)
        .oneshot(request(
            &Method::POST,
            &format!("/api/v1/libraries/{}/invitations", library.as_uuid()),
            "owner",
            Body::from(r#"{"email":"reader@unavailable.test","role":"reader"}"#),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response.into_body().collect().await?.to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(problem["code"], "mail_delivery_unavailable");
    assert_eq!(problem["status"], 503);
    let request_id = problem["request_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("503 response must have a request ID"))?;
    assert_eq!(
        problem["instance"],
        format!("/problems/{request_id}"),
        "503 problem instance must correlate to its request ID"
    );

    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.library_invitations WHERE library_id=$1),(SELECT count(*) FROM folioharbor.audit_events WHERE decision='allowed')",
    )
    .bind(library.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(counts, (0, 0));

    pools.close().await;
    database.cleanup().await?;
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
    let target = UserId::new();
    for (user, email) in [
        (owner, "owner@route.test"),
        (editor, "editor@route.test"),
        (reader, "reader@route.test"),
        (unrelated, "unrelated@route.test"),
        (target, "target@route.test"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
            .bind(user.as_uuid()).bind(email).bind(now).execute(&pools.owner).await?;
    }
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'HTTP Matrix',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    for (user, role) in [
        (owner, "owner"),
        (editor, "editor"),
        (reader, "reader"),
        (target, "reader"),
    ] {
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
        NoopSealer,
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

    for actor in ["owner", "editor", "reader", "unrelated"] {
        let response = app
            .clone()
            .oneshot(request(
                &Method::GET,
                "/api/v1/libraries",
                actor,
                Body::empty(),
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "list as {actor}");
    }

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

    let members = format!("/api/v1/libraries/{}/members", library.as_uuid());
    for actor in ["owner", "editor", "reader"] {
        let response = app
            .clone()
            .oneshot(request(&Method::GET, &members, actor, Body::empty()))
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "member list as {actor}");
    }
    let response = app
        .clone()
        .oneshot(request(&Method::GET, &members, "unrelated", Body::empty()))
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    denial_ids.push(problem_request_id(response).await);

    let operations = [
        (
            Method::PATCH,
            format!("/api/v1/libraries/{}/settings", library.as_uuid()),
            r#"{"name":"HTTP Updated"}"#,
        ),
        (
            Method::POST,
            format!("/api/v1/libraries/{}/invitations", library.as_uuid()),
            r#"{"email":"invited@route.test","role":"reader"}"#,
        ),
        (
            Method::PATCH,
            format!(
                "/api/v1/libraries/{}/members/{}",
                library.as_uuid(),
                target.as_uuid()
            ),
            r#"{"role":"editor"}"#,
        ),
        (
            Method::DELETE,
            format!(
                "/api/v1/libraries/{}/members/{}",
                library.as_uuid(),
                target.as_uuid()
            ),
            "",
        ),
    ];
    for (operation_index, (method, uri, body)) in operations.iter().enumerate() {
        let response = app
            .clone()
            .oneshot(request(method, uri, "owner", Body::from(*body)))
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::NO_CONTENT,
            "owner operation {uri}"
        );
        let allowed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM folioharbor.audit_events WHERE decision='allowed'",
        )
        .fetch_one(&pools.owner)
        .await?;
        assert_eq!(allowed, i64::try_from(operation_index + 1)?);

        for (actor, status) in [
            ("editor", StatusCode::FORBIDDEN),
            ("reader", StatusCode::FORBIDDEN),
            ("unrelated", StatusCode::NOT_FOUND),
        ] {
            let response = app
                .clone()
                .oneshot(request(method, uri, actor, Body::from(*body)))
                .await?;
            assert_eq!(response.status(), status, "{uri} as {actor}");
            denial_ids.push(problem_request_id(response).await);
        }
    }

    let mutation_state: (String, i64, String, String) = sqlx::query_as(
        "SELECT l.name,(SELECT count(*) FROM folioharbor.library_invitations i WHERE i.library_id=l.library_id),(SELECT role_code FROM folioharbor.library_memberships WHERE library_id=l.library_id AND user_id=$2),(SELECT status FROM folioharbor.library_memberships WHERE library_id=l.library_id AND user_id=$2) FROM folioharbor.libraries l WHERE l.library_id=$1",
    )
    .bind(library.as_uuid())
    .bind(target.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(
        mutation_state,
        (
            "HTTP Updated".to_owned(),
            1,
            "editor".to_owned(),
            "removed".to_owned()
        )
    );

    let audits: Vec<(String, String)> = sqlx::query_as(
        "SELECT decision,request_id FROM folioharbor.audit_events ORDER BY occurred_at,audit_event_id",
    )
    .fetch_all(&pools.owner)
    .await?;
    assert_eq!(audits.iter().filter(|(d, _)| d == "allowed").count(), 4);
    let persisted_denials = audits
        .iter()
        .filter(|(d, _)| d == "denied")
        .map(|(_, request_id)| request_id)
        .collect::<Vec<_>>();
    assert_eq!(persisted_denials.len(), 14);
    assert!(denial_ids.iter().all(|id| persisted_denials.contains(&id)));

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
