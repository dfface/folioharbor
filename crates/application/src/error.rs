use std::{fmt, time::Duration};

use folioharbor_domain::id::ErrorId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldViolation {
    pub field: &'static str,
    pub code: &'static str,
}

#[derive(Debug)]
pub enum AppError {
    Unauthenticated,
    Forbidden {
        code: &'static str,
    },
    NotFound {
        code: &'static str,
    },
    Conflict {
        code: &'static str,
    },
    Invalid {
        code: &'static str,
        fields: Vec<FieldViolation>,
    },
    PayloadTooLarge,
    RateLimited {
        retry_after: Duration,
    },
    StorageExhausted,
    DependencyUnavailable {
        code: &'static str,
    },
    Internal {
        error_id: ErrorId,
    },
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("application request failed")
    }
}

impl std::error::Error for AppError {}
