use folioharbor_domain::{
    id::UserId,
    identity::{EmailVerificationToken, NormalizedEmail},
};
use secrecy::SecretString;

use crate::{
    error::{AppError, FieldViolation},
    mail::{Locale, MailIntentSealer, MailMessage, MailTemplate},
    ports::{Clock, IdentityRepository, NewAccount, PasswordHasher, RandomSource},
};

use super::{VERIFICATION_LIFETIME, internal_error};

pub struct RegisterAccountCommand {
    pub email: String,
    pub password: SecretString,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingAccount;

pub struct RegisterAccount<'a, R, H, M, C, N> {
    repository: &'a R,
    password_hasher: &'a H,
    mailer: &'a M,
    clock: &'a C,
    random: &'a N,
}

impl<'a, R, H, M, C, N> RegisterAccount<'a, R, H, M, C, N> {
    #[must_use]
    pub const fn new(
        repository: &'a R,
        password_hasher: &'a H,
        mailer: &'a M,
        clock: &'a C,
        random: &'a N,
    ) -> Self {
        Self {
            repository,
            password_hasher,
            mailer,
            clock,
            random,
        }
    }
}

impl<R: IdentityRepository, H: PasswordHasher, M: MailIntentSealer, C: Clock, N: RandomSource>
    RegisterAccount<'_, R, H, M, C, N>
{
    /// Registers a local account without revealing whether its email already exists.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] for invalid input or a hashing or persistence dependency failure.
    pub async fn execute(
        &self,
        command: RegisterAccountCommand,
    ) -> Result<PendingAccount, AppError> {
        let email = NormalizedEmail::parse(&command.email).map_err(|_| AppError::Invalid {
            code: "invalid_registration",
            fields: vec![FieldViolation {
                field: "email",
                code: "invalid",
            }],
        })?;
        if command.password.expose_secret().is_empty() {
            return Err(AppError::Invalid {
                code: "invalid_registration",
                fields: vec![FieldViolation {
                    field: "password",
                    code: "required",
                }],
            });
        }
        let password_hash = self
            .password_hasher
            .hash(&command.password)
            .map_err(|_| internal_error())?;
        let mut bytes = [0_u8; 32];
        self.random.fill(&mut bytes);
        let token = EmailVerificationToken::from_random_bytes(bytes);
        let token_hash = token.hash_for_storage();
        let now = self.clock.now();
        let user_id = UserId::new();
        let mail = self
            .mailer
            .seal(
                MailMessage::new(
                    Some(user_id.as_uuid()),
                    email.clone(),
                    MailTemplate::Verification,
                    Locale::En,
                    token.into_secret(),
                ),
                now,
                now + VERIFICATION_LIFETIME,
            )
            .map_err(|_| internal_error())?;
        let _outcome = self
            .repository
            .register_with_verification(
                NewAccount {
                    user_id,
                    normalized_email: email.clone(),
                    display_email: command.email.trim().to_owned(),
                    password_hash,
                    verification_token_hash: token_hash,
                    created_at: now,
                    verification_expires_at: now + VERIFICATION_LIFETIME,
                },
                mail,
            )
            .await
            .map_err(|_| internal_error())?;
        Ok(PendingAccount)
    }
}

use secrecy::ExposeSecret;
