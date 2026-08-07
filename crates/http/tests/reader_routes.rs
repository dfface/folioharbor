#![allow(clippy::expect_used)]

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{
        HeaderValue, Request, StatusCode,
        header::{
            CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, REFERRER_POLICY,
            X_CONTENT_TYPE_OPTIONS,
        },
    },
};
use folioharbor_application::{
    actor::Actor,
    catalog::{DownloadRange, DownloadService},
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
        BlobStore, BlobStoreError, DownloadAuthorization, DownloadRepository,
        DownloadRepositoryError, DownloadSourceReceiver, PromotedBlob, ReaderCatalogError,
        ReaderCatalogRepository, ReaderPublication, ReaderResource, ReaderSpineEntry,
    },
    rate_limit::{CheckRateLimit, RateLimitDecision, RateLimitUseCase},
    reader::{
        ManifestLink, ManifestMetadata, ProgressApi, PublicationManifest, ReaderApi, ReaderService,
        ResourceId, ResourceResponse, UpdateReadingProgressCommand,
    },
};
use folioharbor_domain::{
    id::{
        BlobId, DeviceId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId,
        SessionId, UserId,
    },
    identity::CsrfToken,
    imports::blob::{BlobIdentity, StorageKey},
    reader::{
        DeviceReadingState, LocatorExtensions, LocatorLocations, ReadingProgress,
        ReadingUpdateOutcome, ReadiumLocator,
    },
    time::OffsetDateTime,
};
use folioharbor_epub::{EpubResourceReader, ResourceCacheLimits};
use folioharbor_http::{AppState, router};
use folioharbor_storage_local::LocalBlobStore;
use http_body_util::BodyExt as _;
use secrecy::{ExposeSecret as _, SecretString};
use std::{
    collections::HashMap,
    io::{Cursor, Write},
    sync::atomic::{AtomicUsize, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt as _;
use url::Url;
use uuid::Uuid;
use zip::{ZipWriter, write::SimpleFileOptions};

struct Identity(HashMap<String, UserId>);

#[derive(Clone)]
struct DownloadFixture {
    allowed: UserId,
    item: ItemId,
    blob: BlobId,
    key: StorageKey,
    byte_size: u64,
    blobs: LocalBlobStore,
    _root: Arc<tempfile::TempDir>,
    reads: Arc<AtomicUsize>,
    starts: Arc<Mutex<Vec<DownloadRange>>>,
}

#[async_trait]
impl DownloadRepository for DownloadFixture {
    async fn authorize_download(
        &self,
        actor: Actor,
        item: ItemId,
        _: RequestId,
        source: &mut dyn DownloadSourceReceiver,
    ) -> Result<DownloadAuthorization, DownloadRepositoryError> {
        if actor.user_id != self.allowed || item != self.item {
            return Ok(DownloadAuthorization::NotFound);
        }
        source.receive(
            self.blob,
            self.key.clone(),
            self.byte_size,
            "危 险.epub".to_owned(),
        );
        Ok(DownloadAuthorization::Granted)
    }

    async fn record_download_start(
        &self,
        _: Actor,
        _: ItemId,
        _: RequestId,
        range: DownloadRange,
    ) -> Result<bool, DownloadRepositoryError> {
        self.starts
            .lock()
            .expect("download starts lock")
            .push(range);
        Ok(true)
    }
}

#[async_trait]
impl BlobStore for DownloadFixture {
    fn candidate_key(&self, identity: &BlobIdentity) -> StorageKey {
        self.blobs.candidate_key(identity)
    }
    async fn create_staging_for(&self, key: &StorageKey) -> Result<(), BlobStoreError> {
        self.blobs.create_staging_for(key).await
    }
    async fn append(&self, key: &StorageKey, bytes: &[u8]) -> Result<(), BlobStoreError> {
        self.blobs.append(key, bytes).await
    }
    async fn read_range(
        &self,
        key: &StorageKey,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, BlobStoreError> {
        assert_eq!(key, &self.key);
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.blobs.read_range(key, offset, length).await
    }
    async fn promote(
        &self,
        key: &StorageKey,
        identity: &BlobIdentity,
    ) -> Result<PromotedBlob, BlobStoreError> {
        self.blobs.promote(key, identity).await
    }
    async fn delete(&self, key: &StorageKey) -> Result<(), BlobStoreError> {
        self.blobs.delete(key).await
    }
    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        self.blobs.free_bytes().await
    }
}
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
            resources: vec![
                ManifestLink {
                    href: format!("/api/v1/items/{}/resources/cover", self.item.as_uuid()),
                    media_type: "image/png".to_owned(),
                    relation: "resource".to_owned(),
                    title: None,
                },
                ManifestLink {
                    href: format!("/api/v1/items/{}/resources/styles", self.item.as_uuid()),
                    media_type: "text/css".to_owned(),
                    relation: "resource".to_owned(),
                    title: None,
                },
            ],
            toc: vec![ManifestLink {
                href: format!("/api/v1/items/{}/resources/safe#start", self.item.as_uuid()),
                media_type: "application/xhtml+xml".to_owned(),
                relation: String::new(),
                title: Some("Chapter 1".to_owned()),
            }],
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

fn progress_locator(progression: f64) -> ReadiumLocator {
    ReadiumLocator::new(
        "OPS/chapter.xhtml".to_owned(),
        Some("application/xhtml+xml".to_owned()),
        LocatorLocations::new(Some(progression), None, None, Vec::new()).expect("locations"),
        None,
        LocatorExtensions::empty_v1(),
    )
    .expect("locator")
}

#[async_trait]
impl ProgressApi for Reader {
    async fn get_progress(
        &self,
        actor: UserId,
        manifestation_id: ManifestationId,
        _: RequestId,
    ) -> Result<Option<ReadingProgress>, AppError> {
        if actor != self.allowed {
            return Err(AppError::NotFound {
                code: "manifestation_not_found",
            });
        }
        if manifestation_id != self.manifestation {
            return Ok(None);
        }
        Ok(Some(ReadingProgress {
            manifestation_id,
            package_id: None,
            content_unit_id: None,
            locator: progress_locator(0.4),
            version: 3,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }))
    }
    async fn update_progress(
        &self,
        command: UpdateReadingProgressCommand,
    ) -> Result<ReadingUpdateOutcome, AppError> {
        if command.locator.locations().progression() == Some(0.77) {
            return Err(AppError::Conflict {
                code: "progress_mutation_mismatch",
            });
        }
        if command.locator.locations().progression() == Some(0.88) {
            return Err(AppError::RateLimited {
                retry_after: Duration::from_secs(17),
            });
        }
        let global = ReadingProgress {
            manifestation_id: command.manifestation_id,
            package_id: None,
            content_unit_id: None,
            locator: progress_locator(0.4),
            version: 3,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        let device = DeviceReadingState {
            device_id: command.device_id,
            locator: command.locator,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        if device.locator.locations().progression() == Some(0.66) {
            return Ok(ReadingUpdateOutcome::Updated { global, device });
        }
        Ok(ReadingUpdateOutcome::Conflict {
            global: (command.base_version != 9).then_some(global),
            device,
        })
    }
}

fn allowed_user() -> UserId {
    UserId::from_uuid(
        Uuid::parse_str("018f47b5-58b4-7ba6-9a3a-d9f41f17a101").expect("allowed user UUID"),
    )
}

fn app() -> (axum::Router, ItemId, ManifestationId) {
    let allowed = allowed_user();
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
    }))
    .with_progress_api(Arc::new(Reader {
        allowed,
        item,
        manifestation,
    }));
    (router(state), item, manifestation)
}

fn download_app() -> (
    axum::Router,
    ItemId,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<DownloadRange>>>,
) {
    let allowed = UserId::new();
    let item = ItemId::new();
    let reads = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(Mutex::new(Vec::new()));
    let root = Arc::new(tempfile::tempdir().expect("download blob root"));
    let payload = (0_u8..=250)
        .cycle()
        .take(128 * 1024 + 17)
        .collect::<Vec<_>>();
    let hash = "11".repeat(32);
    let directory = root.path().join("objects/instance-v1/11/11");
    std::fs::create_dir_all(&directory).expect("blob hierarchy");
    std::fs::write(
        directory.join(format!("{hash}-{}", payload.len())),
        &payload,
    )
    .expect("local blob");
    let key = StorageKey::from_opaque(format!("blob:instance-v1:{hash}:{}", payload.len()));
    let fixture = DownloadFixture {
        allowed,
        item,
        blob: BlobId::new(),
        key,
        byte_size: u64::try_from(payload.len()).expect("payload size"),
        blobs: LocalBlobStore::new(root.path()),
        _root: root,
        reads: reads.clone(),
        starts: starts.clone(),
    };
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
    .with_download(
        Arc::new(DownloadService::new(fixture.clone())),
        Arc::new(fixture),
    );
    (router(state), item, reads, starts)
}

#[tokio::test]
async fn download_routes_stream_ranges_head_conditionals_and_cancel_on_drop() {
    let (app, item, reads, starts) = download_app();
    let path = format!("/api/v1/items/{}/download", item.as_uuid());
    let ranged = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .header("Range", "bytes=65530-65550")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(ranged.headers()["content-length"], "21");
    assert_eq!(
        ranged.headers()["content-range"],
        "bytes 65530-65550/131089"
    );
    assert_eq!(ranged.headers()["accept-ranges"], "bytes");
    assert_eq!(ranged.headers()["content-type"], "application/epub+zip");
    assert_eq!(ranged.headers()["x-content-type-options"], "nosniff");
    assert!(
        ranged.headers()["content-disposition"]
            .to_str()
            .expect("header")
            .contains("filename*=UTF-8''")
    );
    let etag = ranged.headers()[ETAG].clone();
    let bytes = ranged.into_body().collect().await.expect("body").to_bytes();
    assert_eq!(bytes.len(), 21);

    let head = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()["content-length"], "131089");
    assert!(
        head.into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .is_empty()
    );

    let unchanged = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .header("If-None-Match", etag)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);

    for range in ["bytes=999999-", "bytes=0-1,4-5"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header("Cookie", "folioharbor_session=allowed")
                    .header("Range", range)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(response.headers()["content-range"], "bytes */131089");
    }

    assert_prompt_cancellation(app, &path, &reads).await;
    assert_eq!(
        *starts.lock().expect("download starts lock"),
        [
            DownloadRange {
                start: 65_530,
                end: 65_550,
            },
            DownloadRange {
                start: 0,
                end: 131_088,
            },
        ],
        "only successful GET responses record a download start"
    );
}

