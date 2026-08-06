#![allow(clippy::expect_used)]

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{
        Request, StatusCode,
        header::{
            CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, X_CONTENT_TYPE_OPTIONS,
        },
    },
};
use folioharbor_application::{
    actor::Actor,
    error::AppError,
    identity::{
        AuthenticateSessionCommand, AuthenticateSessionUseCase, AuthenticatedSession,
        CompletePasswordResetCommand, CompletePasswordResetUseCase, CurrentSessionUseCase,
        IssuedSession, ListSessionsUseCase, LoginCommand, LoginUseCase, LogoutCommand,
        LogoutUseCase, PasswordResetComplete, PasswordResetRequested, PendingAccount,
        RegisterAccountCommand, RegisterAccountUseCase, RequestPasswordResetCommand,
        RequestPasswordResetUseCase, RevokeSessionCommand, RevokeSessionOutcome,
        RevokeSessionUseCase, SafeSession, VerifiedAccount, VerifyEmailCommand, VerifyEmailUseCase,
    },
    ports::{
        BlobStore, BlobStoreError, PromotedBlob, ReaderCatalogError, ReaderCatalogRepository,
        ReaderPublication, ReaderResource, ReaderSpineEntry,
    },
    rate_limit::{CheckRateLimit, RateLimitDecision, RateLimitUseCase},
    reader::{
        ManifestLink, ManifestMetadata, PublicationManifest, ReaderApi, ReaderService, ResourceId,
        ResourceResponse,
    },
};
use folioharbor_domain::{
    id::{
        BlobId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId, SessionId,
        UserId,
    },
    identity::CsrfToken,
    imports::blob::{BlobIdentity, StorageKey},
};
use folioharbor_epub::{EpubResourceReader, ResourceCacheLimits};
use folioharbor_http::{AppState, router};
use http_body_util::BodyExt as _;
use secrecy::{ExposeSecret as _, SecretString};
use std::{
    collections::HashMap,
    io::{Cursor, Write},
    sync::Arc,
};
use tower::ServiceExt as _;
use url::Url;
use zip::{ZipWriter, write::SimpleFileOptions};

struct Identity(HashMap<String, UserId>);
fn unused<T>() -> Result<T, AppError> {
    unreachable!("unused identity endpoint")
}

#[async_trait]
impl AuthenticateSessionUseCase for Identity {
    async fn authenticate_session(
        &self,
        command: AuthenticateSessionCommand,
    ) -> Result<Option<AuthenticatedSession>, AppError> {
        Ok(self
            .0
            .get(command.session_token.expose_secret())
            .copied()
            .map(|user_id| AuthenticatedSession {
                actor: Actor {
                    user_id,
                    session_id: SessionId::new(),
                },
                csrf_token_hash: CsrfToken::parse(SecretString::from("reader-csrf".to_owned()))
                    .hash_for_storage(),
            }))
    }
}
#[async_trait]
impl RegisterAccountUseCase for Identity {
    async fn register(&self, _: RegisterAccountCommand) -> Result<PendingAccount, AppError> {
        unused()
    }
}
#[async_trait]
impl VerifyEmailUseCase for Identity {
    async fn verify_email(&self, _: VerifyEmailCommand) -> Result<VerifiedAccount, AppError> {
        unused()
    }
}
#[async_trait]
impl LoginUseCase for Identity {
    async fn login(&self, _: LoginCommand) -> Result<IssuedSession, AppError> {
        unused()
    }
}
#[async_trait]
impl LogoutUseCase for Identity {
    async fn logout(&self, _: LogoutCommand) -> Result<(), AppError> {
        unused()
    }
}
#[async_trait]
impl RequestPasswordResetUseCase for Identity {
    async fn request_password_reset(
        &self,
        _: RequestPasswordResetCommand,
    ) -> Result<PasswordResetRequested, AppError> {
        unused()
    }
}
#[async_trait]
impl CompletePasswordResetUseCase for Identity {
    async fn complete_password_reset(
        &self,
        _: CompletePasswordResetCommand,
    ) -> Result<PasswordResetComplete, AppError> {
        unused()
    }
}
#[async_trait]
impl CurrentSessionUseCase for Identity {
    async fn current_session(&self, _: Actor) -> Result<SafeSession, AppError> {
        unused()
    }
}
#[async_trait]
impl ListSessionsUseCase for Identity {
    async fn list_sessions(&self, _: Actor) -> Result<Vec<SafeSession>, AppError> {
        unused()
    }
}
#[async_trait]
impl RevokeSessionUseCase for Identity {
    async fn revoke_session(
        &self,
        _: RevokeSessionCommand,
    ) -> Result<RevokeSessionOutcome, AppError> {
        unused()
    }
}
#[async_trait]
impl RateLimitUseCase for Identity {
    async fn check_rate_limit(&self, _: CheckRateLimit) -> Result<RateLimitDecision, AppError> {
        unused()
    }
}

