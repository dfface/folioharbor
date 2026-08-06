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
pub struct DeleteItemCommand {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub item_id: ItemId,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}

pub struct DeleteItem<'a, R: ?Sized, A: ?Sized> {
    repository: &'a R,
    authorization: &'a A,
}

impl<'a, R: ?Sized, A: ?Sized> DeleteItem<'a, R, A> {
    #[must_use]
    pub const fn new(repository: &'a R, authorization: &'a A) -> Self {
        Self {
            repository,
            authorization,
        }
    }
}

impl<R: ItemLifecycleRepository + ?Sized, A: AuthorizationRepository + ?Sized>
    DeleteItem<'_, R, A>
{
    /// Soft-deletes an Item after a versioned `holding.edit` grant is resolved.
    ///
    /// # Errors
    /// Returns anti-enumerating authorization, lifecycle conflict, or persistence errors.
    pub async fn execute(&self, command: DeleteItemCommand) -> Result<ItemLifecycle, AppError> {
        let resource = ResourceRef::Item {
            library_id: command.library_id,
            item_id: command.item_id,
        };
        let grant = Authorization::new(self.authorization)
            .require(command.actor, Action::DeleteItem, resource)
            .await?;
        let audit = AuditEvent::allowed(
            command.actor,
            Action::DeleteItem,
            resource,
            command.request_id,
            command.now,
        );
        self.repository
            .delete(ItemLifecycleMutation {
                grant,
                item_id: command.item_id,
                now: command.now,
                audit,
            })
            .await
            .map_err(Into::into)
    }
}
