//! Public catalog use cases and safe response projections.
//!
//! Persistence-only download source metadata is deliberately not part of this API.
//!
//! ```compile_fail
//! use folioharbor_application::catalog::DownloadSource;
//! ```

mod download_item;
mod get_item;
mod import_publication;
mod list_library_books;

pub use crate::ports::DownloadRange;
pub use download_item::{
    DownloadApi, DownloadGrant, DownloadItem, DownloadService, UnavailableDownloadApi,
    sanitize_download_file_name,
};
pub use get_item::{GetItem, ItemDetail};
pub use import_publication::{ImportCatalogCommand, ImportCatalogResult, ImportPublicationCatalog};
pub use list_library_books::{
    BookSummary, CatalogApi, CatalogService, MAX_PAGE_SIZE, Page, PageRequest,
    UnavailableCatalogApi,
};
