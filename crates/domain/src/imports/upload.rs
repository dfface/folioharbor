use crate::{
    id::{LibraryId, UploadId},
    imports::{blob::StorageKey, quota::ByteCount},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UploadState {
    Created,
    Receiving,
    Received,
    Queued,
    Validating,
    Importing,
    Ready,
    Duplicate,
    Failed,
    Expired,
    RetryWait,
    OperatorRequired,
}

impl UploadState {
    pub const ALL: [Self; 12] = [
        Self::Created,
        Self::Receiving,
        Self::Received,
        Self::Queued,
        Self::Validating,
        Self::Importing,
        Self::Ready,
        Self::Duplicate,
        Self::Failed,
        Self::Expired,
        Self::RetryWait,
        Self::OperatorRequired,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Receiving => "receiving",
            Self::Received => "received",
            Self::Queued => "queued",
            Self::Validating => "validating",
            Self::Importing => "importing",
            Self::Ready => "ready",
            Self::Duplicate => "duplicate",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::RetryWait => "retry_wait",
            Self::OperatorRequired => "operator_required",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == value)
    }

    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Created, Self::Receiving | Self::Expired)
                | (Self::Receiving, Self::Received | Self::Failed)
                | (
                    Self::Received | Self::RetryWait | Self::OperatorRequired,
                    Self::Queued
                )
                | (Self::Queued, Self::Validating)
                | (
                    Self::Validating,
                    Self::Importing | Self::Failed | Self::RetryWait | Self::OperatorRequired
                )
                | (
                    Self::Importing,
                    Self::Ready
                        | Self::Duplicate
                        | Self::Failed
                        | Self::RetryWait
                        | Self::OperatorRequired
                )
                | (Self::Failed, Self::Receiving)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadSession {
    pub upload_id: UploadId,
    pub library_id: LibraryId,
    pub file_name: String,
    pub media_type: String,
    pub declared_bytes: ByteCount,
    pub received_bytes: ByteCount,
    pub state: UploadState,
    pub storage_key: Option<StorageKey>,
    pub error_code: Option<String>,
}
