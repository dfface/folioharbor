use axum::{
    Router,
    body::Body,
    extract::{Extension, Path, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{
            ACCEPT_RANGES, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE,
            CONTENT_TYPE, ETAG, IF_NONE_MATCH, RANGE, X_CONTENT_TYPE_OPTIONS,
        },
    },
    response::Response,
    routing::get,
};
use bytes::Bytes;
use folioharbor_application::{
    catalog::{DownloadGrant, DownloadRange},
    error::{AppError, FieldViolation},
    ports::BlobStore,
};
use folioharbor_domain::{
    id::{ItemId, RequestId},
    imports::blob::StorageKey,
};
use futures_util::stream;
use std::{io, sync::Arc};
use uuid::Uuid;

use super::AppState;
use crate::{
    auth::AuthenticatedActor,
    problem::{ProblemContext, response as problem_response},
};

const STREAM_CHUNK_BYTES: u64 = 64 * 1024;

pub fn router() -> Router<AppState> {
    Router::new().route("/{item_id}/download", get(get_download).head(head_download))
}

async fn get_download(
    state: State<AppState>,
    context: Extension<ProblemContext>,
    request_id: Extension<RequestId>,
    actor: AuthenticatedActor,
    item: Path<String>,
    headers: HeaderMap,
) -> Response {
    download(state, context, request_id, actor, item, headers, false).await
}

async fn head_download(
    state: State<AppState>,
    context: Extension<ProblemContext>,
    request_id: Extension<RequestId>,
    actor: AuthenticatedActor,
    item: Path<String>,
    headers: HeaderMap,
) -> Response {
    download(state, context, request_id, actor, item, headers, true).await
}

async fn download(
    State(state): State<AppState>,
    Extension(context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path(raw_item): Path<String>,
    headers: HeaderMap,
    head: bool,
) -> Response {
    let item_id = match Uuid::parse_str(&raw_item) {
        Ok(value) => ItemId::from_uuid(value),
        Err(_) => {
            return problem_response(
                &AppError::BadRequest {
                    code: "invalid_identifier",
                    fields: vec![FieldViolation {
                        field: "item_id",
                        code: "invalid_uuid",
                    }],
                },
                &context,
            );
        }
    };
    let grant = match state
        .download_api
        .authorize(actor, item_id, request_id)
        .await
    {
        Ok(grant) => grant,
        Err(error) => return problem_response(&error, &context),
    };
    if matches_validator(&headers, grant.etag()) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        common_headers(response.headers_mut(), grant.etag());
        return response;
    }
    let requested = match headers.get(RANGE) {
        Some(value) => match value.to_str() {
            Ok(value) => Some(value),
            Err(_) => return range_not_satisfiable(grant.byte_size(), grant.etag()),
        },
        None => None,
    };
    let Ok(range) = resolve_range(requested, grant.byte_size()) else {
        return range_not_satisfiable(grant.byte_size(), grant.etag());
    };
    if head {
        return success_response(&grant, requested.is_some(), range, None);
    }
    let Some(blobs) = state.download_blobs else {
        return problem_response(
            &AppError::DependencyUnavailable {
                code: "blob_store_unavailable",
            },
            &context,
        );
    };
    if let Err(error) = state
        .download_api
        .record_start(
            actor,
            item_id,
            request_id,
            DownloadRange {
                start: range.start,
                end: range.end,
            },
        )
        .await
    {
        return problem_response(&error, &context);
    }
    success_response(&grant, requested.is_some(), range, Some(blobs))
}

fn success_response(
    grant: &DownloadGrant,
    partial: bool,
    range: ByteRange,
    blobs: Option<Arc<dyn BlobStore>>,
) -> Response {
    let body = blobs.map_or_else(Body::empty, |blobs| {
        streaming_body(blobs, grant.storage_identity().clone(), range)
    });
    let mut response = Response::new(body);
    *response.status_mut() = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    common_headers(response.headers_mut(), grant.etag());
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/epub+zip"),
    );
    insert_header(
        response.headers_mut(),
        CONTENT_LENGTH,
        &range.length().to_string(),
    );
    if partial {
        insert_header(
            response.headers_mut(),
            CONTENT_RANGE,
            &format!("bytes {}-{}/{}", range.start, range.end, grant.byte_size()),
        );
    }
    if let Ok(value) = HeaderValue::from_str(&content_disposition(grant.safe_file_name())) {
        response.headers_mut().insert(CONTENT_DISPOSITION, value);
    }
    response
}

