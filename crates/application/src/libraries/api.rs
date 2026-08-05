use async_trait::async_trait;
use folioharbor_domain::{
    id::{LibraryId, RequestId, UserId},
    libraries::role::RoleCode,
};

use super::{
    ChangeMemberRole, ChangeMemberRoleCommand, InviteMember, InviteMemberCommand, RemoveMember,
    RemoveMemberCommand, UpdateLibrarySettings, UpdateLibrarySettingsCommand,
};
use crate::{
    audit::AuditEvent,
    authorization::{Action, Authorization, ResourceRef},
    error::AppError,
    ports::{
        AuditSink, AuthorizationRepository, Clock, LibraryQueryRepository, LibraryRepository,
        Mailer, RandomSource,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryView {
    pub library_id: LibraryId,
    pub name: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LibraryMemberView {
    pub user_id: UserId,
    pub role: RoleCode,
}
#[derive(Clone, Copy)]
pub struct ListLibrariesRequest {
    pub actor: UserId,
    pub request_id: RequestId,
}
#[derive(Clone, Copy)]
pub struct ReadLibraryRequest {
    pub actor: UserId,
    pub request_id: RequestId,
    pub library_id: LibraryId,
}
pub struct UpdateSettingsRequest {
    pub actor: UserId,
    pub request_id: RequestId,
    pub library_id: LibraryId,
    pub name: String,
}
pub struct InviteLibraryMemberRequest {
    pub actor: UserId,
    pub request_id: RequestId,
    pub library_id: LibraryId,
    pub email: String,
    pub role: RoleCode,
}
pub struct ChangeLibraryMemberRequest {
    pub actor: UserId,
    pub request_id: RequestId,
    pub library_id: LibraryId,
    pub user_id: UserId,
    pub role: RoleCode,
}
pub struct RemoveLibraryMemberRequest {
    pub actor: UserId,
    pub request_id: RequestId,
    pub library_id: LibraryId,
    pub user_id: UserId,
}

#[async_trait]
pub trait LibraryApi: Send + Sync {
    async fn list_libraries(
        &self,
        request: ListLibrariesRequest,
    ) -> Result<Vec<LibraryView>, AppError>;
    async fn get_library(&self, request: ReadLibraryRequest) -> Result<LibraryView, AppError>;
    async fn list_members(
        &self,
        request: ReadLibraryRequest,
    ) -> Result<Vec<LibraryMemberView>, AppError>;
    async fn update_settings(&self, request: UpdateSettingsRequest) -> Result<(), AppError>;
    async fn invite_member(&self, request: InviteLibraryMemberRequest) -> Result<(), AppError>;
    async fn change_member(&self, request: ChangeLibraryMemberRequest) -> Result<(), AppError>;
    async fn remove_member(&self, request: RemoveLibraryMemberRequest) -> Result<(), AppError>;
}

pub struct LibraryService<R, A, S, M, C, N> {
    repository: R,
    authorization: A,
    audit: S,
    mailer: M,
    clock: C,
    random: N,
}
impl<R, A, S, M, C, N> LibraryService<R, A, S, M, C, N> {
    #[must_use]
    pub const fn new(
        repository: R,
        authorization: A,
        audit: S,
        mailer: M,
        clock: C,
        random: N,
    ) -> Self {
        Self {
            repository,
            authorization,
            audit,
            mailer,
            clock,
            random,
        }
    }
}
impl<R, A, S, M, C, N> LibraryService<R, A, S, M, C, N>
where
    R: LibraryRepository + LibraryQueryRepository,
    A: AuthorizationRepository,
    S: AuditSink,
    C: Clock,
{
    async fn grant(
        &self,
        actor: UserId,
        request_id: RequestId,
        action: Action,
        resource: ResourceRef,
    ) -> Result<crate::authorization::AuthorizationGrant, AppError> {
        match Authorization::new(&self.authorization)
            .require(actor, action, resource)
            .await
        {
            Ok(grant) => Ok(grant),
            Err(error) => {
                let reason = error_code(&error);
                self.audit
                    .record_denial(AuditEvent::denied(
                        actor,
                        action,
                        resource,
                        reason,
                        request_id,
                        self.clock.now(),
                    ))
                    .await
                    .map_err(|_| AppError::DependencyUnavailable {
                        code: "audit_repository_unavailable",
                    })?;
                Err(error)
            }
        }
    }
}

#[async_trait]
impl<R, A, S, M, C, N> LibraryApi for LibraryService<R, A, S, M, C, N>
where
    R: LibraryRepository + LibraryQueryRepository,
    A: AuthorizationRepository,
    S: AuditSink,
    M: Mailer,
    C: Clock,
    N: RandomSource,
{
    async fn list_libraries(&self, r: ListLibrariesRequest) -> Result<Vec<LibraryView>, AppError> {
        self.repository
            .list_visible(r.actor)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })
    }
    async fn get_library(&self, r: ReadLibraryRequest) -> Result<LibraryView, AppError> {
        let resource = ResourceRef::Library(r.library_id);
        let grant = self
            .grant(r.actor, r.request_id, Action::ViewLibrary, resource)
            .await?;
        self.repository
            .get_library(grant, r.library_id)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })?
            .ok_or(AppError::NotFound {
                code: "library_not_found",
            })
    }
    async fn list_members(
        &self,
        r: ReadLibraryRequest,
    ) -> Result<Vec<LibraryMemberView>, AppError> {
        let resource = ResourceRef::Library(r.library_id);
        let grant = self
            .grant(r.actor, r.request_id, Action::ViewLibrary, resource)
            .await?;
        self.repository
            .list_members(grant, r.library_id)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "library_repository_unavailable",
            })
    }
    async fn update_settings(&self, r: UpdateSettingsRequest) -> Result<(), AppError> {
        let result = UpdateLibrarySettings::new(&self.repository, &self.authorization, &self.clock)
            .execute(UpdateLibrarySettingsCommand {
                actor: r.actor,
                library_id: r.library_id,
                name: r.name,
                request_id: r.request_id,
            })
            .await;
        self.audit_if_denied(
            result,
            r.actor,
            r.request_id,
            Action::ManageLibrary,
            ResourceRef::Library(r.library_id),
        )
        .await
    }
    async fn invite_member(&self, r: InviteLibraryMemberRequest) -> Result<(), AppError> {
        let result = InviteMember::new(
            &self.repository,
            &self.authorization,
            &self.mailer,
            &self.clock,
            &self.random,
        )
        .execute(InviteMemberCommand {
            actor: r.actor,
            library_id: r.library_id,
            email: r.email,
            role: r.role,
            request_id: r.request_id,
        })
        .await;
        self.audit_if_denied(
            result,
            r.actor,
            r.request_id,
            Action::InviteMember,
            ResourceRef::Library(r.library_id),
        )
        .await
    }
    async fn change_member(&self, r: ChangeLibraryMemberRequest) -> Result<(), AppError> {
        let resource = ResourceRef::Membership {
            library_id: r.library_id,
            user_id: r.user_id,
        };
        let result = ChangeMemberRole::new(&self.repository, &self.authorization, &self.clock)
            .execute(ChangeMemberRoleCommand {
                actor: r.actor,
                library_id: r.library_id,
                member: r.user_id,
                role: r.role,
                request_id: r.request_id,
            })
            .await;
        self.audit_if_denied(
            result,
            r.actor,
            r.request_id,
            Action::ChangeMemberRole,
            resource,
        )
        .await
    }
    async fn remove_member(&self, r: RemoveLibraryMemberRequest) -> Result<(), AppError> {
        let resource = ResourceRef::Membership {
            library_id: r.library_id,
            user_id: r.user_id,
        };
        let result = RemoveMember::new(&self.repository, &self.authorization, &self.clock)
            .execute(RemoveMemberCommand {
                actor: r.actor,
                library_id: r.library_id,
                member: r.user_id,
                request_id: r.request_id,
            })
            .await;
        self.audit_if_denied(
            result,
            r.actor,
            r.request_id,
            Action::RemoveMember,
            resource,
        )
        .await
    }
}
impl<R, A, S, M, C, N> LibraryService<R, A, S, M, C, N>
where
    S: AuditSink,
    C: Clock,
{
    async fn audit_if_denied(
        &self,
        result: Result<(), AppError>,
        actor: UserId,
        request: RequestId,
        action: Action,
        resource: ResourceRef,
    ) -> Result<(), AppError> {
        if let Err(error) = result {
            if matches!(
                error,
                AppError::Forbidden { .. } | AppError::NotFound { .. }
            ) {
                self.audit
                    .record_denial(AuditEvent::denied(
                        actor,
                        action,
                        resource,
                        error_code(&error),
                        request,
                        self.clock.now(),
                    ))
                    .await
                    .map_err(|_| AppError::DependencyUnavailable {
                        code: "audit_repository_unavailable",
                    })?;
            }
            return Err(error);
        }
        Ok(())
    }
}
fn error_code(error: &AppError) -> &'static str {
    match error {
        AppError::Forbidden { code }
        | AppError::NotFound { code }
        | AppError::Conflict { code }
        | AppError::Invalid { code, .. }
        | AppError::DependencyUnavailable { code } => code,
        _ => "request_denied",
    }
}

pub struct UnavailableLibraryApi;
#[async_trait]
impl LibraryApi for UnavailableLibraryApi {
    async fn list_libraries(&self, _: ListLibrariesRequest) -> Result<Vec<LibraryView>, AppError> {
        unavailable()
    }
    async fn get_library(&self, _: ReadLibraryRequest) -> Result<LibraryView, AppError> {
        unavailable()
    }
    async fn list_members(
        &self,
        _: ReadLibraryRequest,
    ) -> Result<Vec<LibraryMemberView>, AppError> {
        unavailable()
    }
    async fn update_settings(&self, _: UpdateSettingsRequest) -> Result<(), AppError> {
        unavailable()
    }
    async fn invite_member(&self, _: InviteLibraryMemberRequest) -> Result<(), AppError> {
        unavailable()
    }
    async fn change_member(&self, _: ChangeLibraryMemberRequest) -> Result<(), AppError> {
        unavailable()
    }
    async fn remove_member(&self, _: RemoveLibraryMemberRequest) -> Result<(), AppError> {
        unavailable()
    }
}
fn unavailable<T>() -> Result<T, AppError> {
    Err(AppError::DependencyUnavailable {
        code: "library_service_unavailable",
    })
}
