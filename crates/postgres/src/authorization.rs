use crate::{DatabaseContext, PgTransactionContext};
use async_trait::async_trait;
use folioharbor_application::{
    authorization::{Action, AuthorizationFact, ResourceRef},
    ports::{AuthorizationRepository, AuthorizationRepositoryError},
};
use folioharbor_domain::{
    id::{RequestId, UserId},
    libraries::role::RoleCode,
};
use sqlx::PgPool;

#[derive(Clone, Debug)]
pub struct PgAuthorizationRepository {
    pool: PgPool,
}
impl PgAuthorizationRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
#[async_trait]
impl AuthorizationRepository for PgAuthorizationRepository {
    async fn resolve(
        &self,
        actor: UserId,
        action: Action,
        resource: ResourceRef,
    ) -> Result<Option<AuthorizationFact>, AuthorizationRepositoryError> {
        let library = resource.library_id();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorizationRepositoryError)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api(actor, library, RequestId::new()),
        )
        .await
        .map_err(|_| AuthorizationRepositoryError)?;
        let row:Option<(String,i64,bool)>=sqlx::query_as("SELECT m.role_code,m.version,(p.permission_code IS NOT NULL) FROM folioharbor.library_memberships m LEFT JOIN folioharbor.role_permissions p ON p.role_code=m.role_code AND p.permission_code=$3 WHERE m.library_id=$1 AND m.user_id=$2 AND m.status='active'")
   .bind(library.as_uuid()).bind(actor.as_uuid()).bind(action.required_permission().as_str()).fetch_optional(&mut *tx).await.map_err(|_|AuthorizationRepositoryError)?;
        tx.commit()
            .await
            .map_err(|_| AuthorizationRepositoryError)?;
        match row {
            Some((role, version, permitted)) => Ok(Some(AuthorizationFact {
                library_id: library,
                role: RoleCode::parse(&role).ok_or(AuthorizationRepositoryError)?,
                membership_version: version,
                discoverable: true,
                permitted,
            })),
            None => Ok(None),
        }
    }
}
