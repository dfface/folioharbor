use super::AppState;
use crate::{
    auth::AuthenticatedActor,
    problem::{ProblemContext, response as problem_response},
};
use axum::{
    Json, Router,
    extract::rejection::QueryRejection,
    extract::{Extension, Path, Query, State},
    http::{HeaderValue, header::ETAG},
    response::{IntoResponse, Response},
    routing::get,
};
use folioharbor_application::{
    catalog::{BookSummary, ItemDetail, PageRequest},
    error::{AppError, FieldViolation},
};
use folioharbor_domain::id::{ItemId, LibraryId, RequestId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{library_id}/books", get(list_books))
        .route("/{library_id}/items/{item_id}", get(get_item))
}

#[derive(Deserialize)]
struct PageQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}

#[derive(Serialize)]
struct BookResponse {
    item_id: String,
    primary_title: String,
    authors: Vec<String>,
    languages: Vec<String>,
    media_type: String,
    can_read: bool,
    can_download: bool,
}

impl From<BookSummary> for BookResponse {
    fn from(value: BookSummary) -> Self {
        Self {
            item_id: value.item_id.as_uuid().to_string(),
            primary_title: value.primary_title,
            authors: value.authors,
            languages: value.languages,
            media_type: value.media_type,
            can_read: value.can_read,
            can_download: value.can_download,
        }
    }
}

#[derive(Serialize)]
struct PageResponse {
    items: Vec<BookResponse>,
    next_cursor: Option<String>,
}

#[derive(Serialize)]
struct DetailResponse {
    item_id: String,
    manifestation_id: String,
    primary_title: String,
    authors: Vec<String>,
    languages: Vec<String>,
    identifiers: Vec<String>,
    media_type: String,
    can_read: bool,
    can_download: bool,
}

async fn list_books(
    State(state): State<AppState>,
    Extension(context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path(raw_library): Path<String>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let library_id = match parse_library(&raw_library) {
        Ok(value) => value,
        Err(error) => return problem_response(&error, &context),
    };
    let Ok(Query(query)) = query else {
        return problem_response(
            &AppError::Invalid {
                code: "invalid_page",
                fields: vec![FieldViolation {
                    field: "query",
                    code: "invalid_query",
                }],
            },
            &context,
        );
    };
    match state
        .catalog_api
        .list_library_books(
            actor.user_id,
            library_id,
            request_id,
            PageRequest {
                cursor: query.cursor,
                limit: query.limit,
            },
        )
        .await
    {
        Ok(page) => Json(PageResponse {
            items: page.items.into_iter().map(Into::into).collect(),
            next_cursor: page.next_cursor,
        })
        .into_response(),
        Err(error) => problem_response(&error, &context),
    }
}

async fn get_item(
    State(state): State<AppState>,
    Extension(context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path((raw_library, raw_item)): Path<(String, String)>,
) -> Response {
    let (library_id, item_id) = match parse_ids(&raw_library, &raw_item) {
        Ok(value) => value,
        Err(error) => return problem_response(&error, &context),
    };
    match state
        .catalog_api
        .get_item(actor.user_id, library_id, item_id, request_id)
        .await
    {
        Ok(detail) => detail_response(detail),
        Err(error) => problem_response(&error, &context),
    }
}

fn detail_response(detail: ItemDetail) -> Response {
    let etag = HeaderValue::from_str(&detail.etag).ok();
    let mut response = Json(DetailResponse {
        item_id: detail.item_id.as_uuid().to_string(),
        manifestation_id: detail.manifestation_id.as_uuid().to_string(),
        primary_title: detail.primary_title,
        authors: detail.authors,
        languages: detail.languages,
        identifiers: detail.identifiers,
        media_type: detail.media_type,
        can_read: detail.can_read,
        can_download: detail.can_download,
    })
    .into_response();
    if let Some(etag) = etag {
        response.headers_mut().insert(ETAG, etag);
    }
    response
}

fn parse_library(raw: &str) -> Result<LibraryId, AppError> {
    Uuid::parse_str(raw)
        .map(LibraryId::from_uuid)
        .map_err(|_| invalid_id("library_id"))
}

fn parse_ids(library: &str, item: &str) -> Result<(LibraryId, ItemId), AppError> {
    Ok((
        parse_library(library)?,
        Uuid::parse_str(item)
            .map(ItemId::from_uuid)
            .map_err(|_| invalid_id("item_id"))?,
    ))
}

fn invalid_id(field: &'static str) -> AppError {
    AppError::Invalid {
        code: "invalid_identifier",
        fields: vec![FieldViolation {
            field,
            code: "invalid_uuid",
        }],
    }
}
