use folioharbor_domain::{
    id::{InvitationId, ItemId, LibraryId, UploadId, UserId},
    libraries::role::{PermissionCode, RoleCode},
};

use crate::{error::AppError, ports::AuthorizationRepository};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Action {
    ViewLibrary,
    ManageLibrary,
    InviteMember,
    ChangeMemberRole,
    RemoveMember,
    CreateUpload,
    InspectUpload,
    ImportPublication,
    DownloadItem,
    DeleteItem,
    RestoreItem,
}

impl Action {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ViewLibrary => "library.view",
            Self::ManageLibrary => "library.manage",
            Self::InviteMember => "member.invite",
            Self::ChangeMemberRole => "member.role.change",
            Self::RemoveMember => "member.remove",
            Self::CreateUpload => "upload.create",
            Self::InspectUpload => "upload.inspect",
            Self::ImportPublication => "publication.import",
            Self::DownloadItem => "item.download",
            Self::DeleteItem => "item.delete",
            Self::RestoreItem => "item.restore",
        }
    }

    #[must_use]
    pub const fn required_permission(self) -> PermissionCode {
        match self {
            Self::ViewLibrary => PermissionCode::HoldingView,
            Self::CreateUpload
            | Self::InspectUpload
            | Self::ImportPublication
            | Self::DeleteItem
            | Self::RestoreItem => PermissionCode::HoldingEdit,
            Self::DownloadItem => PermissionCode::ItemDownload,
            Self::InviteMember => PermissionCode::MemberInvite,
            Self::ManageLibrary | Self::ChangeMemberRole | Self::RemoveMember => {
                PermissionCode::LibraryManage
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ResourceRef {
    Library(LibraryId),
    Membership {
        library_id: LibraryId,
        user_id: UserId,
    },
    Invitation {
        library_id: LibraryId,
        invitation_id: InvitationId,
    },
    Upload {
        library_id: LibraryId,
        upload_id: UploadId,
    },
    Item {
        library_id: LibraryId,
        item_id: ItemId,
    },
}

impl ResourceRef {
    #[must_use]
    pub const fn library_id(self) -> LibraryId {
        match self {
            Self::Library(id)
            | Self::Membership { library_id: id, .. }
            | Self::Invitation { library_id: id, .. }
            | Self::Upload { library_id: id, .. }
            | Self::Item { library_id: id, .. } => id,
        }
    }

    #[must_use]
    pub const fn resource_type(self) -> &'static str {
        match self {
            Self::Library(_) => "library",
            Self::Membership { .. } => "membership",
            Self::Invitation { .. } => "invitation",
            Self::Upload { .. } => "upload",
            Self::Item { .. } => "item",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthorizationFact {
    pub library_id: LibraryId,
    pub role: RoleCode,
    pub membership_version: i64,
    pub discoverable: bool,
    pub permitted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthorizationGrant {
    actor: UserId,
    library_id: LibraryId,
    role: RoleCode,
    action: Action,
    resource: ResourceRef,
    membership_version: i64,
}

impl AuthorizationGrant {
    #[must_use]
    pub const fn actor(self) -> UserId {
        self.actor
    }
    #[must_use]
    pub const fn library_id(self) -> LibraryId {
        self.library_id
    }
    #[must_use]
    pub const fn action(self) -> Action {
        self.action
    }
    #[must_use]
    pub const fn resource(self) -> ResourceRef {
        self.resource
    }
    #[must_use]
    pub const fn membership_version(self) -> i64 {
        self.membership_version
    }

    #[must_use]
    pub const fn role(self) -> RoleCode {
        self.role
    }
}

pub struct Authorization<'a, R: ?Sized> {
    repository: &'a R,
}

impl<'a, R: ?Sized> Authorization<'a, R> {
    #[must_use]
    pub const fn new(repository: &'a R) -> Self {
        Self { repository }
    }
}

impl<R: AuthorizationRepository + ?Sized> Authorization<'_, R> {
    /// Resolves an action through persisted role-permission mappings.
    ///
    /// # Errors
    /// Returns not-found for undiscoverable resources, forbidden for visible denied actions,
    /// and dependency-unavailable when permission resolution fails.
    pub async fn require(
        &self,
        actor: UserId,
        action: Action,
        resource: ResourceRef,
    ) -> Result<AuthorizationGrant, AppError> {
        let fact = self
            .repository
            .resolve(actor, action, resource)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "authorization_repository_unavailable",
            })?;
        let Some(fact) = fact else {
            return Err(AppError::NotFound {
                code: "library_not_found",
            });
        };
        if !fact.discoverable || fact.library_id != resource.library_id() {
            return Err(AppError::NotFound {
                code: "library_not_found",
            });
        }
        if !fact.permitted {
            return Err(AppError::Forbidden {
                code: "library_action_forbidden",
            });
        }
        Ok(AuthorizationGrant {
            actor,
            library_id: fact.library_id,
            role: fact.role,
            action,
            resource,
            membership_version: fact.membership_version,
        })
    }
}
