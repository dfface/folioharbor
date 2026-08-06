use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use folioharbor_domain::id::{HoldingId, ItemId, LibraryId, RequestId, UserId};
use uuid::Uuid;

use crate::{
    authorization::{Action, Authorization, ResourceRef},
    error::{AppError, FieldViolation},
    ports::{AuthorizationRepository, CatalogQueryRepository, VisibleCatalogItem},
};

use super::ItemDetail;

pub const MAX_PAGE_SIZE: u32 = 100;
const DEFAULT_PAGE_SIZE: u32 = 25;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PageRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookSummary {
    pub item_id: ItemId,
    pub primary_title: String,
    pub authors: Vec<String>,
    pub languages: Vec<String>,
    pub media_type: String,
    pub can_read: bool,
    pub can_download: bool,
}

pub struct ListLibraryBooks<'a, R: ?Sized, A: ?Sized> {
    repository: &'a R,
    authorization: &'a A,
}

impl<'a, R: ?Sized, A: ?Sized> ListLibraryBooks<'a, R, A> {
    #[must_use]
    pub const fn new(repository: &'a R, authorization: &'a A) -> Self {
        Self {
            repository,
            authorization,
        }
    }
}

impl<R: CatalogQueryRepository + ?Sized, A: AuthorizationRepository + ?Sized>
    ListLibraryBooks<'_, R, A>
{
    /// Returns one authorized projection per active Holding.
    ///
    /// # Errors
    /// Returns anti-enumerating not-found, invalid-page, or dependency errors.
    pub async fn execute(
        &self,
        actor: UserId,
        library_id: LibraryId,
        request_id: RequestId,
        page: PageRequest,
    ) -> Result<Page<BookSummary>, AppError> {
        let limit = page.limit.unwrap_or(DEFAULT_PAGE_SIZE);
        if limit == 0 {
            return Err(invalid_page("limit", "out_of_range"));
        }
        let limit = limit.min(MAX_PAGE_SIZE);
        let after = page.cursor.as_deref().map(decode_cursor).transpose()?;
        let grant = Authorization::new(self.authorization)
            .require(actor, Action::ViewLibrary, ResourceRef::Library(library_id))
            .await?;
        let mut rows = self
            .repository
            .list_visible_items(grant, library_id, after, limit + 1, request_id)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "catalog_repository_unavailable",
            })?;
        let next_cursor = if rows.len() > limit as usize {
            rows.truncate(limit as usize);
            rows.last().map(|row| encode_cursor(row.holding_id))
        } else {
            None
        };
        Ok(Page {
            items: rows.into_iter().map(summary).collect(),
            next_cursor,
        })
    }
}

#[async_trait]
pub trait CatalogApi: Send + Sync {
    async fn list_library_books(
        &self,
        actor: UserId,
        library_id: LibraryId,
        request_id: RequestId,
        page: PageRequest,
    ) -> Result<Page<BookSummary>, AppError>;
    async fn get_item(
        &self,
        actor: UserId,
        library_id: LibraryId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<ItemDetail, AppError>;
}

pub struct UnavailableCatalogApi;

#[async_trait]
impl CatalogApi for UnavailableCatalogApi {
    async fn list_library_books(
        &self,
        _: UserId,
        _: LibraryId,
        _: RequestId,
        _: PageRequest,
    ) -> Result<Page<BookSummary>, AppError> {
        Err(AppError::DependencyUnavailable {
            code: "catalog_repository_unavailable",
        })
    }

    async fn get_item(
        &self,
        _: UserId,
        _: LibraryId,
        _: ItemId,
        _: RequestId,
    ) -> Result<ItemDetail, AppError> {
        Err(AppError::DependencyUnavailable {
            code: "catalog_repository_unavailable",
        })
    }
}

pub struct CatalogService<R, A> {
    repository: R,
    authorization: A,
}

impl<R, A> CatalogService<R, A> {
    #[must_use]
    pub const fn new(repository: R, authorization: A) -> Self {
        Self {
            repository,
            authorization,
        }
    }
}

#[async_trait]
impl<R: CatalogQueryRepository, A: AuthorizationRepository> CatalogApi for CatalogService<R, A> {
    async fn list_library_books(
        &self,
        actor: UserId,
        library_id: LibraryId,
        request_id: RequestId,
        page: PageRequest,
    ) -> Result<Page<BookSummary>, AppError> {
        ListLibraryBooks::new(&self.repository, &self.authorization)
            .execute(actor, library_id, request_id, page)
            .await
    }

    async fn get_item(
        &self,
        actor: UserId,
        library_id: LibraryId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<ItemDetail, AppError> {
        super::GetItem::new(&self.repository, &self.authorization)
            .execute(actor, library_id, item_id, request_id)
            .await
    }
}

fn summary(row: VisibleCatalogItem) -> BookSummary {
    BookSummary {
        item_id: row.item_id,
        primary_title: row.primary_title,
        authors: row.authors,
        languages: row.languages,
        media_type: row.media_type,
        can_read: true,
        can_download: row.can_download,
    }
}

fn encode_cursor(holding_id: HoldingId) -> String {
    URL_SAFE_NO_PAD.encode(holding_id.as_uuid().as_bytes())
}

fn decode_cursor(value: &str) -> Result<HoldingId, AppError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| invalid_page("cursor", "invalid_cursor"))?;
    let uuid = Uuid::from_slice(&bytes).map_err(|_| invalid_page("cursor", "invalid_cursor"))?;
    Ok(HoldingId::from_uuid(uuid))
}

fn invalid_page(field: &'static str, code: &'static str) -> AppError {
    AppError::Invalid {
        code: "invalid_page",
        fields: vec![FieldViolation { field, code }],
    }
}
