#![allow(clippy::expect_used)]

use async_trait::async_trait;
use folioharbor_application::{
    actor::Actor,
    catalog::{
        DownloadAuthorization, DownloadItem, DownloadRepository, DownloadRepositoryError,
        DownloadSource,
    },
};
use folioharbor_domain::{
    id::{BlobId, ItemId, RequestId, SessionId, UserId},
    imports::blob::StorageKey,
};
use std::sync::Mutex;

struct Repository {
    authorization: Mutex<DownloadAuthorization>,
}

#[async_trait]
impl DownloadRepository for Repository {
    async fn authorize_download(
        &self,
        _: Actor,
        _: ItemId,
        _: RequestId,
    ) -> Result<DownloadAuthorization, DownloadRepositoryError> {
        Ok(self.authorization.lock().expect("source lock").clone())
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
        authorization: Mutex::new(DownloadAuthorization::Granted(DownloadSource::new(
            blob,
            StorageKey::from_opaque("blob:secret-location".to_owned()),
            131_073,
            "../危\n险/book.epub".to_owned(),
        ))),
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
fn authorization_debug_never_exposes_secret_download_source_metadata() {
    let blob = BlobId::new();
    let authorization = DownloadAuthorization::Granted(DownloadSource::new(
        blob,
        StorageKey::from_opaque("blob:secret-debug-location".to_owned()),
        16,
        "secret-debug-name.epub".to_owned(),
    ));

    let debug = format!("{authorization:?}");
    assert!(!debug.contains(&blob.as_uuid().to_string()));
    assert!(!debug.contains("secret-debug-location"));
    assert!(!debug.contains("secret-debug-name"));
    assert!(debug.contains("Granted"));
}

#[tokio::test]
async fn filename_sanitization_strips_unicode_direction_and_format_controls() {
    let repository = Repository {
        authorization: Mutex::new(DownloadAuthorization::Granted(DownloadSource::new(
            BlobId::new(),
            StorageKey::from_opaque("blob:opaque".to_owned()),
            16,
            "../\u{202e}gpj.exe/\u{2066}safe\u{2069}\u{200f}.epub".to_owned(),
        ))),
    };

    let grant = DownloadItem::new(&repository)
        .authorize(actor(), ItemId::new(), RequestId::new())
        .await
        .expect("authorized");
    assert_eq!(grant.safe_file_name(), "safe.epub");
}

#[tokio::test]
async fn missing_or_unauthorized_items_are_anti_enumerating_not_found() {
    let repository = Repository {
        authorization: Mutex::new(DownloadAuthorization::NotFound),
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
        authorization: Mutex::new(DownloadAuthorization::Forbidden),
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
