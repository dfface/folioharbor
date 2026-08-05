use crate::{
    id::{JobId, LibraryId},
    time::OffsetDateTime,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobKind {
    ImportEpub,
}

impl JobKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImportEpub => "import_epub",
        }
    }
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        (value == "import_epub").then_some(Self::ImportEpub)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    Pending,
    Leased,
    RetryWait,
    Succeeded,
    Failed,
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
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobInput {
    pub version: u16,
    pub upload_id: String,
}

impl JobInput {
    #[must_use]
    pub fn upload_v1(upload_id: impl Into<String>) -> Self {
        Self {
            version: 1,
            upload_id: upload_id.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeasedJob {
    pub job_id: JobId,
    pub library_id: LibraryId,
    pub kind: JobKind,
    pub input: JobInput,
    pub attempt: u32,
    pub lease_expires_at: OffsetDateTime,
}
