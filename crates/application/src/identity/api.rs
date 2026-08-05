use async_trait::async_trait;

use crate::{
    actor::Actor,
    error::AppError,
    ports::{Clock, IdentityRepository, LibraryRepository, Mailer, PasswordHasher, RandomSource},
};

use super::{
    AuthenticateSession, CompletePasswordReset, CurrentSession, ListSessions, Login, Logout,
    RegisterAccount, RequestPasswordReset, RevokeSession, VerifyEmail,
};
use super::{
    AuthenticateSessionCommand, AuthenticatedSession, CompletePasswordResetCommand, IssuedSession,
    LoginCommand, LogoutCommand, PasswordResetComplete, PasswordResetRequested, PendingAccount,
    RegisterAccountCommand, RequestPasswordResetCommand, RevokeSessionCommand,
    RevokeSessionOutcome, SafeSession, VerifiedAccount, VerifyEmailCommand,
};

#[async_trait]
pub trait RegisterAccountUseCase: Send + Sync {
    async fn register(&self, command: RegisterAccountCommand) -> Result<PendingAccount, AppError>;
}
#[async_trait]
pub trait VerifyEmailUseCase: Send + Sync {
    async fn verify_email(&self, command: VerifyEmailCommand) -> Result<VerifiedAccount, AppError>;
}
#[async_trait]
pub trait LoginUseCase: Send + Sync {
    async fn login(&self, command: LoginCommand) -> Result<IssuedSession, AppError>;
}
#[async_trait]
pub trait LogoutUseCase: Send + Sync {
    async fn logout(&self, command: LogoutCommand) -> Result<(), AppError>;
}
#[async_trait]
pub trait RequestPasswordResetUseCase: Send + Sync {
    async fn request_password_reset(
        &self,
        command: RequestPasswordResetCommand,
    ) -> Result<PasswordResetRequested, AppError>;
}
#[async_trait]
pub trait CompletePasswordResetUseCase: Send + Sync {
    async fn complete_password_reset(
        &self,
        command: CompletePasswordResetCommand,
    ) -> Result<PasswordResetComplete, AppError>;
}
#[async_trait]
pub trait AuthenticateSessionUseCase: Send + Sync {
    async fn authenticate_session(
        &self,
        command: AuthenticateSessionCommand,
    ) -> Result<Option<AuthenticatedSession>, AppError>;
}
#[async_trait]
pub trait CurrentSessionUseCase: Send + Sync {
    async fn current_session(&self, actor: Actor) -> Result<SafeSession, AppError>;
}
#[async_trait]
pub trait ListSessionsUseCase: Send + Sync {
    async fn list_sessions(&self, actor: Actor) -> Result<Vec<SafeSession>, AppError>;
}
#[async_trait]
pub trait RevokeSessionUseCase: Send + Sync {
    async fn revoke_session(
        &self,
        command: RevokeSessionCommand,
    ) -> Result<RevokeSessionOutcome, AppError>;
}

pub struct IdentityApi<R, H, M, C, N, L = ()> {
    repository: R,
    password_hasher: H,
    mailer: M,
    clock: C,
    random: N,
    library_repository: Option<L>,
}
impl<R, H, M, C, N> IdentityApi<R, H, M, C, N, ()> {
    #[must_use]
    pub const fn new(repository: R, password_hasher: H, mailer: M, clock: C, random: N) -> Self {
        Self {
            repository,
            password_hasher,
            mailer,
            clock,
            random,
            library_repository: None,
        }
    }
}
impl<R, H, M, C, N, L> IdentityApi<R, H, M, C, N, L> {
    #[must_use]
    pub const fn new_with_personal_library(
        repository: R,
        password_hasher: H,
        mailer: M,
        clock: C,
        random: N,
        library_repository: L,
    ) -> Self {
        Self {
            repository,
            password_hasher,
            mailer,
            clock,
            random,
            library_repository: Some(library_repository),
        }
    }
    #[must_use]
    pub fn new_configured(
        repository: R,
        password_hasher: H,
        mailer: M,
        clock: C,
        random: N,
        personal_library_enabled: bool,
        library_repository: L,
    ) -> Self {
        Self {
            repository,
            password_hasher,
            mailer,
            clock,
            random,
            library_repository: personal_library_enabled.then_some(library_repository),
        }
    }
}

