use crate::problem::{ProblemContext, request_validation_response};
use axum::{
    Json,
    extract::{FromRequest, Request},
    http::StatusCode,
    response::{IntoResponse as _, Response},
};
use serde::de::DeserializeOwned;

pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let context = request.extensions().get::<ProblemContext>().cloned();
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|rejection| {
                let status = rejection.status();
                let code = match status {
                    StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
                    StatusCode::UNPROCESSABLE_ENTITY => "invalid_json_body",
                    StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
                    _ => "malformed_json",
                };
                context.map_or_else(
                    || rejection.into_response(),
                    |context| request_validation_response(status, code, &context),
                )
            })
    }
}
