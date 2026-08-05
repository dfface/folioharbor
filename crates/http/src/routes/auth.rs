use super::AppState;
use crate::{
    auth::{AuthenticatedActor, ClientIpPrefix},
    middleware::csrf::{SESSION_COOKIE, cookie_value},
    problem::{ProblemContext, response as problem_response},
};
use axum::{
    Json, Router,
    extract::{Extension, Path, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{COOKIE, SET_COOKIE},
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use folioharbor_application::{
    error::AppError,
    identity::{
        CompletePasswordResetCommand, LoginCommand, LogoutCommand, RegisterAccountCommand,
        RequestPasswordResetCommand, RevokeSessionCommand, SafeSession, VerifyEmailCommand,
    },
    rate_limit::{CheckRateLimit, RateLimitDecision, RateLimitPurpose},
};
use folioharbor_domain::{id::SessionId, identity::SessionStatus};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/register", post(register))
        .route("/verify-email", post(verify_email))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/forgot-password", post(forgot_password))
        .route("/reset-password", post(reset_password))
        .route("/session", get(current_session))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{session_id}/revoke", post(revoke_session))
}

#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
}
#[derive(Deserialize)]
struct TokenBody {
    token: String,
}
#[derive(Deserialize)]
struct ForgotPasswordBody {
    email: String,
}
#[derive(Deserialize)]
struct ResetPasswordBody {
    token: String,
    new_password: String,
}
#[derive(Serialize)]
struct Accepted {
    status: &'static str,
}
#[derive(Serialize)]
struct LoginResponse {
    user_id: String,
    session_id: String,
}
#[derive(Serialize)]
struct SessionResponse {
    session_id: String,
    is_current: bool,
    status: &'static str,
}

async fn limited(
    state: &AppState,
    purpose: RateLimitPurpose,
    identifier: &str,
    ip_prefix: &str,
) -> Result<(), AppError> {
    match state
        .rate_limit
        .check_rate_limit(CheckRateLimit {
            purpose,
            normalized_identifier: identifier.trim().to_lowercase(),
            ip_prefix: ip_prefix.to_owned(),
        })
        .await?
    {
        RateLimitDecision::Allowed => Ok(()),
        RateLimitDecision::Denied { retry_after } => Err(AppError::RateLimited { retry_after }),
    }
}