#[tokio::test]
async fn download_range_handles_repeated_fields_case_and_invalid_bytes() {
    let (app, item, _, starts) = download_app();
    let path = format!("/api/v1/items/{}/download", item.as_uuid());

    for method in ["GET", "HEAD"] {
        let repeated_range = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header("Cookie", "folioharbor_session=allowed")
                    .header("Range", "bytes=0-1")
                    .header("Range", "bytes=4-5")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(repeated_range.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }

    let mixed_case = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .header("Range", "ByTeS=2-5")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(mixed_case.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(mixed_case.headers()["content-range"], "bytes 2-5/131089");

    let invalid_range = HeaderValue::from_bytes(b"bytes=0-1\xff").expect("opaque header bytes");
    let invalid_range = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .header("Range", invalid_range)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(invalid_range.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    assert_eq!(
        *starts.lock().expect("download starts lock"),
        [DownloadRange { start: 2, end: 5 }]
    );
}

#[tokio::test]
async fn download_validators_handle_repeated_fields_weak_wildcard_and_invalid_bytes() {
    let (app, item, _, starts) = download_app();
    let path = format!("/api/v1/items/{}/download", item.as_uuid());
    let baseline = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let etag = baseline.headers()[ETAG].clone();

    let weak = HeaderValue::from_str(&format!(
        "W/{}",
        etag.to_str().expect("etag is visible ASCII")
    ))
    .expect("weak etag");
    let validator_in_later_field = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .header("If-None-Match", "\"not-current\"")
                .header("If-None-Match", weak)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(validator_in_later_field.status(), StatusCode::NOT_MODIFIED);

    let wildcard_in_later_field = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .header("If-None-Match", "\"not-current\"")
                .header("If-None-Match", "*")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(wildcard_in_later_field.status(), StatusCode::NOT_MODIFIED);

    let invalid_validator =
        HeaderValue::from_bytes(b"W/\"opaque\xff\"").expect("opaque header bytes");
    let invalid_validator = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .header("If-None-Match", invalid_validator)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(invalid_validator.status(), StatusCode::OK);

    assert_eq!(
        *starts.lock().expect("download starts lock"),
        [],
        "conditional HEAD requests never record a download start"
    );
}

#[tokio::test]
async fn download_validators_match_later_comma_list_members_for_get_and_head() {
    let (app, item, _, starts) = download_app();
    let path = format!("/api/v1/items/{}/download", item.as_uuid());
    let baseline = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let etag = baseline.headers()[ETAG]
        .to_str()
        .expect("etag is visible ASCII");

    let weak_list = format!("\"not-current\", W/{etag}");
    let weak_match = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .header("If-None-Match", weak_list)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(weak_match.status(), StatusCode::NOT_MODIFIED);

    let wildcard_match = app
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri(&path)
                .header("Cookie", "folioharbor_session=allowed")
                .header("If-None-Match", "\"not-current\", *")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(wildcard_match.status(), StatusCode::NOT_MODIFIED);
    assert!(starts.lock().expect("download starts lock").is_empty());
}

async fn assert_prompt_cancellation(app: axum::Router, path: &str, reads: &AtomicUsize) {
    reads.store(0, Ordering::SeqCst);
    let response = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header("Cookie", "folioharbor_session=allowed")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    let mut body = response.into_body();
    let first = body.frame().await.expect("frame").expect("bytes");
    assert_eq!(first.into_data().expect("data").len(), 64 * 1024);
    drop(body);
    tokio::task::yield_now().await;
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn progress_routes_return_etag_and_correlated_safe_conflict_positions() {
    let (app, _, manifestation) = app();
    let response = app
        .clone()
        .oneshot(request(
            &format!(
                "/api/v1/manifestations/{}/progress",
                manifestation.as_uuid()
            ),
            "allowed",
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(ETAG).and_then(|v| v.to_str().ok()),
        Some("\"progress-v3\"")
    );
    assert_eq!(
        response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("private, no-store")
    );

    let body = serde_json::json!({
        "accountId":allowed_user().as_uuid().to_string(),
        "deviceId":DeviceId::new().as_uuid().to_string(),"clientMutationId":Uuid::now_v7().to_string(),"baseVersion":2,
        "locator":{"href":"OPS/chapter.xhtml","type":"application/xhtml+xml","locations":{"progression":0.9},"extensions":{"version":1,"values":{}}}
    });
    let request = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/v1/manifestations/{}/progress",
            manifestation.as_uuid()
        ))
        .header("Cookie", "folioharbor_session=allowed")
        .header("X-CSRF-Token", "reader-csrf")
        .header("If-Match", "\"progress-v2\"")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.headers().get(ETAG).and_then(|v| v.to_str().ok()),
        Some("\"progress-v3\"")
    );
    assert_eq!(
        response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("private, no-store")
    );
    let body: serde_json::Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    assert_eq!(body["code"], "progress_conflict");
    assert_eq!(
        body["instance"]
            .as_str()
            .expect("instance")
            .split('/')
            .count(),
        3
    );
    assert_eq!(body["global"]["version"], 3);
    assert_eq!(body["global"]["locator"]["locations"]["progression"], 0.4);
    assert_eq!(body["device"]["locator"]["locations"]["progression"], 0.9);
    assert!(body.get("userId").is_none());
}

#[tokio::test]
async fn progress_empty_and_success_responses_are_private_and_not_stored() {
    let (app, _, manifestation) = app();
    let empty = app
        .clone()
        .oneshot(request(
            &format!(
                "/api/v1/manifestations/{}/progress",
                ManifestationId::new().as_uuid()
            ),
            "allowed",
        ))
        .await
        .expect("response");
    assert_eq!(empty.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        empty
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("private, no-store")
    );

    let body = serde_json::json!({
        "accountId": allowed_user().as_uuid().to_string(),
        "deviceId":DeviceId::new().as_uuid().to_string(),
        "clientMutationId":Uuid::now_v7().to_string(),
        "baseVersion":2,
        "locator":{"href":"OPS/chapter.xhtml","locations":{"progression":0.66},"extensions":{"version":1,"values":{}}}
    });
    let updated = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/manifestations/{}/progress",
                    manifestation.as_uuid()
                ))
                .header("Cookie", "folioharbor_session=allowed")
                .header("X-CSRF-Token", "reader-csrf")
                .header("If-Match", "\"progress-v2\"")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(
        updated
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("private, no-store")
    );
}

