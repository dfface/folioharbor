use folioharbor_application::error::{AppError, FieldViolation};
use serde::Serialize;
use url::Url;

pub const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProblemContext {
    problem_base: String,
    request_id: String,
}

impl ProblemContext {
    #[must_use]
    pub fn new(public_base_url: &Url, request_id: impl Into<String>) -> Self {
        Self {
            problem_base: format!(
                "{}/problems/",
                public_base_url.as_str().trim_end_matches('/')
            ),
            request_id: request_id.into(),
        }
    }

    #[must_use]
    pub fn example(request_id: impl Into<String>) -> Self {
        Self {
            problem_base: "https://library.example/problems/".to_owned(),
            request_id: request_id.into(),
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
        let fields = match error {
            AppError::Invalid { fields, .. } => {
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
            instance: format!("/problems/{}", context.request_id),
            code: mapping.code,
            request_id: context.request_id.clone(),
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
