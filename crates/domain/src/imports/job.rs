use crate::{
    id::{JobId, LibraryId},
    time::OffsetDateTime,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobKind {
    ImportEpub,
    ExpireUploadsAndReservations,
    PurgeFailedUploads,
    CollectBlobsLater,
}

impl JobKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImportEpub => "import_epub",
            Self::ExpireUploadsAndReservations => "expire_uploads_and_reservations",
            Self::PurgeFailedUploads => "purge_failed_uploads",
            Self::CollectBlobsLater => "collect_blobs_later",
        }
    }
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "import_epub" => Some(Self::ImportEpub),
            "expire_uploads_and_reservations" => Some(Self::ExpireUploadsAndReservations),
            "purge_failed_uploads" => Some(Self::PurgeFailedUploads),
            "collect_blobs_later" => Some(Self::CollectBlobsLater),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    Pending,
    Leased,
    RetryWait,
    Succeeded,
    Failed,
    OperatorRequired,
}

impl JobState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Leased => "leased",
            Self::RetryWait => "retry_wait",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::OperatorRequired => "operator_required",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobInput {
    ImportEpubV1 { upload_id: String },
    CleanupV1,
}

impl JobInput {
    #[must_use]
    pub fn upload_v1(upload_id: impl Into<String>) -> Self {
        Self::ImportEpubV1 {
            upload_id: upload_id.into(),
        }
    }

    #[must_use]
    pub fn cleanup_v1() -> Self {
        Self::CleanupV1
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeasedJob {
    pub job_id: JobId,
    pub library_id: Option<LibraryId>,
    pub kind: JobKind,
    pub input: JobInput,
    pub attempt: u32,
    pub lease_expires_at: OffsetDateTime,
}