#[tokio::test]
async fn progress_put_requires_if_match_to_equal_json_base_version() {
    let (app, _, manifestation) = app();
    let body = serde_json::json!({"accountId":allowed_user().as_uuid().to_string(),"deviceId":DeviceId::new().as_uuid().to_string(),"clientMutationId":Uuid::now_v7().to_string(),"baseVersion":2,"locator":{"href":"OPS/chapter.xhtml","locations":{"progression":0.5},"extensions":{"version":1,"values":{}}}});
    let request = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/v1/manifestations/{}/progress",
            manifestation.as_uuid()
        ))
        .header("Cookie", "folioharbor_session=allowed")
        .header("X-CSRF-Token", "reader-csrf")
        .header("If-Match", "\"progress-v1\"")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    assert_eq!(body["code"], "invalid_progress_request");
    assert_eq!(body["fields"][0]["field"], "base_version");
    assert_eq!(body["fields"][0]["code"], "if_match_mismatch");
}

#[tokio::test]
async fn progress_put_rejects_an_account_that_differs_from_the_authenticated_actor() {
    let (app, _, manifestation) = app();
    let body = serde_json::json!({
        "accountId": UserId::new().as_uuid().to_string(),
        "deviceId": DeviceId::new().as_uuid().to_string(),
        "clientMutationId": Uuid::now_v7().to_string(),
        "baseVersion": 2,
        "locator": {
            "href": "OPS/chapter.xhtml",
            "locations": {"progression": 0.5},
            "extensions": {"version": 1, "values": {}}
        }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/manifestations/{}/progress",
                    manifestation.as_uuid()
                ))
                .header("Cookie", "folioharbor_session=allowed")
                .header("X-CSRF-Token", "reader-csrf")
                .header("If-Match", "\"progress-v2\"")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("private, no-store")
    );
    let body: serde_json::Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    assert_eq!(body["code"], "progress_account_mismatch");
    assert!(!body.to_string().contains("OPS/chapter.xhtml"));
}

