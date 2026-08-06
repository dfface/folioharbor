use async_trait::async_trait;
use folioharbor_domain::{
    id::{BlobId, ItemId, RequestId},
    imports::blob::StorageKey,
};
use std::fmt;
use thiserror::Error;

use crate::actor::Actor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadRange {
    pub start: u64,
    pub end: u64,
}

pub enum DownloadAuthorization {
    Granted,
    Forbidden,
    NotFound,
}

impl fmt::Debug for DownloadAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Granted => "Granted",
            Self::Forbidden => "Forbidden",
            Self::NotFound => "NotFound",
        })
    }
}

/// Receives persistence-only metadata after the repository has authorized a download.
///
/// This keeps the concrete secret carrier private to the application use case.
pub trait DownloadSourceReceiver: Send {
    fn receive(
        &mut self,
        blob_id: BlobId,
        storage_identity: StorageKey,
        byte_size: u64,
        file_name: String,
    );
}

#[derive(Debug, Error)]
#[error("download repository failed")]
pub struct DownloadRepositoryError;

#[async_trait]
pub trait DownloadRepository: Send + Sync {
    async fn authorize_download(
        &self,
        actor: Actor,
        item_id: ItemId,
        request_id: RequestId,
        source: &mut dyn DownloadSourceReceiver,
    ) -> Result<DownloadAuthorization, DownloadRepositoryError>;

    async fn record_download_start(
        &self,
        _actor: Actor,
        _item_id: ItemId,
        _request_id: RequestId,
        _range: DownloadRange,
    ) -> Result<bool, DownloadRepositoryError> {
        Err(DownloadRepositoryError)
    }
}
