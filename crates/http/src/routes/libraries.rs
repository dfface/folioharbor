use super::AppState;
use crate::{
    auth::AuthenticatedActor,
    json::ApiJson,
    problem::{ProblemContext, response as problem_response},
};
use axum::{
    Json, Router,
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use folioharbor_application::{
    config::AuthFeatures,
    error::{AppError, FieldViolation},
    libraries::{
        AcceptLibraryInvitationRequest, ChangeLibraryMemberRequest, InvitationAcceptance,
        InviteLibraryMemberRequest, LibraryCapabilities, ListLibrariesRequest, ReadLibraryRequest,
        RemoveLibraryMemberRequest, UpdateSettingsRequest,
    },
};
use folioharbor_domain::{
    id::{LibraryId, RequestId, UserId},
    libraries::role::RoleCode,
};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};

pub fn router(auth_features: Option<AuthFeatures>) -> Router<AppState> {
    let router = Router::new()
        .route("/", get(list_libraries))
        .route("/{library_id}", get(get_library))
        .route("/{library_id}/settings", patch(update_settings))
        .route("/{library_id}/members", get(list_members))
        .route(
            "/{library_id}/members/{user_id}",
            patch(change_member).delete(remove_member),
        );
    if auth_features.is_none_or(AuthFeatures::invitation_enabled) {
        router.route("/{library_id}/invitations", post(invite_member))
    } else {
        router
    }
}

pub fn invitation_router() -> Router<AppState> {
    Router::new().route("/accept", post(accept_invitation))
}

#[derive(Serialize)]
struct LibraryResponse {
    library_id: String,
    name: String,
    role: &'static str,
    reader_download_enabled: bool,
    capabilities: LibraryCapabilitiesResponse,
}
#[derive(Serialize)]
struct LibraryCapabilitiesResponse {
    can_upload: bool,
    can_invite_members: bool,
    can_manage_members: bool,
    can_manage_settings: bool,
}
#[derive(Serialize)]
struct MemberResponse {
    user_id: String,
    role: &'static str,
}
#[derive(Deserialize)]
struct SettingsBody {
    name: String,
    #[serde(default)]
    reader_download_enabled: Option<bool>,
}
#[derive(Deserialize)]
struct RoleBody {
    role: String,
}
#[derive(Deserialize)]
struct InvitationBody {
    email: String,
    role: String,
}
#[derive(Deserialize)]
struct AcceptInvitationBody {
    token: String,
}
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum AcceptInvitationResponse {
    Accepted { library_id: String },
    WrongAccount { email_hint: String },
    Unverified,
    Expired,
    Consumed,
    Invalid,
}

impl From<LibraryCapabilities> for LibraryCapabilitiesResponse {
    fn from(value: LibraryCapabilities) -> Self {
        Self {
            can_upload: value.can_upload,
            can_invite_members: value.can_invite_members,
            can_manage_members: value.can_manage_members,
            can_manage_settings: value.can_manage_settings,
        }
    }
}

impl From<folioharbor_application::libraries::LibraryView> for LibraryResponse {
    fn from(value: folioharbor_application::libraries::LibraryView) -> Self {
        Self {
            library_id: value.library_id.as_uuid().to_string(),
            name: value.name,
            role: value.role.as_str(),
            reader_download_enabled: value.reader_download_enabled,
            capabilities: value.capabilities.into(),
        }
    }
}

fn library_id(raw: &str) -> Result<LibraryId, AppError> {
    uuid::Uuid::parse_str(raw)
        .map(LibraryId::from_uuid)
        .map_err(|_| invalid("library_id", "invalid_library_id"))
}
fn user_id(raw: &str) -> Result<UserId, AppError> {
    uuid::Uuid::parse_str(raw)
        .map(UserId::from_uuid)
        .map_err(|_| invalid("user_id", "invalid_user_id"))
}
fn role(raw: &str) -> Result<RoleCode, AppError> {
    RoleCode::parse(raw).ok_or_else(|| invalid("role", "invalid_role"))
}
fn invalid(field: &'static str, code: &'static str) -> AppError {
    AppError::Invalid {
        code,
        fields: vec![FieldViolation { field, code }],
    }
}
fn problem(error: &AppError, context: &ProblemContext) -> Response {
    problem_response(error, context)
}