#[tokio::test]
async fn progress_conflict_safely_represents_an_absent_global_position() {
    let (app, _, manifestation) = app();
    let body = serde_json::json!({
        "accountId": allowed_user().as_uuid().to_string(),
        "deviceId":DeviceId::new().as_uuid().to_string(),
        "clientMutationId":Uuid::now_v7().to_string(),
        "baseVersion":9,
        "locator":{"href":"OPS/chapter.xhtml","locations":{"progression":0.8},"extensions":{"version":1,"values":{}}}
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/manifestations/{}/progress",
                    manifestation.as_uuid()
                ))
                .header("Cookie", "folioharbor_session=allowed")
                .header("X-CSRF-Token", "reader-csrf")
                .header("If-Match", "\"progress-v9\"")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.headers().get(ETAG).and_then(|v| v.to_str().ok()),
        Some("\"progress-v0\"")
    );
    let body: serde_json::Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    assert_eq!(
        body["global"]["manifestationId"],
        manifestation.as_uuid().to_string()
    );
    assert_eq!(body["global"]["version"], 0);
    assert!(body["global"]["locator"].is_null());
    assert!(body["global"]["updatedAt"].is_null());
    assert_eq!(body["device"]["locator"]["locations"]["progression"], 0.8);
}

#[tokio::test]
async fn progress_mutation_mismatch_returns_only_correlated_problem_details() {
    let (app, _, manifestation) = app();
    let body = serde_json::json!({
        "accountId": allowed_user().as_uuid().to_string(),
        "deviceId":DeviceId::new().as_uuid().to_string(),
        "clientMutationId":Uuid::now_v7().to_string(),
        "baseVersion":2,
        "locator":{"href":"OPS/chapter.xhtml","locations":{"progression":0.77},"extensions":{"version":1,"values":{}}}
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/manifestations/{}/progress",
                    manifestation.as_uuid()
                ))
                .header("Cookie", "folioharbor_session=allowed")
                .header("X-CSRF-Token", "reader-csrf")
                .header("If-Match", "\"progress-v2\"")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json")
    );
    let body: serde_json::Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    assert_eq!(body["code"], "progress_mutation_mismatch");
    assert!(body["instance"].as_str().is_some());
    assert!(body.get("global").is_none());
    assert!(body.get("device").is_none());
    assert!(!body.to_string().contains("OPS/chapter.xhtml"));
}

