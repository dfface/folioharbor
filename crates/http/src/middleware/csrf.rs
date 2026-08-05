use crate::{problem_response, routes::AppState};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use folioharbor_application::{error::AppError, identity::AuthenticateSessionCommand};
use folioharbor_domain::identity::CsrfToken;
use secrecy::SecretString;

pub const SESSION_COOKIE: &str = "folioharbor_session";
pub async fn authenticate_and_protect(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = cookie_value(
        request
            .headers()
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok()),
        SESSION_COOKIE,
    );
    if let Some(token) = token {
        let session = match state
            .authenticate_session
            .authenticate_session(AuthenticateSessionCommand {
                session_token: SecretString::from(token),
            })
            .await
        {
            Ok(value) => value,
            Err(error) => return problem_response(request.extensions(), &error),
        };
        if let Some(session) = session {
            if is_unsafe(request.method()) {
                let matches = request
                    .headers()
                    .get("X-CSRF-Token")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|value| {
                        CsrfToken::parse(SecretString::from(value.to_owned())).hash_for_storage()
                            == session.csrf_token_hash
                    });
                if !matches {
                    return problem_response(
                        request.extensions(),
                        &AppError::Forbidden {
                            code: "csrf_failed",
                        },
                    );
                }
            }
            request.extensions_mut().insert(session.actor);
        }
    }
    next.run(request).await
}
fn is_unsafe(method: &axum::http::Method) -> bool {
    !matches!(
        *method,
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    )
}
#[must_use]
pub fn cookie_value(header: Option<&str>, name: &str) -> Option<String> {
    header?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_owned())
}