async fn list_libraries(
    State(state): State<AppState>,
    Extension(problem_context): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
) -> Response {
    match state
        .library_api
        .list_libraries(ListLibrariesRequest {
            actor: actor.user_id,
            request_id,
        })
        .await
    {
        Ok(values) => Json(
            values
                .into_iter()
                .map(LibraryResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => problem(&error, &problem_context),
    }
}
async fn get_library(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path(raw): Path<String>,
) -> Response {
    let id = match library_id(&raw) {
        Ok(v) => v,
        Err(e) => return problem(&e, &ctx),
    };
    match state
        .library_api
        .get_library(ReadLibraryRequest {
            actor: actor.user_id,
            request_id,
            library_id: id,
        })
        .await
    {
        Ok(v) => Json(LibraryResponse::from(v)).into_response(),
        Err(e) => problem(&e, &ctx),
    }
}
async fn list_members(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path(raw): Path<String>,
) -> Response {
    let id = match library_id(&raw) {
        Ok(v) => v,
        Err(e) => return problem(&e, &ctx),
    };
    match state
        .library_api
        .list_members(ReadLibraryRequest {
            actor: actor.user_id,
            request_id,
            library_id: id,
        })
        .await
    {
        Ok(v) => Json(
            v.into_iter()
                .map(|m| MemberResponse {
                    user_id: m.user_id.as_uuid().to_string(),
                    role: m.role.as_str(),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => problem(&e, &ctx),
    }
}
async fn update_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path(raw): Path<String>,
    ApiJson(body): ApiJson<SettingsBody>,
) -> Response {
    let id = match library_id(&raw) {
        Ok(v) => v,
        Err(e) => return problem(&e, &ctx),
    };
    mutation(
        state
            .library_api
            .update_settings(UpdateSettingsRequest {
                actor: actor.user_id,
                request_id,
                library_id: id,
                name: body.name,
                reader_download_enabled: body.reader_download_enabled,
            })
            .await,
        &ctx,
    )
}
async fn invite_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path(raw): Path<String>,
    ApiJson(body): ApiJson<InvitationBody>,
) -> Response {
    let id = match library_id(&raw) {
        Ok(v) => v,
        Err(e) => return problem(&e, &ctx),
    };
    let role = match role(&body.role) {
        Ok(v) => v,
        Err(e) => return problem(&e, &ctx),
    };
    mutation(
        state
            .library_api
            .invite_member(InviteLibraryMemberRequest {
                actor: actor.user_id,
                request_id,
                library_id: id,
                email: body.email,
                role,
            })
            .await,
        &ctx,
    )
}

async fn accept_invitation(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProblemContext>,
    AuthenticatedActor(actor): AuthenticatedActor,
    ApiJson(body): ApiJson<AcceptInvitationBody>,
) -> Response {
    match state
        .library_api
        .accept_invitation(AcceptLibraryInvitationRequest {
            actor: actor.user_id,
            token: SecretString::from(body.token),
        })
        .await
    {
        Ok(outcome) => {
            let response = match outcome {
                InvitationAcceptance::Accepted { library_id } => {
                    AcceptInvitationResponse::Accepted {
                        library_id: library_id.as_uuid().to_string(),
                    }
                }
                InvitationAcceptance::WrongAccount { email_hint } => {
                    AcceptInvitationResponse::WrongAccount { email_hint }
                }
                InvitationAcceptance::Unverified => AcceptInvitationResponse::Unverified,
                InvitationAcceptance::Expired => AcceptInvitationResponse::Expired,
                InvitationAcceptance::Consumed => AcceptInvitationResponse::Consumed,
                InvitationAcceptance::Invalid => AcceptInvitationResponse::Invalid,
            };
            Json(response).into_response()
        }
        Err(error) => problem(&error, &ctx),
    }
}
async fn change_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path((library, user)): Path<(String, String)>,
    ApiJson(body): ApiJson<RoleBody>,
) -> Response {
    let library_id = match library_id(&library) {
        Ok(v) => v,
        Err(e) => return problem(&e, &ctx),
    };
    let user_id = match user_id(&user) {
        Ok(v) => v,
        Err(e) => return problem(&e, &ctx),
    };
    let role = match role(&body.role) {
        Ok(v) => v,
        Err(e) => return problem(&e, &ctx),
    };
    mutation(
        state
            .library_api
            .change_member(ChangeLibraryMemberRequest {
                actor: actor.user_id,
                request_id,
                library_id,
                user_id,
                role,
            })
            .await,
        &ctx,
    )
}
async fn remove_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProblemContext>,
    Extension(request_id): Extension<RequestId>,
    AuthenticatedActor(actor): AuthenticatedActor,
    Path((library, user)): Path<(String, String)>,
) -> Response {
    let library_id = match library_id(&library) {
        Ok(v) => v,
        Err(e) => return problem(&e, &ctx),
    };
    let user_id = match user_id(&user) {
        Ok(v) => v,
        Err(e) => return problem(&e, &ctx),
    };
    mutation(
        state
            .library_api
            .remove_member(RemoveLibraryMemberRequest {
                actor: actor.user_id,
                request_id,
                library_id,
                user_id,
            })
            .await,
        &ctx,
    )
}
fn mutation(result: Result<(), AppError>, ctx: &ProblemContext) -> Response {
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => problem(&e, ctx),
    }
}