struct Reader {
    allowed: UserId,
    item: ItemId,
    manifestation: ManifestationId,
}
#[async_trait]
impl ReaderApi for Reader {
    async fn get_manifest(
        &self,
        actor: UserId,
        _: ItemId,
        _: RequestId,
    ) -> Result<PublicationManifest, AppError> {
        if actor != self.allowed {
            return Err(AppError::NotFound {
                code: "item_not_found",
            });
        }
        Ok(PublicationManifest {
            metadata: ManifestMetadata {
                title: "Safe Book".to_owned(),
                authors: vec!["Writer".to_owned()],
                languages: vec!["en".to_owned()],
            },
            manifestation_id: self.manifestation,
            reading_order: vec![ManifestLink {
                href: format!("/api/v1/items/{}/resources/safe", self.item.as_uuid()),
                media_type: "application/xhtml+xml".to_owned(),
                relation: String::new(),
                title: None,
            }],
            resources: Vec::new(),
            toc: Vec::new(),
            links: vec![ManifestLink {
                href: format!("/api/v1/items/{}/manifest", self.item.as_uuid()),
                media_type: "application/webpub+json".to_owned(),
                relation: "self".to_owned(),
                title: None,
            }],
            etag: "\"package-test-v1\"".to_owned(),
        })
    }
    async fn get_resource(
        &self,
        actor: UserId,
        _: ItemId,
        _: ResourceId,
        _: RequestId,
    ) -> Result<ResourceResponse, AppError> {
        if actor != self.allowed {
            return Err(AppError::NotFound {
                code: "item_not_found",
            });
        }
        Ok(ResourceResponse {
            bytes: b"<html><body><p>safe</p></body></html>".to_vec(),
            media_type: "application/xhtml+xml".to_owned(),
            etag: "\"resource-test-v1\"".to_owned(),
        })
    }
}

fn app() -> (axum::Router, ItemId, ManifestationId) {
    let allowed = UserId::new();
    let item = ItemId::new();
    let manifestation = ManifestationId::new();
    let identity = Arc::new(Identity(HashMap::from([
        ("allowed".to_owned(), allowed),
        ("outsider".to_owned(), UserId::new()),
    ])));
    let state = AppState::new(
        Url::parse("https://library.example").expect("url"),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity,
    )
    .with_reader_api(Arc::new(Reader {
        allowed,
        item,
        manifestation,
    }));
    (router(state), item, manifestation)
}

fn request(uri: &str, actor: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("Cookie", format!("folioharbor_session={actor}"))
        .body(Body::empty())
        .expect("request")
}

#[tokio::test]
async fn serves_readium_manifest_and_isolated_resource_without_internal_paths() {
    let (app, item, manifestation) = app();
    let manifest = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/items/{}/manifest", item.as_uuid()),
            "allowed",
        ))
        .await
        .expect("manifest");
    assert_eq!(manifest.status(), StatusCode::OK);
    assert_eq!(
        manifest.headers().get(ETAG).and_then(|v| v.to_str().ok()),
        Some("\"package-test-v1\"")
    );
    let body = manifest
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["metadata"]["title"], "Safe Book");
    assert_eq!(json["manifestationId"], manifestation.as_uuid().to_string());
    assert!(!String::from_utf8_lossy(&body).contains("OPS/"));

    let resource = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/items/{}/resources/safe", item.as_uuid()),
            "allowed",
        ))
        .await
        .expect("resource");
    assert_eq!(resource.status(), StatusCode::OK);
    assert_eq!(
        resource
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/xhtml+xml")
    );
    assert_eq!(
        resource
            .headers()
            .get(CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some(
            "default-src 'none'; img-src data: blob:; style-src 'unsafe-inline'; font-src data: blob:"
        )
    );
    assert_eq!(
        resource
            .headers()
            .get(X_CONTENT_TYPE_OPTIONS)
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(
        resource
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("private, no-cache")
    );

    let denied = app
        .oneshot(request(
            &format!("/api/v1/items/{}/resources/safe", item.as_uuid()),
            "outsider",
        ))
        .await
        .expect("denied");
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
}

struct ArchiveBlobs(Vec<u8>);
#[async_trait]
impl BlobStore for ArchiveBlobs {
    fn candidate_key(&self, _: &BlobIdentity) -> StorageKey {
        StorageKey::from_opaque("unused".to_owned())
    }
    async fn create_staging_for(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        unreachable!()
    }
    async fn append(&self, _: &StorageKey, _: &[u8]) -> Result<(), BlobStoreError> {
        unreachable!()
    }
    async fn read_range(&self, _: &StorageKey, _: u64, _: u64) -> Result<Vec<u8>, BlobStoreError> {
        unreachable!()
    }
    async fn promote(
        &self,
        _: &StorageKey,
        _: &BlobIdentity,
    ) -> Result<PromotedBlob, BlobStoreError> {
        unreachable!()
    }
    async fn delete(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        unreachable!()
    }
    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        unreachable!()
    }
    async fn open_publication(
        &self,
        _: &StorageKey,
    ) -> Result<Box<dyn folioharbor_application::ports::PublicationSource>, BlobStoreError> {
        Ok(Box::new(Cursor::new(self.0.clone())))
    }
}

