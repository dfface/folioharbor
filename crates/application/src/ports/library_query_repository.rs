use async_trait::async_trait;
use folioharbor_domain::id::{LibraryId, UserId};
use thiserror::Error;

use crate::{
    authorization::AuthorizationGrant,
    libraries::{LibraryMemberView, LibraryView},
};

#[derive(Debug, Error)]
#[error("library query persistence failed")]
pub struct LibraryQueryRepositoryError;

#[async_trait]
pub trait LibraryQueryRepository: Send + Sync {
    async fn list_visible(
        &self,
        actor: UserId,
    ) -> Result<Vec<LibraryView>, LibraryQueryRepositoryError>;
    async fn get_library(
        &self,
        grant: AuthorizationGrant,
        library: LibraryId,
    ) -> Result<Option<LibraryView>, LibraryQueryRepositoryError>;
    async fn list_members(
        &self,
        grant: AuthorizationGrant,
        library: LibraryId,
    ) -> Result<Vec<LibraryMemberView>, LibraryQueryRepositoryError>;
}
