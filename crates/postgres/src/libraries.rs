use async_trait::async_trait;
use folioharbor_application::ports::{
    AcceptInvitationOutcome, LibraryMutationOutcome, LibraryQueryRepository,
    LibraryQueryRepositoryError, LibraryRepository, LibraryRepositoryError, NewLibraryInvitation,
    NewMailOutboxEntry,
};
use folioharbor_application::{
    audit::{AuditDecision, AuditEvent, AuditSource},
    authorization::{Action, AuthorizationGrant, ResourceRef},
    libraries::{LibraryCapabilities, LibraryMemberView, LibraryView},
};
use folioharbor_domain::{
    id::{LibraryId, UserId},
    identity::TokenHash,
    libraries::{Library, role::RoleCode},
    time::OffsetDateTime,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{DatabaseContext, PgTransactionContext};

#[derive(Clone, Debug)]
pub struct PgLibraryRepository {
    pool: PgPool,
    library_quota_bytes: u64,
}

#[async_trait]
impl LibraryQueryRepository for PgLibraryRepository {
    async fn list_visible(
        &self,
        actor: UserId,
    ) -> Result<Vec<LibraryView>, LibraryQueryRepositoryError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LibraryQueryRepositoryError)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api_without_library(actor, folioharbor_domain::id::RequestId::new()),
        )
        .await
        .map_err(|_| LibraryQueryRepositoryError)?;
        let rows = sqlx::query_as::<_, (Uuid, String, String, bool, bool, bool, bool, bool)>(
            "SELECT library_id,name,role_code,reader_download_enabled,can_upload,can_invite_members,can_manage_members,can_manage_settings FROM folioharbor.library_web_visible($1)",
        )
        .bind(actor.as_uuid())
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| LibraryQueryRepositoryError)?;
        tx.commit().await.map_err(|_| LibraryQueryRepositoryError)?;
        Ok(rows
            .into_iter()
            .map(library_view)
            .collect::<Result<Vec<_>, _>>()?)
    }
    async fn get_library(
        &self,
        grant: AuthorizationGrant,
        library: LibraryId,
    ) -> Result<Option<LibraryView>, LibraryQueryRepositoryError> {
        if grant.library_id() != library
            || grant.action() != Action::ViewLibrary
            || grant.resource() != ResourceRef::Library(library)
        {
            return Err(LibraryQueryRepositoryError);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LibraryQueryRepositoryError)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api(
                grant.actor(),
                library,
                folioharbor_domain::id::RequestId::new(),
            ),
        )
        .await
        .map_err(|_| LibraryQueryRepositoryError)?;
        let row = sqlx::query_as::<_, (Uuid, String, String, bool, bool, bool, bool, bool)>(
            "SELECT library_id,name,role_code,reader_download_enabled,can_upload,can_invite_members,can_manage_members,can_manage_settings FROM folioharbor.library_web_visible($1) WHERE library_id=$2",
        )
        .bind(grant.actor().as_uuid())
        .bind(library.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LibraryQueryRepositoryError)?;
        tx.commit().await.map_err(|_| LibraryQueryRepositoryError)?;
        row.map(library_view).transpose()
    }
    async fn list_members(
        &self,
        grant: AuthorizationGrant,
        library: LibraryId,
    ) -> Result<Vec<LibraryMemberView>, LibraryQueryRepositoryError> {
        if grant.library_id() != library
            || grant.action() != Action::ViewLibrary
            || grant.resource() != ResourceRef::Library(library)
        {
            return Err(LibraryQueryRepositoryError);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| LibraryQueryRepositoryError)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api(
                grant.actor(),
                library,
                folioharbor_domain::id::RequestId::new(),
            ),
        )
        .await
        .map_err(|_| LibraryQueryRepositoryError)?;
        let rows = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT user_id,role_code FROM folioharbor.library_members_web_visible($1,$2,$3)",
        )
        .bind(grant.actor().as_uuid())
        .bind(library.as_uuid())
        .bind(grant.membership_version())
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| LibraryQueryRepositoryError)?;
        tx.commit().await.map_err(|_| LibraryQueryRepositoryError)?;
        rows.into_iter()
            .map(|row| {
                RoleCode::parse(&row.1)
                    .map(|role| LibraryMemberView {
                        user_id: UserId::from_uuid(row.0),
                        role,
                    })
                    .ok_or(LibraryQueryRepositoryError)
            })
            .collect()
    }
}
impl PgLibraryRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self {
            pool,
            library_quota_bytes: 5 * 1024 * 1024 * 1024,
        }
    }

    #[must_use]
    pub const fn with_library_quota_bytes(mut self, library_quota_bytes: u64) -> Self {
        self.library_quota_bytes = library_quota_bytes;
        self
    }
}
fn persistence_error(_: sqlx::Error) -> LibraryRepositoryError {
    LibraryRepositoryError
}
fn library_view(
    row: (Uuid, String, String, bool, bool, bool, bool, bool),
) -> Result<LibraryView, LibraryQueryRepositoryError> {
    let (
        library_id,
        name,
        role,
        reader_download_enabled,
        can_upload,
        can_invite_members,
        can_manage_members,
        can_manage_settings,
    ) = row;
    Ok(LibraryView {
        library_id: LibraryId::from_uuid(library_id),
        name,
        role: RoleCode::parse(&role).ok_or(LibraryQueryRepositoryError)?,
        reader_download_enabled,
        capabilities: LibraryCapabilities {
            can_upload,
            can_invite_members,
            can_manage_members,
            can_manage_settings,
        },
    })
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

fn resource_id(resource: ResourceRef) -> Uuid {
    match resource {
        ResourceRef::Library(id) => id.as_uuid(),
        ResourceRef::Membership { user_id, .. } => user_id.as_uuid(),
        ResourceRef::Invitation { invitation_id, .. } => invitation_id.as_uuid(),
        ResourceRef::Upload { upload_id, .. } => upload_id.as_uuid(),
        ResourceRef::Item { item_id, .. } => item_id.as_uuid(),
    }
}

fn validate_facts(
    grant: AuthorizationGrant,
    audit: &AuditEvent,
    actor: UserId,
    library: LibraryId,
    action: Action,
    resource: ResourceRef,
) -> Result<(), LibraryRepositoryError> {
    if grant.actor() == actor
        && grant.library_id() == library
        && grant.action() == action
        && grant.resource() == resource
        && audit.actor == Some(actor)
        && audit.effective_actor == Some(actor)
        && audit.library_id == library
        && audit.action == action
        && audit.resource == resource
        && audit.decision == AuditDecision::Allowed
        && audit.reason_code.is_none()
        && audit.source == AuditSource::Api
    {
        Ok(())
    } else {
        Err(LibraryRepositoryError)
    }
}

#[async_trait]
impl LibraryRepository for PgLibraryRepository {
    async fn provision_personal_library(
        &self,
        user_id: UserId,
        now: OffsetDateTime,
    ) -> Result<Library, LibraryRepositoryError> {
        let quota = i64::try_from(self.library_quota_bytes).map_err(|_| LibraryRepositoryError)?;
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api_without_library(
                user_id,
                folioharbor_domain::id::RequestId::new(),
            ),
        )
        .await
        .map_err(persistence_error)?;
        let row = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT library_id,name FROM folioharbor.library_provision_personal_configured($1,$2,$3,$4)",
        )
        .bind(LibraryId::new().as_uuid())
        .bind(user_id.as_uuid())
        .bind(now)
        .bind(quota)
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)?;
        Ok(Library {
            library_id: LibraryId::from_uuid(row.0),
            name: row.1,
        })
    }
    async fn create_invitation(
        &self,
        i: NewLibraryInvitation,
        grant: AuthorizationGrant,
        audit: AuditEvent,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        validate_facts(
            grant,
            &audit,
            i.invited_by,
            i.library_id,
            Action::InviteMember,
            ResourceRef::Invitation {
                library_id: i.library_id,
                invitation_id: i.invitation_id,
            },
        )?;
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api(grant.actor(), grant.library_id(), audit.request_id),
        )
        .await
        .map_err(persistence_error)?;
        let row = sqlx::query!(
            r#"SELECT folioharbor.library_create_invitation_authorized($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21) AS "outcome!""#,
            i.invitation_id.as_uuid(), i.library_id.as_uuid(), i.invited_by.as_uuid(),
            i.normalized_email.as_str(), i.display_email, i.role.as_str(),
            i.token_hash.as_bytes().as_slice(), i.created_at, i.expires_at,
            grant.membership_version(), Uuid::now_v7(),
            audit.effective_actor.map(UserId::as_uuid), audit.action.as_str(),
            audit.resource.resource_type(), resource_id(audit.resource), audit.decision.as_str(),
            audit.reason_code, audit.request_id.as_ulid().to_string(), audit.source.as_str(),
            audit.occurred_at, audit.network_hmac.as_ref().map(<[u8; 32]>::as_slice),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)?;
        mutation(&row.outcome)
    }
    async fn create_invitation_with_mail(
        &self,
        i: NewLibraryInvitation,
        grant: AuthorizationGrant,
        audit: AuditEvent,
        mail: NewMailOutboxEntry,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        let resource = ResourceRef::Invitation {
            library_id: i.library_id,
            invitation_id: i.invitation_id,
        };
        validate_facts(
            grant,
            &audit,
            i.invited_by,
            i.library_id,
            Action::InviteMember,
            resource,
        )?;
        if mail.delivery_address != i.normalized_email.as_str()
            || mail.template_code != "invitation"
            || mail.invitation_library_id != Some(i.library_id.as_uuid())
            || mail.invitation_role.as_deref() != Some(i.role.as_str())
        {
            return Err(LibraryRepositoryError);
        }
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api(grant.actor(), grant.library_id(), audit.request_id),
        )
        .await
        .map_err(persistence_error)?;
        let row = sqlx::query!(
            r#"SELECT folioharbor.library_create_invitation_authorized($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21) AS "outcome!""#,
            i.invitation_id.as_uuid(), i.library_id.as_uuid(), i.invited_by.as_uuid(),
            i.normalized_email.as_str(), i.display_email, i.role.as_str(),
            i.token_hash.as_bytes().as_slice(), i.created_at, i.expires_at,
            grant.membership_version(), Uuid::now_v7(), audit.effective_actor.map(UserId::as_uuid),
            audit.action.as_str(), audit.resource.resource_type(), resource_id(audit.resource),
            audit.decision.as_str(), audit.reason_code, audit.request_id.as_ulid().to_string(),
            audit.source.as_str(), audit.occurred_at,
            audit.network_hmac.as_ref().map(<[u8; 32]>::as_slice),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        let outcome = mutation(&row.outcome)?;
        if outcome == LibraryMutationOutcome::Applied {
            crate::mail::insert_mail(&mut tx, mail)
                .await
                .map_err(persistence_error)?;
        }
        tx.commit().await.map_err(persistence_error)?;
        Ok(outcome)
    }
    async fn accept_invitation(
        &self,
        user_id: UserId,
        hash: TokenHash,
        now: OffsetDateTime,
    ) -> Result<AcceptInvitationOutcome, LibraryRepositoryError> {
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api_without_library(
                user_id,
                folioharbor_domain::id::RequestId::new(),
            ),
        )
        .await
        .map_err(persistence_error)?;
        let row = sqlx::query_as::<_, (String, Option<Uuid>, Option<String>)>(
            "SELECT outcome,accepted_library_id,invited_email FROM folioharbor.library_accept_invitation_detailed($1,$2,$3)",
        )
        .bind(user_id.as_uuid())
        .bind(hash.as_bytes().as_slice())
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)?;
        match (row.0.as_str(), row.1, row.2) {
            ("accepted", Some(id), _) => {
                Ok(AcceptInvitationOutcome::Accepted(LibraryId::from_uuid(id)))
            }
            ("wrong_account", _, Some(invited_email)) => {
                Ok(AcceptInvitationOutcome::WrongAccount { invited_email })
            }
            ("unverified", _, _) => Ok(AcceptInvitationOutcome::Unverified),
            ("expired", _, _) => Ok(AcceptInvitationOutcome::Expired),
            ("consumed", _, _) => Ok(AcceptInvitationOutcome::Consumed),
            ("invalid", _, _) => Ok(AcceptInvitationOutcome::Invalid),
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
        grant: AuthorizationGrant,
        audit: AuditEvent,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        validate_facts(
            grant,
            &audit,
            actor,
            library,
            Action::ChangeMemberRole,
            ResourceRef::Membership {
                library_id: library,
                user_id: target,
            },
        )?;
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api(grant.actor(), grant.library_id(), audit.request_id),
        )
        .await
        .map_err(persistence_error)?;
        let row = sqlx::query!(
            r#"SELECT folioharbor.library_change_role_authorized($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17) AS "outcome!""#,
            actor.as_uuid(), library.as_uuid(), target.as_uuid(), role.as_str(), now,
            grant.membership_version(), Uuid::now_v7(), audit.effective_actor.map(UserId::as_uuid),
            audit.action.as_str(), audit.resource.resource_type(), resource_id(audit.resource),
            audit.decision.as_str(), audit.reason_code, audit.request_id.as_ulid().to_string(),
            audit.source.as_str(), audit.occurred_at,
            audit.network_hmac.as_ref().map(<[u8; 32]>::as_slice),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)?;
        mutation(&row.outcome)
    }
    async fn remove_member(
        &self,
        actor: UserId,
        library: LibraryId,
        target: UserId,
        now: OffsetDateTime,
        grant: AuthorizationGrant,
        audit: AuditEvent,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        validate_facts(
            grant,
            &audit,
            actor,
            library,
            Action::RemoveMember,
            ResourceRef::Membership {
                library_id: library,
                user_id: target,
            },
        )?;
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api(grant.actor(), grant.library_id(), audit.request_id),
        )
        .await
        .map_err(persistence_error)?;
        let row = sqlx::query!(
            r#"SELECT folioharbor.library_remove_member_authorized($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) AS "outcome!""#,
            actor.as_uuid(), library.as_uuid(), target.as_uuid(), now, grant.membership_version(),
            Uuid::now_v7(), audit.effective_actor.map(UserId::as_uuid), audit.action.as_str(),
            audit.resource.resource_type(), resource_id(audit.resource), audit.decision.as_str(),
            audit.reason_code, audit.request_id.as_ulid().to_string(), audit.source.as_str(),
            audit.occurred_at, audit.network_hmac.as_ref().map(<[u8; 32]>::as_slice),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)?;
        mutation(&row.outcome)
    }
    async fn update_library_settings(
        &self,
        actor: UserId,
        library: LibraryId,
        settings: folioharbor_application::ports::LibrarySettingsUpdate<'_>,
        now: OffsetDateTime,
        grant: AuthorizationGrant,
        audit: AuditEvent,
    ) -> Result<LibraryMutationOutcome, LibraryRepositoryError> {
        validate_facts(
            grant,
            &audit,
            actor,
            library,
            Action::ManageLibrary,
            ResourceRef::Library(library),
        )?;
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api(grant.actor(), grant.library_id(), audit.request_id),
        )
        .await
        .map_err(persistence_error)?;
        let row = sqlx::query!(
            r#"SELECT folioharbor.library_update_settings_authorized($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) AS "outcome!""#,
            actor.as_uuid(), library.as_uuid(), settings.name, now, grant.membership_version(),
            Uuid::now_v7(), audit.effective_actor.map(UserId::as_uuid), audit.action.as_str(),
            audit.resource.resource_type(), resource_id(audit.resource), audit.decision.as_str(),
            audit.reason_code, audit.request_id.as_ulid().to_string(), audit.source.as_str(),
            audit.occurred_at, audit.network_hmac.as_ref().map(<[u8; 32]>::as_slice),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        if let Some(enabled) = settings.reader_download_enabled {
            let updated: bool = sqlx::query_scalar(
                "SELECT folioharbor.library_update_reader_download_authorized($1,$2,$3,$4,$5)",
            )
            .bind(actor.as_uuid())
            .bind(library.as_uuid())
            .bind(enabled)
            .bind(grant.membership_version())
            .bind(audit.request_id.as_ulid().to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(persistence_error)?;
            if !updated {
                tx.rollback().await.map_err(persistence_error)?;
                return Ok(LibraryMutationOutcome::Forbidden);
            }
        }
        tx.commit().await.map_err(persistence_error)?;
        mutation(&row.outcome)
    }
}
