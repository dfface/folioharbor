use axum::{
    http::{Extensions, HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use folioharbor_application::error::{AppError, FieldViolation};
use folioharbor_domain::id::RequestId;
use serde::Serialize;
use url::Url;

pub const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProblemContext {
    problem_base: String,
    request_id: ProblemRequestId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProblemRequestId {
    Domain(RequestId),
    Example(String),
}

impl ProblemRequestId {
    fn to_public_string(&self) -> String {
        match self {
            Self::Domain(request_id) => request_id.as_ulid().to_string(),
            Self::Example(request_id) => request_id.clone(),
        }
    }
}

impl ProblemContext {
    #[must_use]
    pub fn new(public_base_url: &Url, request_id: RequestId) -> Self {
        Self {
            problem_base: format!(
                "{}/problems/",
                public_base_url.as_str().trim_end_matches('/')
            ),
            request_id: ProblemRequestId::Domain(request_id),
        }
    }

    #[must_use]
    pub fn example(request_id: impl Into<String>) -> Self {
        Self {
            problem_base: "https://library.example/problems/".to_owned(),
            request_id: ProblemRequestId::Example(request_id.into()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProblemTypeUri(String);

impl ProblemTypeUri {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProblemFieldViolation {
    pub field: &'static str,
    pub code: &'static str,
}

impl From<&FieldViolation> for ProblemFieldViolation {
    fn from(violation: &FieldViolation) -> Self {
        Self {
            field: violation.field,
            code: violation.code,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub type_uri: ProblemTypeUri,
    pub title: &'static str,
    pub status: u16,
    pub detail: &'static str,
    pub instance: String,
    pub code: &'static str,
    pub request_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<ProblemFieldViolation>,
}

impl ProblemDetails {
    #[must_use]
    pub fn from_app_error(error: &AppError, context: &ProblemContext) -> Self {
        let mapping = ProblemMapping::from(error);
        let request_id = context.request_id.to_public_string();
        let fields = match error {
            AppError::BadRequest { fields, .. } | AppError::Invalid { fields, .. } => {
                fields.iter().map(ProblemFieldViolation::from).collect()
            }
            _ => Vec::new(),
        };
        Self {
            type_uri: ProblemTypeUri(format!(
                "{}{}",
                context.problem_base,
                mapping.code.replace('_', "-")
            )),
            title: mapping.title,
            status: mapping.status,
            detail: mapping.detail,
            instance: format!("/problems/{request_id}"),
            code: mapping.code,
            request_id,
            fields,
        }
    }

    /// Serializes this route-independent problem document as JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if JSON serialization fails.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

struct ProblemMapping {
    status: u16,
    code: &'static str,
    title: &'static str,
    detail: &'static str,
}

impl From<&AppError> for ProblemMapping {
    fn from(error: &AppError) -> Self {
        match error {
            AppError::Unauthenticated => Self::new(
                401,
                "unauthenticated",
                "Authentication required",
                "Authentication is required to complete this request.",
            ),
            AppError::Forbidden { code } => Self::new(
                403,
                code,
                "Action forbidden",
                "This action is not permitted.",
            ),
            AppError::NotFound { code } => Self::new(
                404,
                code,
                "Resource not found",
                "The requested resource was not found.",
            ),
            AppError::Conflict { code } => Self::new(
                409,
                code,
                "Request conflict",
                "The request conflicts with the current resource state.",
            ),
            AppError::BadRequest { code, .. } => Self::new(
                400,
                code,
                "Malformed request",
                "The request contains a malformed identifier.",
            ),
            AppError::Invalid { code, .. } => Self::new(
                422,
                code,
                "Invalid request",
                "The request contains invalid values.",
            ),
            AppError::PayloadTooLarge => Self::new(
                413,
                "payload_too_large",
                "Payload too large",
                "The request payload exceeds the configured limit.",
            ),
            AppError::RateLimited { .. } => Self::new(
                429,
                "rate_limited",
                "Too many requests",
                "The request rate limit has been exceeded.",
            ),
            AppError::StorageExhausted => Self::new(
                507,
                "storage_exhausted",
                "Storage exhausted",
                "The service does not have enough physical storage.",
            ),
            AppError::DependencyUnavailable { code } => Self::new(
                503,
                code,
                "Dependency unavailable",
                "A required service is temporarily unavailable.",
            ),
            AppError::Internal { .. } => Self::new(
                500,
                "internal_error",
                "Internal server error",
                "An unexpected error occurred.",
            ),
        }
    }
}

impl ProblemMapping {
    const fn new(
        status: u16,
        code: &'static str,
        title: &'static str,
        detail: &'static str,
    ) -> Self {
        Self {
            status,
            code,
            title,
            detail,
        }
    }
}

#[must_use]
pub fn response(error: &AppError, context: &ProblemContext) -> Response {
    let problem = ProblemDetails::from_app_error(error, context);
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (status, axum::Json(problem)).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(PROBLEM_CONTENT_TYPE));
    if let AppError::RateLimited { retry_after } = error {
        if let Ok(value) = HeaderValue::from_str(&retry_after.as_secs().max(1).to_string()) {
            response.headers_mut().insert("Retry-After", value);
        }
    }
    response
}

#[must_use]
pub(crate) fn progress_conflict_response(
    context: &ProblemContext,
    global: &serde_json::Value,
    device: &serde_json::Value,
) -> Response {
    let request_id = context.request_id.to_public_string();
    let problem = serde_json::json!({
        "type": format!("{}progress-conflict", context.problem_base),
        "title": "Reading progress conflict",
        "status": 409,
        "detail": "The global reading position changed before this device update.",
        "instance": format!("/problems/{request_id}"),
        "code": "progress_conflict",
        "request_id": request_id,
        "global": global,
        "device": device,
    });
    let mut response = (StatusCode::CONFLICT, axum::Json(problem)).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(PROBLEM_CONTENT_TYPE));
    response
}

#[must_use]
pub(crate) fn request_validation_response(
    status: StatusCode,
    code: &'static str,
    context: &ProblemContext,
) -> Response {
    let request_id = context.request_id.to_public_string();
    let problem = ProblemDetails {
        type_uri: ProblemTypeUri(format!(
            "{}{}",
            context.problem_base,
            code.replace('_', "-")
        )),
        title: "Invalid request body",
        status: status.as_u16(),
        detail: "The request body could not be accepted as JSON for this operation.",
        instance: format!("/problems/{request_id}"),
        code,
        request_id,
        fields: Vec::new(),
    };
    let mut response = (status, axum::Json(problem)).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(PROBLEM_CONTENT_TYPE));
    response
}

#[must_use]
pub(crate) fn invalid_session_id_response(context: &ProblemContext) -> Response {
    let request_id = context.request_id.to_public_string();
    let problem = ProblemDetails {
        type_uri: ProblemTypeUri(format!("{}invalid-session-id", context.problem_base)),
        title: "Invalid path parameter",
        status: StatusCode::BAD_REQUEST.as_u16(),
        detail: "The session_id path parameter must be a UUID.",
        instance: format!("/problems/{request_id}"),
        code: "invalid_session_id",
        request_id,
        fields: Vec::new(),
    };
    let mut response = (StatusCode::BAD_REQUEST, axum::Json(problem)).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(PROBLEM_CONTENT_TYPE));
    response
}

#[must_use]
pub fn response_from_extensions(
    extensions: &Extensions,
    public_base_url: &Url,
    error: &AppError,
) -> Response {
    let request_id = extensions
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::new);
    response(error, &ProblemContext::new(public_base_url, request_id))
}
