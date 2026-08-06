mod auth;
mod catalog;
mod libraries;
mod progress;
mod reader;
mod uploads;
use crate::middleware;
use axum::{Router, middleware as axum_middleware};
use folioharbor_application::{
    catalog::{CatalogApi, UnavailableCatalogApi},
    identity::{
        AuthenticateSessionUseCase, CompletePasswordResetUseCase, CurrentSessionUseCase,
        ListSessionsUseCase, LoginUseCase, LogoutUseCase, RegisterAccountUseCase,
        RequestPasswordResetUseCase, RevokeSessionUseCase, VerifyEmailUseCase,
    },
    imports::{UnavailableUploadApi, UploadApi},
    libraries::{LibraryApi, UnavailableLibraryApi},
    rate_limit::RateLimitUseCase,
    reader::{ProgressApi, ReaderApi, UnavailableProgressApi, UnavailableReaderApi},
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
    pub library_api: Arc<dyn LibraryApi>,
    pub upload_api: Arc<dyn UploadApi>,
    pub catalog_api: Arc<dyn CatalogApi>,
    pub reader_api: Arc<dyn ReaderApi>,
    pub progress_api: Arc<dyn ProgressApi>,
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
            library_api: Arc::new(UnavailableLibraryApi),
            upload_api: Arc::new(UnavailableUploadApi),
            catalog_api: Arc::new(UnavailableCatalogApi),
            reader_api: Arc::new(UnavailableReaderApi),
            progress_api: Arc::new(UnavailableProgressApi),
        }
    }

    #[must_use]
    pub fn with_library_api(mut self, library_api: Arc<dyn LibraryApi>) -> Self {
        self.library_api = library_api;
        self
    }

    #[must_use]
    pub fn with_upload_api(mut self, upload_api: Arc<dyn UploadApi>) -> Self {
        self.upload_api = upload_api;
        self
    }

    #[must_use]
    pub fn with_catalog_api(mut self, catalog_api: Arc<dyn CatalogApi>) -> Self {
        self.catalog_api = catalog_api;
        self
    }

    #[must_use]
    pub fn with_reader_api(mut self, reader_api: Arc<dyn ReaderApi>) -> Self {
        self.reader_api = reader_api;
        self
    }

    #[must_use]
    pub fn with_progress_api(mut self, progress_api: Arc<dyn ProgressApi>) -> Self {
        self.progress_api = progress_api;
        self
    }
}
pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/api/v1/auth", auth::router())
        .nest(
            "/api/v1/libraries",
            libraries::router()
                .merge(uploads::router())
                .merge(catalog::router()),
        )
        .nest("/api/v1/items", reader::router())
        .nest("/api/v1/manifestations", progress::router())
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
