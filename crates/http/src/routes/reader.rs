use axum::{
    Json, Router,
    body::Body,
    extract::{Extension, Path, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{
            CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, IF_NONE_MATCH,
            REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
        },
    },
    response::{IntoResponse, Response},
    routing::get,
};
use folioharbor_application::{
    error::{AppError, FieldViolation},
    reader::{ManifestLink, ManifestMetadata, PublicationManifest, ResourceId, ResourceResponse},
};
use folioharbor_domain::id::{ItemId, RequestId};
use serde::Serialize;
use uuid::Uuid;

use super::AppState;
use crate::{
    auth::AuthenticatedActor,
    problem::{ProblemContext, response as problem_response},
};

const CONTENT_POLICY: &str = "default-src 'none'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; font-src 'self' data: blob:; script-src 'none'; form-action 'none'; frame-src 'none'";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{item_id}/manifest", get(manifest))
        .route("/{item_id}/resources/{resource_id}", get(resource))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestResponse {
    metadata: ManifestMetadataResponse,
    manifestation_id: String,
    reading_order: Vec<LinkResponse>,
    resources: Vec<LinkResponse>,
    toc: Vec<LinkResponse>,
    links: Vec<LinkResponse>,
}

#[derive(Serialize)]
struct ManifestMetadataResponse {
    title: String,
    authors: Vec<String>,
    languages: Vec<String>,
}

#[derive(Serialize)]
struct LinkResponse {
    href: String,
    #[serde(rename = "type")]
    media_type: String,
    #[serde(rename = "rel", skip_serializing_if = "String::is_empty")]
    relation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

async fn manifest(
    State(state): State<AppState>,
    Extension(context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path(raw_item): Path<String>,
    headers: HeaderMap,
) -> Response {
    let item = match parse_item(&raw_item) {
        Ok(item) => item,
        Err(error) => return problem_response(&error, &context),
    };
    match state
        .reader_api
        .get_manifest(actor.user_id, item, request_id)
        .await
    {
        Ok(value) => manifest_response(value, &headers),
        Err(error) => problem_response(&error, &context),
    }
}

async fn resource(
    State(state): State<AppState>,
    Extension(context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path((raw_item, raw_resource)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let item = match parse_item(&raw_item) {
        Ok(item) => item,
        Err(error) => return problem_response(&error, &context),
    };
    let Ok(resource_id) = ResourceId::parse(&raw_resource) else {
        return problem_response(
            &invalid_identifier("resource_id", "invalid_resource_id"),
            &context,
        );
    };
    match state
        .reader_api
        .get_resource(actor.user_id, item, resource_id, request_id)
        .await
    {
        Ok(value) => resource_response(value, &headers),
        Err(error) => problem_response(&error, &context),
    }
}

fn manifest_response(value: PublicationManifest, headers: &HeaderMap) -> Response {
    if matches_validator(headers, &value.etag) {
        return not_modified(&value.etag);
    }
    let etag = value.etag.clone();
    let response = ManifestResponse {
        metadata: metadata(value.metadata),
        manifestation_id: value.manifestation_id.as_uuid().to_string(),
        reading_order: value.reading_order.into_iter().map(link).collect(),
        resources: value.resources.into_iter().map(link).collect(),
        toc: value.toc.into_iter().map(link).collect(),
        links: value.links.into_iter().map(link).collect(),
    };
    with_validator(Json(response).into_response(), &etag)
}

fn resource_response(value: ResourceResponse, headers: &HeaderMap) -> Response {
    if matches_validator(headers, &value.etag) {
        return not_modified(&value.etag);
    }
    let mut response = Response::new(Body::from(value.bytes));
    *response.status_mut() = StatusCode::OK;
    if let Ok(content_type) = HeaderValue::from_str(&value.media_type) {
        response.headers_mut().insert(CONTENT_TYPE, content_type);
    }
    isolation_headers(response.headers_mut());
    with_validator(response, &value.etag)
}

fn metadata(value: ManifestMetadata) -> ManifestMetadataResponse {
    ManifestMetadataResponse {
        title: value.title,
        authors: value.authors,
        languages: value.languages,
    }
}
fn link(value: ManifestLink) -> LinkResponse {
    LinkResponse {
        href: value.href,
        media_type: value.media_type,
        relation: value.relation,
        title: value.title,
    }
}

fn with_validator(mut response: Response, etag: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(ETAG, value);
    }
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    response
}

fn isolation_headers(headers: &mut HeaderMap) {
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_POLICY),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
}

fn matches_validator(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|candidate| candidate.trim() == etag))
}

fn not_modified(etag: &str) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_MODIFIED;
    with_validator(response, etag)
}

fn parse_item(raw: &str) -> Result<ItemId, AppError> {
    Uuid::parse_str(raw)
        .map(ItemId::from_uuid)
        .map_err(|_| invalid_identifier("item_id", "invalid_uuid"))
}
fn invalid_identifier(field: &'static str, code: &'static str) -> AppError {
    AppError::BadRequest {
        code: "invalid_identifier",
        fields: vec![FieldViolation { field, code }],
    }
}
