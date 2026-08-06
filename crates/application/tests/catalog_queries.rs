#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use folioharbor_application::{
    authorization::{Action, AuthorizationFact, ResourceRef},
    catalog::{CatalogApi, CatalogService, PageRequest},
    error::AppError,
    ports::{
        AuthorizationRepository, AuthorizationRepositoryError, CatalogQueryRepository,
        CatalogRepositoryError, VisibleCatalogItem,
    },
};
use folioharbor_domain::{
    id::{HoldingId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId, UserId},
    libraries::role::RoleCode,
};

struct Authorized(RoleCode);

struct VersionedAuthorized {
    role: RoleCode,
    membership_version: i64,
}

#[async_trait]
impl AuthorizationRepository for Authorized {
    async fn resolve(
        &self,
        _: UserId,
        _: Action,
        resource: ResourceRef,
    ) -> Result<Option<AuthorizationFact>, AuthorizationRepositoryError> {
        Ok(Some(AuthorizationFact {
            library_id: resource.library_id(),
            role: self.0,
            membership_version: 4,
            discoverable: true,
            permitted: true,
        }))
    }
}

#[async_trait]
impl AuthorizationRepository for VersionedAuthorized {
    async fn resolve(
        &self,
        _: UserId,
        _: Action,
        resource: ResourceRef,
    ) -> Result<Option<AuthorizationFact>, AuthorizationRepositoryError> {
        Ok(Some(AuthorizationFact {
            library_id: resource.library_id(),
            role: self.role,
            membership_version: self.membership_version,
            discoverable: true,
            permitted: true,
        }))
    }
}

struct CatalogRows {
    rows: Vec<VisibleCatalogItem>,
    requested_limits: Arc<Mutex<Vec<u32>>>,
}

#[async_trait]
impl CatalogQueryRepository for CatalogRows {
    async fn list_visible_items(
        &self,
        _: folioharbor_application::authorization::AuthorizationGrant,
        _: LibraryId,
        _: Option<HoldingId>,
        limit: u32,
        _: RequestId,
    ) -> Result<Vec<VisibleCatalogItem>, CatalogRepositoryError> {
        self.requested_limits.lock().expect("lock").push(limit);
        Ok(self.rows.clone())
    }

    async fn find_visible_item(
        &self,
        _: folioharbor_application::authorization::AuthorizationGrant,
        _: LibraryId,
        item_id: ItemId,
        _: RequestId,
    ) -> Result<Option<VisibleCatalogItem>, CatalogRepositoryError> {
        Ok(self.rows.iter().find(|row| row.item_id == item_id).cloned())
    }
}

fn item(title: &str) -> VisibleCatalogItem {
    VisibleCatalogItem {
        holding_id: HoldingId::new(),
        item_id: ItemId::new(),
        manifestation_id: ManifestationId::new(),
        package_id: PublicationPackageId::new(),
        primary_title: title.to_owned(),
        authors: vec!["Author".to_owned()],
        languages: vec!["en".to_owned()],
        identifiers: vec!["urn:isbn:example".to_owned()],
        media_type: "application/epub+zip".to_owned(),
        reader_download_enabled: false,
    }
}

#[tokio::test]
async fn list_caps_the_page_and_returns_one_row_per_holding_with_opaque_cursor() {
    let rows = vec![item("One"), item("Two"), item("Three")];
    let requested_limits = Arc::new(Mutex::new(Vec::new()));
    let repository = CatalogRows {
        rows,
        requested_limits: requested_limits.clone(),
    };
    let service = CatalogService::new(repository, Authorized(RoleCode::Reader));
    let page = service
        .list_library_books(
            UserId::new(),
            LibraryId::new(),
            RequestId::new(),
            PageRequest {
                cursor: None,
                limit: Some(2),
            },
        )
        .await
        .expect("visible catalog");

    assert_eq!(page.items.len(), 2);
    assert!(
        page.next_cursor
            .as_deref()
            .is_some_and(|value| value.len() == 22)
    );
    assert!(page.items.iter().all(|book| book.can_read));
    assert!(page.items.iter().all(|book| !book.can_download));
    assert_eq!(*requested_limits.lock().expect("lock"), [3]);
}

#[tokio::test]
async fn owner_and_editor_have_equal_view_and_download_capabilities() {
    for role in [RoleCode::Owner, RoleCode::Editor] {
        let repository = CatalogRows {
            rows: vec![item("Role view")],
            requested_limits: Arc::new(Mutex::new(Vec::new())),
        };
        let service = CatalogService::new(repository, Authorized(role));
        let page = service
            .list_library_books(
                UserId::new(),
                LibraryId::new(),
                RequestId::new(),
                PageRequest::default(),
            )
            .await
            .expect("role can view");
        assert!(page.items[0].can_read);
        assert!(page.items[0].can_download);
    }
}

#[tokio::test]
async fn malformed_cursor_is_rejected_before_querying_catalog() {
    let repository = CatalogRows {
        rows: vec![item("Never queried")],
        requested_limits: Arc::new(Mutex::new(Vec::new())),
    };
    let service = CatalogService::new(repository, Authorized(RoleCode::Reader));
    let error = service
        .list_library_books(
            UserId::new(),
            LibraryId::new(),
            RequestId::new(),
            PageRequest {
                cursor: Some("not-a-cursor".to_owned()),
                limit: Some(10),
            },
        )
        .await
        .expect_err("cursor must be opaque server value");
    assert!(matches!(
        error,
        AppError::Invalid {
            code: "invalid_page",
            ..
        }
    ));
}

#[tokio::test]
async fn detail_keeps_read_and_download_capabilities_independent_for_reader_setting() {
    let row = item("Visible title");
    for (reader_download_enabled, expected) in [(false, false), (true, true)] {
        let mut projected = row.clone();
        projected.reader_download_enabled = reader_download_enabled;
        let repository = CatalogRows {
            rows: vec![projected],
            requested_limits: Arc::new(Mutex::new(Vec::new())),
        };
        let service = CatalogService::new(repository, Authorized(RoleCode::Reader));
        let detail = service
            .get_item(
                UserId::new(),
                LibraryId::new(),
                row.item_id,
                RequestId::new(),
            )
            .await
            .expect("visible detail");
        assert!(detail.can_read);
        assert_eq!(detail.can_download, expected);
        assert_eq!(detail.primary_title, "Visible title");
        assert!(!detail.etag.is_empty());
    }
}

#[tokio::test]
async fn detail_etag_covers_role_membership_version_and_effective_capabilities() {
    let row = item("ETag inputs");
    let mut etags = std::collections::HashSet::new();
    for (role, membership_version, reader_download_enabled) in [
        (RoleCode::Owner, 4, false),
        (RoleCode::Editor, 4, false),
        (RoleCode::Reader, 4, false),
        (RoleCode::Reader, 4, true),
        (RoleCode::Reader, 5, true),
    ] {
        let mut projected = row.clone();
        projected.reader_download_enabled = reader_download_enabled;
        let repository = CatalogRows {
            rows: vec![projected],
            requested_limits: Arc::new(Mutex::new(Vec::new())),
        };
        let service = CatalogService::new(
            repository,
            VersionedAuthorized {
                role,
                membership_version,
            },
        );
        let detail = service
            .get_item(
                UserId::new(),
                LibraryId::new(),
                row.item_id,
                RequestId::new(),
            )
            .await
            .expect("visible detail");
        assert!(
            etags.insert(detail.etag),
            "every response-affecting authorization input needs a distinct validator"
        );
    }
}
