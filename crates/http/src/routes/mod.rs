mod auth;
use crate::middleware;
use axum::{Router, middleware as axum_middleware};
use folioharbor_application::{
    identity::{
        AuthenticateSessionUseCase, CompletePasswordResetUseCase, CurrentSessionUseCase,
        ListSessionsUseCase, LoginUseCase, LogoutUseCase, RegisterAccountUseCase,
        RequestPasswordResetUseCase, RevokeSessionUseCase, VerifyEmailUseCase,
    },
    rate_limit::RateLimitUseCase,
};
use std::sync::Arc;
use url::Url;

#[derive(Clone)]
pub struct AppState {
    pub public_base_url: Url,
    pub register: Arc<dyn RegisterAccountUseCase>,
    pub verify: Arc<dyn VerifyEmailUseCase>,
    pub login: Arc<dyn LoginUseCase>,
    pub logout: Arc<dyn LogoutUseCase>,
    pub request_password_reset: Arc<dyn RequestPasswordResetUseCase>,
    pub complete_password_reset: Arc<dyn CompletePasswordResetUseCase>,
    pub authenticate_session: Arc<dyn AuthenticateSessionUseCase>,
    pub current_session: Arc<dyn CurrentSessionUseCase>,
    pub list_sessions: Arc<dyn ListSessionsUseCase>,
    pub revoke_session: Arc<dyn RevokeSessionUseCase>,
    pub rate_limit: Arc<dyn RateLimitUseCase>,
}
impl AppState {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        public_base_url: Url,
        register: Arc<dyn RegisterAccountUseCase>,
        verify: Arc<dyn VerifyEmailUseCase>,
        login: Arc<dyn LoginUseCase>,
        logout: Arc<dyn LogoutUseCase>,
        request_password_reset: Arc<dyn RequestPasswordResetUseCase>,
        complete_password_reset: Arc<dyn CompletePasswordResetUseCase>,
        authenticate_session: Arc<dyn AuthenticateSessionUseCase>,
        current_session: Arc<dyn CurrentSessionUseCase>,
        list_sessions: Arc<dyn ListSessionsUseCase>,
        revoke_session: Arc<dyn RevokeSessionUseCase>,
        rate_limit: Arc<dyn RateLimitUseCase>,
    ) -> Self {
        Self {
            public_base_url,
            register,
            verify,
            login,
            logout,
            request_password_reset,
            complete_password_reset,
            authenticate_session,
            current_session,
            list_sessions,
            revoke_session,
            rate_limit,
        }
    }
}
pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/api/v1/auth", auth::router())
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::csrf::authenticate_and_protect,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::request_id::attach,
        ))
        .with_state(state)
}
