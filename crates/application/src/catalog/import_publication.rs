use folioharbor_domain::{
    catalog::CatalogPublication,
    id::{BlobId, ItemId, LibraryId, PublicationPackageId, RequestId, UploadId, UserId},
    imports::blob::ByteCount,
    time::OffsetDateTime,
};

use crate::{
    error::AppError,
    ports::{CatalogRepository, FinalizeCatalog},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportCatalogCommand {
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub actor_id: UserId,
    pub original_blob_id: BlobId,
    pub logical_bytes: ByteCount,
    pub parser_profile_version: String,
    pub publication: CatalogPublication,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportCatalogResult {
    Created {
        item_id: ItemId,
        package_id: PublicationPackageId,
    },
    Duplicate {
        item_id: ItemId,
    },
}

pub struct ImportPublicationCatalog<'a, R: ?Sized> {
    repository: &'a R,
}

impl<'a, R: ?Sized> ImportPublicationCatalog<'a, R> {
    #[must_use]
    pub const fn new(repository: &'a R) -> Self {
        Self { repository }
    }
}

impl<R: CatalogRepository + ?Sized> ImportPublicationCatalog<'_, R> {
    /// Finalizes one parsed upload without treating bibliographic text as identity.
    ///
    /// # Errors
    /// Returns a stable application error when validation or persistence fails.
    pub async fn execute(
        &self,
        command: ImportCatalogCommand,
    ) -> Result<ImportCatalogResult, AppError> {
        if command.parser_profile_version.is_empty() || command.parser_profile_version.len() > 128 {
            return Err(AppError::Invalid {
                code: "invalid_parser_profile",
                fields: Vec::new(),
            });
        }
        self.repository
            .finalize(FinalizeCatalog::from(command))
            .await
            .map_err(Into::into)
    }
}
