use super::AppState;
use crate::{
    auth::AuthenticatedActor,
    middleware::telemetry::{MetricAttributes, TelemetryMetrics},
    problem::{ProblemContext, response as problem_response},
};
use axum::{
    Json, Router,
    body::Body,
    extract::{Extension, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use folioharbor_application::{
    error::{AppError, FieldViolation},
    imports::{
        CreateUploadRequest, GetUploadRequest, MAX_UPLOAD_BYTES, ReceiveUploadRequest,
        UploadStreamError,
    },
};
use folioharbor_domain::{
    id::{LibraryId, RequestId, UploadId},
    imports::upload::UploadSession,
};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{library_id}/uploads", post(create_upload))
        .route("/{library_id}/uploads/{upload_id}", get(get_upload))
        .route(
            "/{library_id}/uploads/{upload_id}/content",
            put(receive_upload),
        )
}

#[derive(Deserialize)]
struct CreateBody {
    file_name: String,
    media_type: String,
    declared_bytes: u64,
}
#[derive(Serialize)]
struct UploadResponse {
    upload_id: String,
    library_id: String,
    file_name: String,
    media_type: String,
    declared_bytes: u64,
    received_bytes: u64,
    state: &'static str,
    status_url: String,
    error_code: Option<String>,
    item_id: Option<String>,
}

impl From<UploadSession> for UploadResponse {
    fn from(upload: UploadSession) -> Self {
        Self {
            upload_id: upload.upload_id.as_uuid().to_string(),
            library_id: upload.library_id.as_uuid().to_string(),
            file_name: upload.file_name,
            media_type: upload.media_type,
            declared_bytes: upload.declared_bytes.get(),
            received_bytes: upload.received_bytes.get(),
            state: upload.state.as_str(),
            status_url: format!(
                "/api/v1/libraries/{}/uploads/{}",
                upload.library_id.as_uuid(),
                upload.upload_id.as_uuid()
            ),
            error_code: upload.error_code,
            item_id: upload.item_id.map(|item| item.as_uuid().to_string()),
        }
    }
}

async fn create_upload(
    State(state): State<AppState>,
    Extension(context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path(raw_library): Path<String>,
    Json(body): Json<CreateBody>,
) -> Response {
    let library_id = match library_id(&raw_library) {
        Ok(value) => value,
        Err(error) => return problem_response(&error, &context),
    };
    if body.declared_bytes == 0 || body.declared_bytes > MAX_UPLOAD_BYTES {
        return problem_response(&AppError::PayloadTooLarge, &context);
    }
    match state
        .upload_api
        .create_upload(CreateUploadRequest {
            actor: actor.user_id,
            request_id,
            library_id,
            file_name: body.file_name,
            media_type: body.media_type,
            declared_bytes: body.declared_bytes,
        })
        .await
    {
        Ok(upload) => accepted(upload),
        Err(error) => problem_response(&error, &context),
    }
}

async fn get_upload(
    State(state): State<AppState>,
    Extension(context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path((raw_library, raw_upload)): Path<(String, String)>,
) -> Response {
    let (library_id, upload_id) = match ids(&raw_library, &raw_upload) {
        Ok(value) => value,
        Err(error) => return problem_response(&error, &context),
    };
    match state
        .upload_api
        .get_upload(GetUploadRequest {
            actor: actor.user_id,
            request_id,
            library_id,
            upload_id,
        })
        .await
    {
        Ok(upload) => Json(UploadResponse::from(upload)).into_response(),
        Err(error) => problem_response(&error, &context),
    }
}

async fn receive_upload(
    State(state): State<AppState>,
    Extension(context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path((raw_library, raw_upload)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let (library_id, upload_id) = match ids(&raw_library, &raw_upload) {
        Ok(value) => value,
        Err(error) => return problem_response(&error, &context),
    };
    let media = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or("");
    if !matches!(media, "application/epub+zip" | "application/octet-stream") {
        return problem_response(
            &invalid("content_type", "unsupported_upload_media_type"),
            &context,
        );
    }
    let stream = body
        .into_data_stream()
        .map(|result| result.map_err(|_| UploadStreamError));
    match state
        .upload_api
        .receive_upload(ReceiveUploadRequest {
            actor: actor.user_id,
            request_id,
            library_id,
            upload_id,
            bytes: Box::pin(stream),
        })
        .await
    {
        Ok(upload) => {
            if let Ok(attributes) = MetricAttributes::try_new([("outcome", "accepted")]) {
                TelemetryMetrics.record_upload_bytes(upload.received_bytes.get(), &attributes);
            }
            accepted(upload)
        }
        Err(error) => problem_response(&error, &context),
    }
}

fn accepted(upload: UploadSession) -> Response {
    let value = UploadResponse::from(upload);
    let location = HeaderValue::from_str(&value.status_url).ok();
    let mut response = (StatusCode::ACCEPTED, Json(value)).into_response();
    if let Some(location) = location {
        response.headers_mut().insert("location", location);
    }
    response
}
fn library_id(raw: &str) -> Result<LibraryId, AppError> {
    uuid::Uuid::parse_str(raw)
        .map(LibraryId::from_uuid)
        .map_err(|_| invalid("library_id", "invalid_library_id"))
}
fn ids(library: &str, upload: &str) -> Result<(LibraryId, UploadId), AppError> {
    Ok((
        library_id(library)?,
        uuid::Uuid::parse_str(upload)
            .map(UploadId::from_uuid)
            .map_err(|_| invalid("upload_id", "invalid_upload_id"))?,
    ))
}
fn invalid(field: &'static str, code: &'static str) -> AppError {
    AppError::Invalid {
        code,
        fields: vec![FieldViolation { field, code }],
    }
}