fn insert_header(headers: &mut HeaderMap, name: axum::http::HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

fn streaming_body(blobs: Arc<dyn BlobStore>, key: StorageKey, range: ByteRange) -> Body {
    let stream = stream::unfold(
        (blobs, key, range.start, range.length()),
        |(blobs, key, offset, remaining)| async move {
            if remaining == 0 {
                return None;
            }
            let length = remaining.min(STREAM_CHUNK_BYTES);
            match blobs.read_range(&key, offset, length).await {
                Ok(bytes) if !bytes.is_empty() && bytes.len() as u64 <= length => {
                    let count = bytes.len() as u64;
                    Some((
                        Ok::<_, io::Error>(Bytes::from(bytes)),
                        (blobs, key, offset + count, remaining - count),
                    ))
                }
                Ok(_) => Some((
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "blob truncated",
                    )),
                    (blobs, key, offset, 0),
                )),
                Err(_) => Some((
                    Err(io::Error::other("blob read failed")),
                    (blobs, key, offset, 0),
                )),
            }
        },
    );
    Body::from_stream(stream)
}

fn common_headers(headers: &mut HeaderMap, etag: &str) {
    headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(ETAG, value);
    }
}

fn range_not_satisfiable(size: u64, etag: &str) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
    common_headers(response.headers_mut(), etag);
    insert_header(
        response.headers_mut(),
        CONTENT_RANGE,
        &format!("bytes */{size}"),
    );
    response
}

fn matches_validator(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
            })
        })
}

fn content_disposition(file_name: &str) -> String {
    let clean = file_name
        .chars()
        .filter(|character| !character.is_control() && !matches!(character, '/' | '\\'))
        .collect::<String>();
    let utf8 = if clean.is_empty() {
        "publication.epub"
    } else {
        clean.as_str()
    };
    let fallback = utf8
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!(
        "attachment; filename=\"{fallback}\"; filename*=UTF-8''{}",
        percent_encode(utf8.as_bytes())
    )
}

fn percent_encode(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for &byte in value {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            )
        {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    const fn length(self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RangeError;

fn resolve_range(value: Option<&str>, size: u64) -> Result<ByteRange, RangeError> {
    if size == 0 {
        return Err(RangeError);
    }
    let Some(value) = value else {
        return Ok(ByteRange {
            start: 0,
            end: size - 1,
        });
    };
    let range = value.strip_prefix("bytes=").ok_or(RangeError)?;
    if range.contains(',') {
        return Err(RangeError);
    }
    let (start, end) = range.split_once('-').ok_or(RangeError)?;
    match (start.is_empty(), end.is_empty()) {
        (true, false) => {
            let suffix = end.parse::<u64>().map_err(|_| RangeError)?;
            if suffix == 0 {
                return Err(RangeError);
            }
            Ok(ByteRange {
                start: size.saturating_sub(suffix),
                end: size - 1,
            })
        }
        (false, true) => {
            let start = start.parse::<u64>().map_err(|_| RangeError)?;
            if start >= size {
                return Err(RangeError);
            }
            Ok(ByteRange {
                start,
                end: size - 1,
            })
        }
        (false, false) => {
            let start = start.parse::<u64>().map_err(|_| RangeError)?;
            let requested_end = end.parse::<u64>().map_err(|_| RangeError)?;
            if start > requested_end || start >= size {
                return Err(RangeError);
            }
            Ok(ByteRange {
                start,
                end: requested_end.min(size - 1),
            })
        }
        (true, true) => Err(RangeError),
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteRange, RangeError, content_disposition, resolve_range};

    #[test]
    fn resolves_full_prefix_suffix_and_open_ranges() {
        assert_eq!(resolve_range(None, 10), Ok(ByteRange { start: 0, end: 9 }));
        assert_eq!(
            resolve_range(Some("bytes=2-5"), 10),
            Ok(ByteRange { start: 2, end: 5 })
        );
        assert_eq!(
            resolve_range(Some("bytes=-4"), 10),
            Ok(ByteRange { start: 6, end: 9 })
        );
        assert_eq!(
            resolve_range(Some("bytes=7-"), 10),
            Ok(ByteRange { start: 7, end: 9 })
        );
    }

    #[test]
    fn rejects_multiple_malformed_and_unsatisfiable_ranges() {
        assert_eq!(resolve_range(Some("bytes=0-1,4-5"), 10), Err(RangeError));
        assert_eq!(resolve_range(Some("items=0-1"), 10), Err(RangeError));
        assert_eq!(resolve_range(Some("bytes=10-"), 10), Err(RangeError));
        assert_eq!(resolve_range(Some("bytes=-0"), 10), Err(RangeError));
        assert_eq!(resolve_range(Some("bytes=5-4"), 10), Err(RangeError));
    }

    #[test]
    fn disposition_has_safe_ascii_and_rfc5987_names() {
        assert_eq!(
            content_disposition("危 险.epub"),
            "attachment; filename=\"___.epub\"; filename*=UTF-8''%E5%8D%B1%20%E9%99%A9.epub"
        );
        let value = content_disposition("../bad\r\nbook.epub");
        assert!(!value.contains('\r'));
        assert!(!value.contains('\n'));
        assert!(!value.contains('/'));
    }
}