#[tokio::test]
async fn progress_mutation_capacity_is_correlated_private_and_retryable() {
    let (app, _, manifestation) = app();
    let body = serde_json::json!({
        "accountId": allowed_user().as_uuid().to_string(),
        "deviceId":DeviceId::new().as_uuid().to_string(),
        "clientMutationId":Uuid::now_v7().to_string(),
        "baseVersion":2,
        "locator":{"href":"OPS/chapter.xhtml","locations":{"progression":0.88},"extensions":{"version":1,"values":{}}}
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/manifestations/{}/progress",
                    manifestation.as_uuid()
                ))
                .header("Cookie", "folioharbor_session=allowed")
                .header("X-CSRF-Token", "reader-csrf")
                .header("If-Match", "\"progress-v2\"")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response
            .headers()
            .get("Retry-After")
            .and_then(|value| value.to_str().ok()),
        Some("17")
    );
    assert_eq!(
        response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("private, no-store")
    );
    let body: serde_json::Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    assert_eq!(body["code"], "rate_limited");
    assert!(body["instance"].as_str().is_some());
    assert!(body.get("global").is_none());
    assert!(body.get("device").is_none());
}

fn request(uri: &str, actor: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("Cookie", format!("folioharbor_session={actor}"))
        .body(Body::empty())
        .expect("request")
}

