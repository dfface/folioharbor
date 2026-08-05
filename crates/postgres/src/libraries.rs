use async_trait::async_trait;
use folioharbor_application::ports::{
    AcceptInvitationOutcome, LibraryMutationOutcome, LibraryRepository, LibraryRepositoryError,
    NewLibraryInvitation,
};
use folioharbor_domain::{
    id::{LibraryId, UserId},
    identity::TokenHash,
    libraries::{Library, role::RoleCode},
    time::OffsetDateTime,
};
use sqlx::PgPool;

#[derive(Clone, Debug)]
pub struct PgLibraryRepository {
    pool: PgPool,
}
impl PgLibraryRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
fn persistence_error(_: sqlx::Error) -> LibraryRepositoryError {
    LibraryRepositoryError
}
fn mutation(value: &str) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
    match value {
        "applied" => Ok(LibraryMutationOutcome::Applied),
        "forbidden" => Ok(LibraryMutationOutcome::Forbidden),
        "not_found" => Ok(LibraryMutationOutcome::NotFound),
        "last_owner" => Ok(LibraryMutationOutcome::LastOwner),
        _ => Err(LibraryRepositoryError),
    }
}

#[async_trait]
impl LibraryRepository for PgLibraryRepository {
    async fn provision_personal_library(
        &self,
        user_id: UserId,
        now: OffsetDateTime,
    ) -> Result<Library, LibraryRepositoryError> {
        let row=sqlx::query!(r#"SELECT library_id AS "library_id!", name AS "name!" FROM folioharbor.library_provision_personal($1,$2,$3)"#,LibraryId::new().as_uuid(),user_id.as_uuid(),now).fetch_one(&self.pool).await.map_err(persistence_error)?;
        Ok(Library {
            library_id: LibraryId::from_uuid(row.library_id),
            name: row.name,
        })
    }
    async fn create_invitation(
        &self,
        i: NewLibraryInvitation,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        let value=sqlx::query_scalar!(r#"SELECT folioharbor.library_create_invitation($1,$2,$3,$4,$5,$6,$7,$8,$9) AS "outcome!""#,i.invitation_id.as_uuid(),i.library_id.as_uuid(),i.invited_by.as_uuid(),i.normalized_email.as_str(),i.display_email,i.role.as_str(),i.token_hash.as_bytes().as_slice(),i.created_at,i.expires_at).fetch_one(&self.pool).await.map_err(persistence_error)?;
        mutation(&value)
    }
    async fn accept_invitation(
        &self,
        user_id: UserId,
        hash: TokenHash,
        now: OffsetDateTime,
    ) -> Result<AcceptInvitationOutcome, LibraryRepositoryError> {
        let row=sqlx::query!(r#"SELECT outcome AS "outcome!", accepted_library_id FROM folioharbor.library_accept_invitation($1,$2,$3)"#,user_id.as_uuid(),hash.as_bytes().as_slice(),now).fetch_one(&self.pool).await.map_err(persistence_error)?;
        match (row.outcome.as_str(), row.accepted_library_id) {
            ("accepted", Some(id)) => {
                Ok(AcceptInvitationOutcome::Accepted(LibraryId::from_uuid(id)))
            }
            ("invalid", _) => Ok(AcceptInvitationOutcome::Invalid),
            _ => Err(LibraryRepositoryError),
        }
    }
    async fn change_member_role(
        &self,
        actor: UserId,
        library: LibraryId,
        target: UserId,
        role: RoleCode,
        now: OffsetDateTime,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        let value = sqlx::query_scalar!(
            r#"SELECT folioharbor.library_change_role($1,$2,$3,$4,$5) AS "outcome!""#,
            actor.as_uuid(),
            library.as_uuid(),
            target.as_uuid(),
            role.as_str(),
            now
        )
        .fetch_one(&self.pool)
        .await
        .map_err(persistence_error)?;
        mutation(&value)
    }
    async fn remove_member(
        &self,
        actor: UserId,
        library: LibraryId,
        target: UserId,
        now: OffsetDateTime,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        let value = sqlx::query_scalar!(
            r#"SELECT folioharbor.library_remove_member($1,$2,$3,$4) AS "outcome!""#,
            actor.as_uuid(),
            library.as_uuid(),
            target.as_uuid(),
            now
        )
        .fetch_one(&self.pool)
        .await
        .map_err(persistence_error)?;
        mutation(&value)
    }
    async fn update_library_settings(
        &self,
        actor: UserId,
        library: LibraryId,
        name: &str,
        now: OffsetDateTime,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        let value = sqlx::query_scalar!(
            r#"SELECT folioharbor.library_update_settings($1,$2,$3,$4) AS "outcome!""#,
            actor.as_uuid(),
            library.as_uuid(),
            name,
            now
        )
        .fetch_one(&self.pool)
        .await
        .map_err(persistence_error)?;
        mutation(&value)
    }
}