#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource, L: Send + Sync>
    RegisterAccountUseCase for IdentityApi<R, H, M, C, N, L>
{
    async fn register(&self, command: RegisterAccountCommand) -> Result<PendingAccount, AppError> {
        RegisterAccount::new(
            &self.repository,
            &self.password_hasher,
            &self.mailer,
            &self.clock,
            &self.random,
        )
        .execute(command)
        .await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource, L: Send + Sync>
    VerifyEmailUseCase for IdentityApi<R, H, M, C, N, L>
{
    async fn verify_email(&self, command: VerifyEmailCommand) -> Result<VerifiedAccount, AppError> {
        VerifyEmail::new(&self.repository, &self.clock)
            .execute(command)
            .await
    }
}
#[async_trait]
impl<
    R: IdentityRepository,
    H: PasswordHasher,
    M: Mailer,
    C: Clock,
    N: RandomSource,
    L: LibraryRepository,
> LoginUseCase for IdentityApi<R, H, M, C, N, L>
{
    async fn login(&self, command: LoginCommand) -> Result<IssuedSession, AppError> {
        if let Some(library_repository) = &self.library_repository {
            Login::new_with_personal_library(
                &self.repository,
                &self.password_hasher,
                &self.clock,
                &self.random,
                library_repository,
            )
            .execute(command)
            .await
        } else {
            Login::new(
                &self.repository,
                &self.password_hasher,
                &self.clock,
                &self.random,
            )
            .execute(command)
            .await
        }
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource, L: Send + Sync>
    LogoutUseCase for IdentityApi<R, H, M, C, N, L>
{
    async fn logout(&self, command: LogoutCommand) -> Result<(), AppError> {
        Logout::new(&self.repository, &self.clock)
            .execute(command)
            .await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource, L: Send + Sync>
    RequestPasswordResetUseCase for IdentityApi<R, H, M, C, N, L>
{
    async fn request_password_reset(
        &self,
        command: RequestPasswordResetCommand,
    ) -> Result<PasswordResetRequested, AppError> {
        RequestPasswordReset::new(&self.repository, &self.mailer, &self.clock, &self.random)
            .execute(command)
            .await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource, L: Send + Sync>
    CompletePasswordResetUseCase for IdentityApi<R, H, M, C, N, L>
{
    async fn complete_password_reset(
        &self,
        command: CompletePasswordResetCommand,
    ) -> Result<PasswordResetComplete, AppError> {
        CompletePasswordReset::new(
            &self.repository,
            &self.password_hasher,
            &self.clock,
            &self.random,
        )
        .execute(command)
        .await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource, L: Send + Sync>
    AuthenticateSessionUseCase for IdentityApi<R, H, M, C, N, L>
{
    async fn authenticate_session(
        &self,
        command: AuthenticateSessionCommand,
    ) -> Result<Option<AuthenticatedSession>, AppError> {
        AuthenticateSession::new(&self.repository, &self.clock)
            .execute(command)
            .await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource, L: Send + Sync>
    CurrentSessionUseCase for IdentityApi<R, H, M, C, N, L>
{
    async fn current_session(&self, actor: Actor) -> Result<SafeSession, AppError> {
        CurrentSession::new(&self.repository, &self.clock)
            .execute(actor)
            .await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource, L: Send + Sync>
    ListSessionsUseCase for IdentityApi<R, H, M, C, N, L>
{
    async fn list_sessions(&self, actor: Actor) -> Result<Vec<SafeSession>, AppError> {
        ListSessions::new(&self.repository, &self.clock)
            .execute(actor)
            .await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource, L: Send + Sync>
    RevokeSessionUseCase for IdentityApi<R, H, M, C, N, L>
{
    async fn revoke_session(
        &self,
        command: RevokeSessionCommand,
    ) -> Result<RevokeSessionOutcome, AppError> {
        RevokeSession::new(&self.repository, &self.clock)
            .execute(command)
            .await
    }
}
