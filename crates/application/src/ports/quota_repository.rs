use async_trait::async_trait;
use folioharbor_domain::{
    id::{LibraryId, UploadId},
    imports::quota::ByteCount,
    time::OffsetDateTime,
};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaOutcome {
    Applied,
    Exceeded,
    NotActive,
}

#[derive(Debug, Error)]
#[error("quota persistence failed")]
pub struct QuotaRepositoryError;

#[async_trait]
pub trait QuotaRepository: Send + Sync {
    /// Locks the library and creates one active upload reservation while
    /// incrementing its reserved-byte counter in the same transaction.
    async fn reserve(
        &self,
        library_id: LibraryId,
        upload_id: UploadId,
        bytes: ByteCount,
        expires_at: OffsetDateTime,
    ) -> Result<QuotaOutcome, QuotaRepositoryError>;
    /// Locks the library and active reservation, then changes the reservation
    /// and library counter together or leaves both unchanged.
    async fn resize_reservation(
        &self,
        library_id: LibraryId,
        upload_id: UploadId,
        bytes: ByteCount,
    ) -> Result<QuotaOutcome, QuotaRepositoryError>;
    /// Locks the library and active reservation, atomically moving its bytes
    /// from the reserved counter to logical usage exactly once.
    async fn consume(
        &self,
        library_id: LibraryId,
        upload_id: UploadId,
    ) -> Result<QuotaOutcome, QuotaRepositoryError>;
    /// Locks the library and active reservation, atomically returning its
    /// bytes to available quota exactly once.
    async fn release(
        &self,
        library_id: LibraryId,
        upload_id: UploadId,
    ) -> Result<QuotaOutcome, QuotaRepositoryError>;
}
