use folioharbor_domain::{
    catalog::ItemLifecycle,
    id::{ItemId, LibraryId, RequestId, UserId},
    time::OffsetDateTime,
};

use crate::{
    audit::AuditEvent,
    authorization::{Action, Authorization, ResourceRef},
    error::AppError,
    ports::{AuthorizationRepository, ItemLifecycleMutation, ItemLifecycleRepository},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RestoreItemCommand {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub item_id: ItemId,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}

pub struct RestoreItem<'a, R: ?Sized, A: ?Sized> {
    repository: &'a R,
    authorization: &'a A,
}

impl<'a, R: ?Sized, A: ?Sized> RestoreItem<'a, R, A> {
    #[must_use]
    pub const fn new(repository: &'a R, authorization: &'a A) -> Self {
        Self {
            repository,
            authorization,
        }
    }
}

impl<R: ItemLifecycleRepository + ?Sized, A: AuthorizationRepository + ?Sized>
    RestoreItem<'_, R, A>
{
    /// Restores a soft-deleted Item before its seven-day recovery window closes.
    ///
    /// # Errors
    /// Returns anti-enumerating authorization, lifecycle conflict, or persistence errors.
    pub async fn execute(&self, command: RestoreItemCommand) -> Result<ItemLifecycle, AppError> {
        let resource = ResourceRef::Item {
            library_id: command.library_id,
            item_id: command.item_id,
        };
        let grant = Authorization::new(self.authorization)
            .require(command.actor, Action::RestoreItem, resource)
            .await?;
        let audit = AuditEvent::allowed(
            command.actor,
            Action::RestoreItem,
            resource,
            command.request_id,
            command.now,
        );
        self.repository
            .restore(ItemLifecycleMutation {
                grant,
                item_id: command.item_id,
                now: command.now,
                audit,
            })
            .await
            .map_err(Into::into)
    }
}
