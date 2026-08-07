use crate::{problem::ProblemContext, routes::AppState};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use folioharbor_domain::id::RequestId;
pub async fn attach(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::new);
    request.extensions_mut().insert(request_id);
    request
        .extensions_mut()
        .insert(ProblemContext::new(&state.public_base_url, request_id));
    next.run(request).await
}
