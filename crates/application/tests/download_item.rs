#![allow(clippy::expect_used)]

use async_trait::async_trait;
use folioharbor_application::{
    actor::Actor,
    catalog::DownloadItem,
    ports::{
        DownloadAuthorization, DownloadRepository, DownloadRepositoryError, DownloadSourceReceiver,
    },
};
use folioharbor_domain::{
    id::{BlobId, ItemId, RequestId, SessionId, UserId},
    imports::blob::StorageKey,
};
struct Repository {
    authorization: DownloadAuthorization,
    source: Option<(BlobId, StorageKey, u64, String)>,
}

#[derive(Clone, Copy)]
enum ProtocolDecision {
    Granted,
    Forbidden,
    NotFound,
}

struct ProtocolRepository {
    decision: ProtocolDecision,
    deliveries: usize,
}

#[async_trait]
impl DownloadRepository for ProtocolRepository {
    async fn authorize_download(
        &self,
        _: Actor,
        _: ItemId,
        _: RequestId,
        receiver: &mut dyn DownloadSourceReceiver,
    ) -> Result<DownloadAuthorization, DownloadRepositoryError> {
        for _ in 0..self.deliveries {
            receiver.receive(
                BlobId::new(),
                StorageKey::from_opaque("blob:protocol-secret".to_owned()),
                16,
                "protocol-secret.epub".to_owned(),
            );
        }
        Ok(match self.decision {
            ProtocolDecision::Granted => DownloadAuthorization::Granted,
            ProtocolDecision::Forbidden => DownloadAuthorization::Forbidden,
            ProtocolDecision::NotFound => DownloadAuthorization::NotFound,
        })
    }
}

#[async_trait]
impl DownloadRepository for Repository {
    async fn authorize_download(
        &self,
        _: Actor,
        _: ItemId,
        _: RequestId,
        receiver: &mut dyn DownloadSourceReceiver,
    ) -> Result<DownloadAuthorization, DownloadRepositoryError> {
        match &self.authorization {
            DownloadAuthorization::Granted => {
                let (blob_id, storage_identity, byte_size, file_name) =
                    self.source.as_ref().expect("granted source");
                receiver.receive(
                    *blob_id,
                    storage_identity.clone(),
                    *byte_size,
                    file_name.clone(),
                );
                Ok(DownloadAuthorization::Granted)
            }
            DownloadAuthorization::Forbidden => Ok(DownloadAuthorization::Forbidden),
            DownloadAuthorization::NotFound => Ok(DownloadAuthorization::NotFound),
        }
    }
}

fn actor() -> Actor {
    Actor {
        user_id: UserId::new(),
        session_id: SessionId::new(),
    }
}

#[tokio::test]
async fn authorization_returns_only_streaming_metadata_with_an_opaque_strong_etag() {
    let blob = BlobId::new();
    let repository = Repository {
        authorization: DownloadAuthorization::Granted,
        source: Some((
            blob,
            StorageKey::from_opaque("blob:secret-location".to_owned()),
            131_073,
            "../危\n险/book.epub".to_owned(),
        )),
    };

    let grant = DownloadItem::new(&repository)
        .authorize(actor(), ItemId::new(), RequestId::new())
        .await
        .expect("authorized");

    assert_eq!(grant.byte_size(), 131_073);
    assert_eq!(grant.media_type(), "application/epub+zip");
    assert_eq!(grant.safe_file_name(), "book.epub");
    assert!(grant.etag().starts_with('"') && grant.etag().ends_with('"'));
    assert!(!grant.etag().contains(&blob.as_uuid().to_string()));
    assert!(!grant.etag().contains("secret-location"));
}

#[test]
fn authorization_decision_debug_has_no_secret_payload() {
    let authorization = DownloadAuthorization::Granted;

    assert_eq!(format!("{authorization:?}"), "Granted");
}

#[tokio::test]
async fn filename_sanitization_strips_unicode_direction_and_format_controls() {
    let repository = Repository {
        authorization: DownloadAuthorization::Granted,
        source: Some((
            BlobId::new(),
            StorageKey::from_opaque("blob:opaque".to_owned()),
            16,
            "../\u{202e}gpj.exe/\u{2066}\u{00ad}安\u{180e}全\u{fff9}e\u{301}😀\u{e0001}\u{2069}\u{200f}.epub"
                .to_owned(),
        )),
    };

    let grant = DownloadItem::new(&repository)
        .authorize(actor(), ItemId::new(), RequestId::new())
        .await
        .expect("authorized");
    assert_eq!(grant.safe_file_name(), "安全e\u{301}😀.epub");
}

#[tokio::test]
async fn missing_or_unauthorized_items_are_anti_enumerating_not_found() {
    let repository = Repository {
        authorization: DownloadAuthorization::NotFound,
        source: None,
    };
    let error = DownloadItem::new(&repository)
        .authorize(actor(), ItemId::new(), RequestId::new())
        .await
        .expect_err("must be hidden");
    assert!(matches!(
        error,
        folioharbor_application::error::AppError::NotFound {
            code: "item_not_found"
        }
    ));
}

#[tokio::test]
async fn visible_reader_without_download_setting_is_forbidden_not_read_fallback() {
    let repository = Repository {
        authorization: DownloadAuthorization::Forbidden,
        source: None,
    };
    let error = DownloadItem::new(&repository)
        .authorize(actor(), ItemId::new(), RequestId::new())
        .await
        .expect_err("item.read must not grant download");
    assert!(matches!(
        error,
        folioharbor_application::error::AppError::Forbidden {
            code: "item_download_forbidden"
        }
    ));
}

#[tokio::test]
async fn granted_authorization_requires_exactly_one_source_delivery() {
    for deliveries in [0, 2] {
        let repository = ProtocolRepository {
            decision: ProtocolDecision::Granted,
            deliveries,
        };
        let error = DownloadItem::new(&repository)
            .authorize(actor(), ItemId::new(), RequestId::new())
            .await
            .expect_err("malformed repository response must fail closed");
        assert!(matches!(
            error,
            folioharbor_application::error::AppError::DependencyUnavailable {
                code: "download_repository_unavailable"
            }
        ));
    }
}

#[tokio::test]
async fn denial_decisions_ignore_any_delivered_source() {
    for decision in [ProtocolDecision::Forbidden, ProtocolDecision::NotFound] {
        let repository = ProtocolRepository {
            decision,
            deliveries: 1,
        };
        let error = DownloadItem::new(&repository)
            .authorize(actor(), ItemId::new(), RequestId::new())
            .await
            .expect_err("denial decision must win");
        assert!(matches!(
            (decision, error),
            (
                ProtocolDecision::Forbidden,
                folioharbor_application::error::AppError::Forbidden {
                    code: "item_download_forbidden"
                }
            ) | (
                ProtocolDecision::NotFound,
                folioharbor_application::error::AppError::NotFound {
                    code: "item_not_found"
                }
            )
        ));
    }
}
