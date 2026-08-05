use async_trait::async_trait;
use folioharbor_domain::id::UserId;
use thiserror::Error;

use crate::authorization::{Action, AuthorizationFact, ResourceRef};

#[derive(Debug, Error)]
#[error("authorization persistence failed")]
pub struct AuthorizationRepositoryError;

#[async_trait]
pub trait AuthorizationRepository: Send + Sync {
    async fn resolve(
        &self,
        actor: UserId,
        action: Action,
        resource: ResourceRef,
    ) -> Result<Option<AuthorizationFact>, AuthorizationRepositoryError>;
}