struct ReadableCatalog {
    allowed: UserId,
    publication: ReaderPublication,
}
#[async_trait]
impl ReaderCatalogRepository for ReadableCatalog {
    async fn find_readable_publication(
        &self,
        actor: UserId,
        _: ItemId,
        _: RequestId,
    ) -> Result<Option<ReaderPublication>, ReaderCatalogError> {
        Ok((actor == self.allowed).then(|| self.publication.clone()))
    }
}

fn malicious_archive() -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("OPS/chapter.xhtml", SimpleFileOptions::default())
        .expect("entry");
    writer.write_all(br#"<html><head><meta http-equiv="refresh" content="0;url=https://evil.test"></head><body onload="steal()"><script>steal()</script><form>x</form><iframe src="https://evil.test"></iframe><object>x</object><p onclick="x()">safe boundary</p></body></html>"#).expect("html");
    writer.finish().expect("archive").into_inner()
}

#[tokio::test]
async fn malicious_epub_content_is_sanitized_through_the_real_http_boundary() {
    let allowed = UserId::new();
    let item = ItemId::new();
    let package = PublicationPackageId::new();
    let publication = ReaderPublication {
        library_id: LibraryId::new(),
        item_id: item,
        manifestation_id: ManifestationId::new(),
        package_id: package,
        blob_id: BlobId::new(),
        storage_key: StorageKey::from_opaque("blob:test".to_owned()),
        parser_profile_version: "epub3-v1".to_owned(),
        primary_title: "Unsafe input".to_owned(),
        authors: Vec::new(),
        languages: Vec::new(),
        resources: vec![ReaderResource {
            normalized_href: "OPS/chapter.xhtml".to_owned(),
            media_type: "application/xhtml+xml".to_owned(),
        }],
        reading_order: vec![ReaderSpineEntry {
            normalized_href: "OPS/chapter.xhtml".to_owned(),
            linear: true,
        }],
        toc: Vec::new(),
    };
    let resource_id = ResourceId::for_resource(package, "OPS/chapter.xhtml");
    let reader: Arc<dyn ReaderApi> = Arc::new(ReaderService::new(
        ReadableCatalog {
            allowed,
            publication,
        },
        EpubResourceReader::new(
            Arc::new(ArchiveBlobs(malicious_archive())),
            ResourceCacheLimits::default(),
        ),
    ));
    let identity = Arc::new(Identity(HashMap::from([("allowed".to_owned(), allowed)])));
    let state = AppState::new(
        Url::parse("https://library.example").expect("url"),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity,
    )
    .with_reader_api(reader);
    let response = router(state)
        .oneshot(request(
            &format!(
                "/api/v1/items/{}/resources/{}",
                item.as_uuid(),
                resource_id.as_str()
            ),
            "allowed",
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let html = String::from_utf8_lossy(&body).to_ascii_lowercase();
    assert!(html.contains("safe boundary"));
    for forbidden in [
        "<script",
        "<form",
        "<iframe",
        "<object",
        "http-equiv",
        "onload",
        "onclick",
        "https://",
    ] {
        assert!(!html.contains(forbidden), "found {forbidden}: {html}");
    }
}
