use async_trait::async_trait;

use crate::{
    actor::Actor,
    error::AppError,
    ports::{Clock, IdentityRepository, Mailer, PasswordHasher, RandomSource},
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

pub struct IdentityApi<R, H, M, C, N> {
    repository: R,
    password_hasher: H,
    mailer: M,
    clock: C,
    random: N,
}
impl<R, H, M, C, N> IdentityApi<R, H, M, C, N> {
    #[must_use]
    pub const fn new(repository: R, password_hasher: H, mailer: M, clock: C, random: N) -> Self {
        Self {
            repository,
            password_hasher,
            mailer,
            clock,
            random,
        }
    }
}

#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource>
    RegisterAccountUseCase for IdentityApi<R, H, M, C, N>
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
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource>
    VerifyEmailUseCase for IdentityApi<R, H, M, C, N>
{
    async fn verify_email(&self, command: VerifyEmailCommand) -> Result<VerifiedAccount, AppError> {
        VerifyEmail::new(&self.repository, &self.clock)
            .execute(command)
            .await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource> LoginUseCase
    for IdentityApi<R, H, M, C, N>
{
    async fn login(&self, command: LoginCommand) -> Result<IssuedSession, AppError> {
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
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource> LogoutUseCase
    for IdentityApi<R, H, M, C, N>
{
    async fn logout(&self, command: LogoutCommand) -> Result<(), AppError> {
        Logout::new(&self.repository, &self.clock)
            .execute(command)
            .await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource>
    RequestPasswordResetUseCase for IdentityApi<R, H, M, C, N>
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
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource>
    CompletePasswordResetUseCase for IdentityApi<R, H, M, C, N>
{
    async fn complete_password_reset(
        &self,
        command: CompletePasswordResetCommand,
    ) -> Result<PasswordResetComplete, AppError> {
        CompletePasswordReset::new(&self.repository, &self.password_hasher, &self.clock)
            .execute(command)
            .await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource>
    AuthenticateSessionUseCase for IdentityApi<R, H, M, C, N>
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
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource>
    CurrentSessionUseCase for IdentityApi<R, H, M, C, N>
{
    async fn current_session(&self, actor: Actor) -> Result<SafeSession, AppError> {
        CurrentSession::new(&self.repository).execute(actor).await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource>
    ListSessionsUseCase for IdentityApi<R, H, M, C, N>
{
    async fn list_sessions(&self, actor: Actor) -> Result<Vec<SafeSession>, AppError> {
        ListSessions::new(&self.repository).execute(actor).await
    }
}
#[async_trait]
impl<R: IdentityRepository, H: PasswordHasher, M: Mailer, C: Clock, N: RandomSource>
    RevokeSessionUseCase for IdentityApi<R, H, M, C, N>
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
