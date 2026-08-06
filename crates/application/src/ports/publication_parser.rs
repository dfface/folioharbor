use async_trait::async_trait;
use folioharbor_domain::{catalog::CatalogPublication, imports::blob::StorageKey};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum PublicationParserError {
    #[error("publication is malformed")]
    Malformed,
    #[error("publication bytes are temporarily unavailable")]
    Unavailable,
    #[error("publication parser configuration is invalid")]
    Configuration,
    #[error("publication storage capacity is exhausted")]
    Capacity,
}

#[async_trait]
pub trait PublicationParser: Send + Sync {
    fn profile_version(&self) -> &'static str;
    async fn parse(&self, key: &StorageKey) -> Result<CatalogPublication, PublicationParserError>;
}
