use folioharbor_domain::id::{ItemId, LibraryId, ManifestationId, RequestId, UserId};

use crate::{
    authorization::{Action, Authorization, ResourceRef},
    error::AppError,
    ports::{AuthorizationRepository, CatalogQueryRepository, VisibleCatalogItem},
};

use super::list_library_books::capabilities;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemDetail {
    pub item_id: ItemId,
    pub manifestation_id: ManifestationId,
    pub primary_title: String,
    pub authors: Vec<String>,
    pub languages: Vec<String>,
    pub identifiers: Vec<String>,
    pub media_type: String,
    pub can_read: bool,
    pub can_download: bool,
    pub etag: String,
}

pub struct GetItem<'a, R: ?Sized, A: ?Sized> {
    repository: &'a R,
    authorization: &'a A,
    reader_download_enabled: bool,
}

impl<'a, R: ?Sized, A: ?Sized> GetItem<'a, R, A> {
    #[must_use]
    pub const fn new(
        repository: &'a R,
        authorization: &'a A,
        reader_download_enabled: bool,
    ) -> Self {
        Self {
            repository,
            authorization,
            reader_download_enabled,
        }
    }
}

impl<R: CatalogQueryRepository + ?Sized, A: AuthorizationRepository + ?Sized> GetItem<'_, R, A> {
    /// Resolves detail only through a visible Holding.
    ///
    /// # Errors
    /// Returns anti-enumerating not-found or dependency errors.
    pub async fn execute(
        &self,
        actor: UserId,
        library_id: LibraryId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<ItemDetail, AppError> {
        let grant = Authorization::new(self.authorization)
            .require(actor, Action::ViewLibrary, ResourceRef::Library(library_id))
            .await?;
        let row = self
            .repository
            .find_visible_item(grant, library_id, item_id, request_id)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "catalog_repository_unavailable",
            })?
            .ok_or(AppError::NotFound {
                code: "item_not_found",
            })?;
        Ok(detail(
            row,
            grant.role(),
            grant.membership_version(),
            capabilities(grant.role(), self.reader_download_enabled),
            self.reader_download_enabled,
        ))
    }
}

fn detail(
    row: VisibleCatalogItem,
    role: folioharbor_domain::libraries::role::RoleCode,
    membership_version: i64,
    (can_read, can_download): (bool, bool),
    reader_download_enabled: bool,
) -> ItemDetail {
    let etag = format!(
        "W/\"item-{}-r{}-m{membership_version}-c{}{}-d{}\"",
        row.item_id.as_uuid(),
        role.as_str(),
        u8::from(can_read),
        u8::from(can_download),
        u8::from(reader_download_enabled)
    );
    ItemDetail {
        item_id: row.item_id,
        manifestation_id: row.manifestation_id,
        primary_title: row.primary_title,
        authors: row.authors,
        languages: row.languages,
        identifiers: row.identifiers,
        media_type: row.media_type,
        can_read,
        can_download,
        etag,
    }
}