async fn register(
    State(state): State<AppState>,
    Extension(problem): Extension<ProblemContext>,
    ClientIpPrefix(ip_prefix): ClientIpPrefix,
    Json(body): Json<Credentials>,
) -> Response {
    if let Err(error) = limited(
        &state,
        RateLimitPurpose::Registration,
        &body.email,
        &ip_prefix,
    )
    .await
    {
        return problem_response(&error, &problem);
    }
    match state
        .register
        .register(RegisterAccountCommand {
            email: body.email,
            password: SecretString::from(body.password),
        })
        .await
    {
        Ok(_) => (StatusCode::ACCEPTED, Json(Accepted { status: "pending" })).into_response(),
        Err(error) => problem_response(&error, &problem),
    }
}
async fn verify_email(
    State(state): State<AppState>,
    Extension(problem): Extension<ProblemContext>,
    ClientIpPrefix(ip_prefix): ClientIpPrefix,
    Json(body): Json<TokenBody>,
) -> Response {
    if let Err(error) = limited(
        &state,
        RateLimitPurpose::Verification,
        &body.token,
        &ip_prefix,
    )
    .await
    {
        return problem_response(&error, &problem);
    }
    match state
        .verify
        .verify_email(VerifyEmailCommand {
            token: SecretString::from(body.token),
        })
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => problem_response(&error, &problem),
    }
}
async fn login(
    State(state): State<AppState>,
    Extension(problem): Extension<ProblemContext>,
    ClientIpPrefix(ip_prefix): ClientIpPrefix,
    Json(body): Json<Credentials>,
) -> Response {
    if let Err(error) = limited(&state, RateLimitPurpose::Login, &body.email, &ip_prefix).await {
        return problem_response(&error, &problem);
    }
    match state
        .login
        .login(LoginCommand {
            email: body.email,
            password: SecretString::from(body.password),
        })
        .await
    {
        Ok(session) => {
            let mut response = Json(LoginResponse {
                user_id: session.user_id.as_uuid().to_string(),
                session_id: session.session_id.as_uuid().to_string(),
            })
            .into_response();
            let cookie = format!(
                "{SESSION_COOKIE}={}; HttpOnly; Secure; SameSite=Lax; Path=/",
                session.session_token.expose_secret()
            );
            let csrf = format!(
                "folioharbor_csrf={}; Secure; SameSite=Lax; Path=/",
                session.csrf_token.expose_secret()
            );
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                response.headers_mut().append(SET_COOKIE, value);
            }
            if let Ok(value) = HeaderValue::from_str(&csrf) {
                response.headers_mut().append(SET_COOKIE, value);
            }
            response
        }
        Err(error) => problem_response(&error, &problem),
    }
}
async fn logout(
    State(state): State<AppState>,
    Extension(problem): Extension<ProblemContext>,
    _: AuthenticatedActor,
    headers: HeaderMap,
) -> Response {
    let Some(token) = cookie_value(
        headers.get(COOKIE).and_then(|v| v.to_str().ok()),
        SESSION_COOKIE,
    ) else {
        return problem_response(&AppError::Unauthenticated, &problem);
    };
    match state
        .logout
        .logout(LogoutCommand {
            session_token: SecretString::from(token),
        })
        .await
    {
        Ok(()) => cleared_cookie_response(),
        Err(error) => problem_response(&error, &problem),
    }
}
async fn forgot_password(
    State(state): State<AppState>,
    Extension(problem): Extension<ProblemContext>,
    ClientIpPrefix(ip_prefix): ClientIpPrefix,
    Json(body): Json<ForgotPasswordBody>,
) -> Response {
    if let Err(error) = limited(
        &state,
        RateLimitPurpose::PasswordReset,
        &body.email,
        &ip_prefix,
    )
    .await
    {
        return problem_response(&error, &problem);
    }
    match state
        .request_password_reset
        .request_password_reset(RequestPasswordResetCommand { email: body.email })
        .await
    {
        Ok(_) => (StatusCode::ACCEPTED, Json(Accepted { status: "accepted" })).into_response(),
        Err(error) => problem_response(&error, &problem),
    }
}
async fn reset_password(
    State(state): State<AppState>,
    Extension(problem): Extension<ProblemContext>,
    ClientIpPrefix(ip_prefix): ClientIpPrefix,
    Json(body): Json<ResetPasswordBody>,
) -> Response {
    if let Err(error) = limited(
        &state,
        RateLimitPurpose::PasswordReset,
        &body.token,
        &ip_prefix,
    )
    .await
    {
        return problem_response(&error, &problem);
    }
    match state
        .complete_password_reset
        .complete_password_reset(CompletePasswordResetCommand {
            token: SecretString::from(body.token),
            new_password: SecretString::from(body.new_password),
        })
        .await
    {
        Ok(_) => cleared_cookie_response(),
        Err(error) => problem_response(&error, &problem),
    }
}
async fn current_session(
    State(state): State<AppState>,
    Extension(problem): Extension<ProblemContext>,
    AuthenticatedActor(actor): AuthenticatedActor,
) -> Response {
    match state.current_session.current_session(actor).await {
        Ok(session) => Json(to_response(session)).into_response(),
        Err(error) => problem_response(&error, &problem),
    }
}
async fn list_sessions(
    State(state): State<AppState>,
    Extension(problem): Extension<ProblemContext>,
    AuthenticatedActor(actor): AuthenticatedActor,
) -> Response {
    match state.list_sessions.list_sessions(actor).await {
        Ok(sessions) => {
            Json(sessions.into_iter().map(to_response).collect::<Vec<_>>()).into_response()
        }
        Err(error) => problem_response(&error, &problem),
    }
}
async fn revoke_session(
    State(state): State<AppState>,
    Extension(problem): Extension<ProblemContext>,
    Path(raw): Path<String>,
    AuthenticatedActor(actor): AuthenticatedActor,
) -> Response {
    let Ok(raw) = Uuid::parse_str(&raw) else {
        return problem_response(
            &AppError::NotFound {
                code: "session_not_found",
            },
            &problem,
        );
    };
    match state
        .revoke_session
        .revoke_session(RevokeSessionCommand {
            actor,
            session_id: SessionId::from_uuid(raw),
        })
        .await
    {
        Ok(outcome) if outcome.revoked_current => cleared_cookie_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => problem_response(&error, &problem),
    }
}
fn to_response(session: SafeSession) -> SessionResponse {
    SessionResponse {
        session_id: session.session_id.as_uuid().to_string(),
        is_current: session.is_current,
        status: match session.status {
            SessionStatus::Active => "active",
            SessionStatus::IdleExpired => "idle_expired",
            SessionStatus::AbsolutelyExpired => "absolute_expired",
            SessionStatus::Revoked => "revoked",
        },
    }
}
fn cleared_cookie_response() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_static(
            "folioharbor_session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0",
        ),
    );
    response.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_static("folioharbor_csrf=; Secure; SameSite=Lax; Path=/; Max-Age=0"),
    );
    response
}
