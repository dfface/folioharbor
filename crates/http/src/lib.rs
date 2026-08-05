#![forbid(unsafe_code)]

pub mod auth;
pub mod middleware;
pub mod problem;
pub mod routes;

pub use routes::{AppState, router};

pub(crate) fn problem_response(
    extensions: &axum::http::Extensions,
    error: &folioharbor_application::error::AppError,
) -> axum::response::Response {
    extensions.get::<problem::ProblemContext>().map_or_else(
        || axum::response::Response::new(axum::body::Body::empty()),
        |context| problem::response(error, context),
    )
}
