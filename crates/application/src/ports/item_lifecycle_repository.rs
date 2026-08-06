use async_trait::async_trait;
use folioharbor_domain::{catalog::ItemLifecycle, id::ItemId, time::OffsetDateTime};
use thiserror::Error;

use crate::{audit::AuditEvent, authorization::AuthorizationGrant, error::AppError};

#[derive(Clone, Debug)]
pub struct ItemLifecycleMutation {
    pub grant: AuthorizationGrant,
    pub item_id: ItemId,
    pub now: OffsetDateTime,
    pub audit: AuditEvent,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ItemLifecycleRepositoryError {
    #[error("item was not found")]
    NotFound,
    #[error("authorization changed before the mutation committed")]
    Forbidden,
    #[error("the item recovery window has elapsed")]
    RecoveryWindowElapsed,
    #[error("item lifecycle persistence failed")]
    Persistence,
}

impl From<ItemLifecycleRepositoryError> for AppError {
    fn from(value: ItemLifecycleRepositoryError) -> Self {
        match value {
            ItemLifecycleRepositoryError::NotFound => AppError::NotFound {
                code: "item_not_found",
            },
            ItemLifecycleRepositoryError::Forbidden => AppError::Forbidden {
                code: "library_action_forbidden",
            },
            ItemLifecycleRepositoryError::RecoveryWindowElapsed => AppError::Conflict {
                code: "item_recovery_window_elapsed",
            },
            ItemLifecycleRepositoryError::Persistence => AppError::DependencyUnavailable {
                code: "catalog_repository_unavailable",
            },
        }
    }
}

#[async_trait]
pub trait ItemLifecycleRepository: Send + Sync {
    async fn delete(
        &self,
        mutation: ItemLifecycleMutation,
    ) -> Result<ItemLifecycle, ItemLifecycleRepositoryError>;

    async fn restore(
        &self,
        mutation: ItemLifecycleMutation,
    ) -> Result<ItemLifecycle, ItemLifecycleRepositoryError>;
}