fn manifest_golden(item: ItemId, manifestation: ManifestationId) -> serde_json::Value {
    serde_json::json!({
        "metadata": {
            "title": "Safe Book",
            "authors": ["Writer"],
            "languages": ["en"]
        },
        "manifestationId": manifestation.as_uuid().to_string(),
        "readingOrder": [{
            "href": format!("/api/v1/items/{}/resources/safe", item.as_uuid()),
            "type": "application/xhtml+xml"
        }],
        "resources": [
            {
                "href": format!("/api/v1/items/{}/resources/cover", item.as_uuid()),
                "type": "image/png",
                "rel": "resource"
            },
            {
                "href": format!("/api/v1/items/{}/resources/styles", item.as_uuid()),
                "type": "text/css",
                "rel": "resource"
            }
        ],
        "toc": [{
            "href": format!("/api/v1/items/{}/resources/safe#start", item.as_uuid()),
            "type": "application/xhtml+xml",
            "title": "Chapter 1"
        }],
        "links": [{
            "href": format!("/api/v1/items/{}/manifest", item.as_uuid()),
            "type": "application/webpub+json",
            "rel": "self"
        }]
    })
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
    assert_eq!(json, manifest_golden(item, manifestation));
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
            "default-src 'none'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; font-src 'self' data: blob:; script-src 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'self'"
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

    for validator in ["W/\"resource-test-v1\"", "*"] {
        let conditional = Request::builder()
            .uri(format!("/api/v1/items/{}/resources/safe", item.as_uuid()))
            .header("Cookie", "folioharbor_session=allowed")
            .header("If-None-Match", validator)
            .body(Body::empty())
            .expect("conditional request");
        let response = app.clone().oneshot(conditional).await.expect("conditional");
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }

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
    publication: Arc<ReaderPublication>,
}
#[async_trait]
impl ReaderCatalogRepository for ReadableCatalog {
    async fn find_readable_publication(
        &self,
        actor: UserId,
        _: ItemId,
        _: RequestId,
    ) -> Result<Option<Arc<ReaderPublication>>, ReaderCatalogError> {
        Ok((actor == self.allowed).then(|| self.publication.clone()))
    }
}

