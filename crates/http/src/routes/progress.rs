use std::collections::BTreeMap;

use axum::{
    Router,
    extract::{Extension, Path, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{ETAG, IF_MATCH},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use folioharbor_application::{
    error::{AppError, FieldViolation},
    reader::UpdateReadingProgressCommand,
};
use folioharbor_domain::{
    id::{ContentUnitId, DeviceId, ManifestationId, PublicationPackageId, RequestId},
    reader::{
        DeviceReadingState, LocatorExtensionValue, LocatorExtensions, LocatorLocations,
        LocatorText, ReadingProgress, ReadingUpdateOutcome, ReadiumLocator,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::AppState;
use crate::{
    auth::AuthenticatedActor,
    json::ApiJson,
    problem::{ProblemContext, progress_conflict_response, response as problem_response},
};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/{manifestation_id}/progress",
        get(get_progress).put(put_progress),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateBody {
    device_id: String,
    client_mutation_id: String,
    base_version: u64,
    package_id: Option<String>,
    content_unit_id: Option<String>,
    locator: LocatorBody,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocatorBody {
    href: String,
    #[serde(rename = "type")]
    media_type: Option<String>,
    locations: LocationsBody,
    text: Option<TextBody>,
    #[serde(default)]
    extensions: Option<ExtensionsBody>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocationsBody {
    progression: Option<f64>,
    position: Option<u32>,
    total_progression: Option<f64>,
    #[serde(default)]
    fragments: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextBody {
    before: Option<String>,
    highlight: Option<String>,
    after: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtensionsBody {
    version: u16,
    #[serde(default)]
    values: BTreeMap<String, Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressResponse {
    manifestation_id: String,
    package_id: Option<String>,
    content_unit_id: Option<String>,
    locator: LocatorResponse,
    version: u64,
    updated_at: String,
}
#[derive(Serialize)]
struct DeviceResponse {
    #[serde(rename = "deviceId")]
    device_id: String,
    locator: LocatorResponse,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}
#[derive(Serialize)]
struct LocatorResponse {
    href: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    media_type: Option<String>,
    locations: LocationsResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<TextResponse>,
    extensions: ExtensionsResponse,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocationsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    progression: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_progression: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fragments: Vec<String>,
}
#[derive(Serialize)]
struct TextResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    highlight: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
}
#[derive(Serialize)]
struct ExtensionsResponse {
    version: u16,
    values: BTreeMap<String, Value>,
}

async fn get_progress(
    State(state): State<AppState>,
    Extension(context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path(raw): Path<String>,
) -> Response {
    let manifestation = match parse_manifestation(&raw) {
        Ok(v) => v,
        Err(e) => return problem_response(&e, &context),
    };
    match state
        .progress_api
        .get_progress(actor.user_id, manifestation, request_id)
        .await
    {
        Ok(Some(progress)) => state_response(progress),
        Ok(None) => with_etag(StatusCode::NO_CONTENT.into_response(), 0),
        Err(error) => problem_response(&error, &context),
    }
}
async fn put_progress(
    State(state): State<AppState>,
    Extension(context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path(raw): Path<String>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<UpdateBody>,
) -> Response {
    let manifestation = match parse_manifestation(&raw) {
        Ok(v) => v,
        Err(e) => return problem_response(&e, &context),
    };
    let header_version = match parse_if_match(&headers) {
        Ok(v) => v,
        Err(e) => return problem_response(&e, &context),
    };
    if header_version != body.base_version {
        return problem_response(&invalid("base_version", "if_match_mismatch"), &context);
    }
    let device_id = match parse_uuid(&body.device_id, "device_id") {
        Ok(v) => DeviceId::from_uuid(v),
        Err(e) => return problem_response(&e, &context),
    };
    let client_mutation_id = match parse_uuid(&body.client_mutation_id, "client_mutation_id") {
        Ok(v) => v,
        Err(e) => return problem_response(&e, &context),
    };
    let package_id = match body
        .package_id
        .as_deref()
        .map(|v| parse_uuid(v, "package_id"))
        .transpose()
    {
        Ok(v) => v.map(PublicationPackageId::from_uuid),
        Err(e) => return problem_response(&e, &context),
    };
    let content_unit_id = match body
        .content_unit_id
        .as_deref()
        .map(|v| parse_uuid(v, "content_unit_id"))
        .transpose()
    {
        Ok(v) => v.map(ContentUnitId::from_uuid),
        Err(e) => return problem_response(&e, &context),
    };
    let locator = match body.locator.try_into() {
        Ok(v) => v,
        Err(e) => return problem_response(&e, &context),
    };
    let command = UpdateReadingProgressCommand {
        actor: actor.user_id,
        manifestation_id: manifestation,
        device_id,
        client_mutation_id,
        base_version: body.base_version,
        package_id,
        content_unit_id,
        locator,
        request_id,
    };
    match state.progress_api.update_progress(command).await {
        Ok(ReadingUpdateOutcome::Updated { global, .. }) => state_response(global),
        Ok(ReadingUpdateOutcome::Conflict { global, device }) => {
            let version = global.version;
            let global_json =
                serde_json::to_value(ProgressResponse::from(global)).unwrap_or(Value::Null);
            let device_json =
                serde_json::to_value(DeviceResponse::from(device)).unwrap_or(Value::Null);
            with_etag(
                progress_conflict_response(&context, &global_json, &device_json),
                version,
            )
        }
        Err(error) => problem_response(&error, &context),
    }
}
fn state_response(progress: ReadingProgress) -> Response {
    let version = progress.version;
    with_etag(
        axum::Json(ProgressResponse::from(progress)).into_response(),
        version,
    )
}
fn with_etag(mut response: Response, version: u64) -> Response {
    if let Ok(value) = HeaderValue::from_str(&format!("\"progress-v{version}\"")) {
        response.headers_mut().insert(ETAG, value);
    }
    response
}
fn parse_if_match(headers: &HeaderMap) -> Result<u64, AppError> {
    let raw = headers
        .get(IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| invalid("if_match", "required"))?;
    raw.strip_prefix("\"progress-v")
        .and_then(|v| v.strip_suffix('"'))
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| invalid("if_match", "invalid_progress_etag"))
}
fn parse_manifestation(raw: &str) -> Result<ManifestationId, AppError> {
    Uuid::parse_str(raw)
        .map(ManifestationId::from_uuid)
        .map_err(|_| invalid("manifestation_id", "invalid_uuid"))
}
fn parse_uuid(raw: &str, field: &'static str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw).map_err(|_| invalid(field, "invalid_uuid"))
}
fn invalid(field: &'static str, code: &'static str) -> AppError {
    AppError::BadRequest {
        code: "invalid_progress_request",
        fields: vec![FieldViolation { field, code }],
    }
}

impl TryFrom<LocatorBody> for ReadiumLocator {
    type Error = AppError;
    fn try_from(value: LocatorBody) -> Result<Self, Self::Error> {
        let extensions = value.extensions.unwrap_or(ExtensionsBody {
            version: 1,
            values: BTreeMap::new(),
        });
        let values = extensions
            .values
            .into_iter()
            .map(|(key, value)| {
                let parsed = if let Some(v) = value.as_bool() {
                    LocatorExtensionValue::Boolean(v)
                } else if let Some(v) = value.as_i64() {
                    LocatorExtensionValue::Integer(v)
                } else if let Some(v) = value.as_f64() {
                    LocatorExtensionValue::Number(v)
                } else if let Some(v) = value.as_str() {
                    LocatorExtensionValue::String(v.to_owned())
                } else {
                    return Err(invalid("locator.extensions", "unsupported_value"));
                };
                Ok((key, parsed))
            })
            .collect::<Result<BTreeMap<_, _>, AppError>>()?;
        let locations = LocatorLocations::new(
            value.locations.progression,
            value.locations.position,
            value.locations.total_progression,
            value.locations.fragments,
        )
        .map_err(|e| invalid("locator.locations", e.code()))?;
        let text = value
            .text
            .map(|v| {
                LocatorText::new(v.before, v.highlight, v.after)
                    .map_err(|e| invalid("locator.text", e.code()))
            })
            .transpose()?;
        ReadiumLocator::new(
            value.href,
            value.media_type,
            locations,
            text,
            LocatorExtensions::new(extensions.version, values)
                .map_err(|e| invalid("locator.extensions", e.code()))?,
        )
        .map_err(|e| invalid("locator", e.code()))
    }
}
impl From<ReadingProgress> for ProgressResponse {
    fn from(v: ReadingProgress) -> Self {
        Self {
            manifestation_id: v.manifestation_id.as_uuid().to_string(),
            package_id: v.package_id.map(|id| id.as_uuid().to_string()),
            content_unit_id: v.content_unit_id.map(|id| id.as_uuid().to_string()),
            locator: LocatorResponse::from(v.locator),
            version: v.version,
            updated_at: v.updated_at.to_string(),
        }
    }
}
impl From<DeviceReadingState> for DeviceResponse {
    fn from(v: DeviceReadingState) -> Self {
        Self {
            device_id: v.device_id.as_uuid().to_string(),
            locator: LocatorResponse::from(v.locator),
            updated_at: v.updated_at.to_string(),
        }
    }
}
impl From<ReadiumLocator> for LocatorResponse {
    fn from(v: ReadiumLocator) -> Self {
        let values = v
            .extensions()
            .values()
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    match v {
                        LocatorExtensionValue::Boolean(v) => serde_json::json!(v),
                        LocatorExtensionValue::Integer(v) => serde_json::json!(v),
                        LocatorExtensionValue::Number(v) => serde_json::json!(v),
                        LocatorExtensionValue::String(v) => serde_json::json!(v),
                    },
                )
            })
            .collect();
        Self {
            href: v.href().to_owned(),
            media_type: v.media_type().map(str::to_owned),
            locations: LocationsResponse {
                progression: v.locations().progression(),
                position: v.locations().position(),
                total_progression: v.locations().total_progression(),
                fragments: v.locations().fragments().to_vec(),
            },
            text: v.text().map(|t| TextResponse {
                before: t.before().map(str::to_owned),
                highlight: t.highlight().map(str::to_owned),
                after: t.after().map(str::to_owned),
            }),
            extensions: ExtensionsResponse {
                version: v.extensions().version(),
                values,
            },
        }
    }
}
