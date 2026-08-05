use folioharbor_domain::id::{LibraryId, RequestId, UserId};
use sqlx::{PgConnection, query};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatabaseContext {
    user_id: Option<UserId>,
    library_id: Option<LibraryId>,
    request_id: RequestId,
    worker: bool,
}

impl DatabaseContext {
    #[must_use]
    pub const fn api(user_id: UserId, library_id: LibraryId, request_id: RequestId) -> Self {
        Self {
            user_id: Some(user_id),
            library_id: Some(library_id),
            request_id,
            worker: false,
        }
    }

    #[must_use]
    pub const fn worker(request_id: RequestId, library_id: Option<LibraryId>) -> Self {
        Self {
            user_id: None,
            library_id,
            request_id,
            worker: true,
        }
    }
}

pub struct PgTransactionContext;

impl PgTransactionContext {
    /// Applies request identity using transaction-local `PostgreSQL` settings.
    ///
    /// Call this only after beginning a transaction. `PostgreSQL` discards every
    /// setting written here when that transaction commits or rolls back.
    ///
    /// # Errors
    ///
    /// Returns the `PostgreSQL` error if any setting cannot be applied.
    pub async fn apply(
        connection: &mut PgConnection,
        context: &DatabaseContext,
    ) -> Result<(), sqlx::Error> {
        let user_id = context.user_id.map(|id| id.as_uuid().to_string());
        let library_id = context.library_id.map(|id| id.as_uuid().to_string());
        let request_id = context.request_id.as_ulid().to_string();
        let worker = context.worker.to_string();

        for (name, value) in [
            ("app.user_id", user_id.as_deref().unwrap_or("")),
            ("app.library_id", library_id.as_deref().unwrap_or("")),
            ("app.request_id", request_id.as_str()),
            ("app.is_worker", worker.as_str()),
        ] {
            query("SELECT set_config($1, $2, true)")
                .bind(name)
                .bind(value)
                .execute(&mut *connection)
                .await?;
        }
        Ok(())
    }
}
