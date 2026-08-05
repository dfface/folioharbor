use async_trait::async_trait;
use folioharbor_domain::{
    id::{InvitationId, LibraryId, UserId},
    identity::{NormalizedEmail, TokenHash},
    libraries::{Library, role::RoleCode},
    time::OffsetDateTime,
};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("library persistence failed")]
pub struct LibraryRepositoryError;

pub struct NewLibraryInvitation {
    pub invitation_id: InvitationId,
    pub library_id: LibraryId,
    pub invited_by: UserId,
    pub normalized_email: NormalizedEmail,
    pub display_email: String,
    pub role: RoleCode,
    pub token_hash: TokenHash,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryMutationOutcome {
    Applied,
    Forbidden,
    NotFound,
    LastOwner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcceptInvitationOutcome {
    Accepted(LibraryId),
    Invalid,
}

#[async_trait]
pub trait LibraryRepository: Send + Sync {
    async fn provision_personal_library(
        &self,
        user_id: UserId,
        now: OffsetDateTime,
    ) -> Result<Library, LibraryRepositoryError>;

    async fn create_invitation(
        &self,
        _: NewLibraryInvitation,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        Err(LibraryRepositoryError)
    }
    async fn accept_invitation(
        &self,
        _: UserId,
        _: TokenHash,
        _: OffsetDateTime,
    ) -> Result<AcceptInvitationOutcome, LibraryRepositoryError> {
        Err(LibraryRepositoryError)
    }
    async fn change_member_role(
        &self,
        _: UserId,
        _: LibraryId,
        _: UserId,
        _: RoleCode,
        _: OffsetDateTime,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        Err(LibraryRepositoryError)
    }
    async fn remove_member(
        &self,
        _: UserId,
        _: LibraryId,
        _: UserId,
        _: OffsetDateTime,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        Err(LibraryRepositoryError)
    }
    async fn update_library_settings(
        &self,
        _: UserId,
        _: LibraryId,
        _: &str,
        _: OffsetDateTime,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        Err(LibraryRepositoryError)
    }
}

#[async_trait]
impl LibraryRepository for () {
    async fn provision_personal_library(
        &self,
        _: UserId,
        _: OffsetDateTime,
    ) -> Result<Library, LibraryRepositoryError> {
        Err(LibraryRepositoryError)
    }
}