fn malicious_archive() -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("OPS/chapter.xhtml", SimpleFileOptions::default())
        .expect("entry");
    writer.write_all(br#"<html><head><meta http-equiv="refresh" content="0;url=https://evil.test"></head><body onload="steal()"><script>steal()</script><form>x</form><iframe src="https://evil.test"></iframe><object>x</object><p onclick="x()">safe boundary</p></body></html>"#).expect("html");
    writer
        .start_file("OPS/book.css", SimpleFileOptions::default())
        .expect("css entry");
    writer.write_all(br"@import url('https://evil.test/import.css'); p{color:red;background-image:url(cover.png);list-style-image:url(javascript:alert(1))}").expect("css");
    writer
        .start_file("OPS/cover.png", SimpleFileOptions::default())
        .expect("image entry");
    writer.write_all(b"PNG").expect("image");
    writer.finish().expect("archive").into_inner()
}

fn real_reader_app() -> (axum::Router, ItemId, PublicationPackageId) {
    let allowed = UserId::new();
    let item = ItemId::new();
    let package = PublicationPackageId::new();
    let publication = Arc::new(ReaderPublication::new(
        LibraryId::new(),
        item,
        ManifestationId::new(),
        package,
        BlobId::new(),
        StorageKey::from_opaque("blob:test".to_owned()),
        "epub3-v1".to_owned(),
        "Unsafe input".to_owned(),
        Vec::new(),
        Vec::new(),
        vec![
            ReaderResource {
                normalized_href: "OPS/chapter.xhtml".to_owned(),
                media_type: "application/xhtml+xml".to_owned(),
            },
            ReaderResource {
                normalized_href: "OPS/book.css".to_owned(),
                media_type: "text/css".to_owned(),
            },
            ReaderResource {
                normalized_href: "OPS/cover.png".to_owned(),
                media_type: "image/png".to_owned(),
            },
        ],
        vec![ReaderSpineEntry {
            normalized_href: "OPS/chapter.xhtml".to_owned(),
            linear: true,
        }],
        Vec::new(),
    ));
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
    (router(state), item, package)
}

#[tokio::test]
async fn malicious_epub_content_is_sanitized_through_the_real_http_boundary() {
    let (app, item, package) = real_reader_app();
    let resource_id = ResourceId::for_resource(package, "OPS/chapter.xhtml");
    let response = app
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

#[tokio::test]
async fn standalone_css_is_sanitized_and_isolated_through_the_real_http_boundary() {
    let (app, item, package) = real_reader_app();
    let css_id = ResourceId::for_resource(package, "OPS/book.css");
    let image_id = ResourceId::for_resource(package, "OPS/cover.png");
    let response = app
        .oneshot(request(
            &format!(
                "/api/v1/items/{}/resources/{}",
                item.as_uuid(),
                css_id.as_str()
            ),
            "allowed",
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/css")
    );
    assert_eq!(
        response
            .headers()
            .get(CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok()),
        Some(
            "default-src 'none'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; font-src 'self' data: blob:; script-src 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'self'"
        )
    );
    assert_eq!(
        response
            .headers()
            .get(X_CONTENT_TYPE_OPTIONS)
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(
        response
            .headers()
            .get(REFERRER_POLICY)
            .and_then(|value| value.to_str().ok()),
        Some("no-referrer")
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let css = String::from_utf8_lossy(&body).to_ascii_lowercase();
    assert!(css.contains("color:red"), "{css}");
    assert!(
        css.contains(&format!(
            "/api/v1/items/{}/resources/{}",
            item.as_uuid(),
            image_id.as_str().to_ascii_lowercase()
        )),
        "{css}"
    );
    for forbidden in ["@import", "https://", "javascript:", "OPS/", "resource:"] {
        assert!(!css.contains(forbidden), "found {forbidden}: {css}");
    }
}
