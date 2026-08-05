#![allow(clippy::expect_used)]

use async_trait::async_trait;
use folioharbor_application::{
    authorization::{Action, Authorization, AuthorizationFact, ResourceRef},
    error::AppError,
    ports::{AuthorizationRepository, AuthorizationRepositoryError},
};
use folioharbor_domain::{
    id::{LibraryId, UserId},
    libraries::role::RoleCode,
};

struct StubRepository {
    fact: Option<AuthorizationFact>,
}

#[async_trait]
impl AuthorizationRepository for StubRepository {
    async fn resolve(
        &self,
        _: UserId,
        _: Action,
        _: ResourceRef,
    ) -> Result<Option<AuthorizationFact>, AuthorizationRepositoryError> {
        Ok(self.fact)
    }
}

#[tokio::test]
async fn require_returns_a_versioned_grant_for_a_seeded_permission() {
    let actor = UserId::new();
    let library = LibraryId::new();
    let resource = ResourceRef::Library(library);
    let repository = StubRepository {
        fact: Some(AuthorizationFact {
            library_id: library,
            role: RoleCode::Owner,
            membership_version: 7,
            discoverable: true,
            permitted: true,
        }),
    };
    let authorization = Authorization::new(&repository);

    let grant = authorization
        .require(actor, Action::ManageLibrary, resource)
        .await
        .expect("owner mapping should grant library.manage");

    assert_eq!(grant.actor(), actor);
    assert_eq!(grant.library_id(), library);
    assert_eq!(grant.action(), Action::ManageLibrary);
    assert_eq!(grant.resource(), resource);
    assert_eq!(grant.membership_version(), 7);
}

#[tokio::test]
async fn require_uses_anti_enumeration_then_visible_forbidden() {
    let actor = UserId::new();
    let library = LibraryId::new();
    let resource = ResourceRef::Library(library);

    for fact in [
        None,
        Some(AuthorizationFact {
            library_id: library,
            role: RoleCode::Reader,
            membership_version: 1,
            discoverable: false,
            permitted: false,
        }),
    ] {
        let error = Authorization::new(&StubRepository { fact })
            .require(actor, Action::ManageLibrary, resource)
            .await
            .expect_err("undiscoverable resources must be hidden");
        assert!(matches!(
            error,
            AppError::NotFound {
                code: "library_not_found"
            }
        ));
    }

    let error = Authorization::new(&StubRepository {
        fact: Some(AuthorizationFact {
            library_id: library,
            role: RoleCode::Reader,
            membership_version: 1,
            discoverable: true,
            permitted: false,
        }),
    })
    .require(actor, Action::ManageLibrary, resource)
    .await
    .expect_err("a visible denied action is forbidden");
    assert!(matches!(
        error,
        AppError::Forbidden {
            code: "library_action_forbidden"
        }
    ));
}
