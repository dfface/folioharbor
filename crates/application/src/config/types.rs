use std::path::PathBuf;

use secrecy::SecretString;
use serde::Deserialize;
use url::Url;

use folioharbor_domain::identity::NormalizedEmail;

use super::{ConfigError, raw::RawStorage};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteSize(u64);

impl ByteSize {
    fn new(key: &'static str, value: u64) -> Result<Self, ConfigError> {
        if value == 0 {
            return Err(ConfigError::invalid(key, "must be greater than zero"));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Duration(u64);

impl Duration {
    // This is the whole-second distance from the latest supported lifecycle
    // input (2100-01-01T00:00:00Z) to the application/SQLx timestamp ceiling
    // (9999-12-31T23:59:59Z). All database lifecycle paths add these durations
    // to a supplied timestamp and may return the resulting deadline.
    const MAX_LIFECYCLE_SECONDS: u64 = 249_299_855_999;

    fn new(key: &'static str, seconds: u64) -> Result<Self, ConfigError> {
        if seconds == 0 {
            return Err(ConfigError::invalid(key, "must be greater than zero"));
        }
        if seconds > Self::MAX_LIFECYCLE_SECONDS {
            return Err(ConfigError::invalid(
                key,
                "must keep lifecycle deadlines representable through 2100-01-01T00:00:00Z",
            ));
        }
        Ok(Self(seconds))
    }

    #[must_use]
    pub const fn as_seconds(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DedupScope {
    Instance,
    Library,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicUrl(Url);

impl PublicUrl {
    pub(super) fn parse(value: &str) -> Result<Self, ConfigError> {
        let url = Url::parse(value)
            .map_err(|error| ConfigError::invalid("server.public_base_url", error.to_string()))?;
        if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
            return Err(ConfigError::invalid(
                "server.public_base_url",
                "must be an absolute HTTP(S) base URL",
            ));
        }
        Ok(Self(url))
    }

    #[must_use]
    pub const fn as_url(&self) -> &Url {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SmtpUrl(Url);

impl SmtpUrl {
    pub(super) fn parse(value: &str) -> Result<Self, ConfigError> {
        let url = Url::parse(value)
            .map_err(|error| ConfigError::invalid("mail.smtp_url", error.to_string()))?;
        if !matches!(url.scheme(), "smtp" | "smtps") {
            return Err(ConfigError::invalid(
                "mail.smtp_url",
                "scheme must be smtp or smtps",
            ));
        }
        if url.cannot_be_a_base()
            || url.host_str().is_none()
            || !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ConfigError::invalid(
                "mail.smtp_url",
                "must contain a relay authority and no path, query, or fragment",
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ConfigError::invalid(
                "mail.smtp_url",
                "must not contain embedded credentials; use the dedicated mail credential settings",
            ));
        }
        Ok(Self(url))
    }

    #[must_use]
    pub const fn as_url(&self) -> &Url {
        &self.0
    }
}

#[derive(Debug)]
pub(super) struct ApplicationSecret {
    pub(super) key_id: ApplicationSecretKeyId,
    pub(super) secret: SecretString,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ApplicationSecretKeyId(String);

impl ApplicationSecretKeyId {
    pub(super) fn parse(value: &str, key: &'static str) -> Result<Self, ConfigError> {
        let mut characters = value.chars();
        let valid_first = characters
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric());
        let valid_rest = characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        });
        if !valid_first || !valid_rest {
            return Err(ConfigError::invalid(
                key,
                "must be a non-empty identifier using ASCII letters, digits, '.', '_', or '-'",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EncryptionSecret<'a>(&'a ApplicationSecret);

impl EncryptionSecret<'_> {
    #[must_use]
    pub const fn key_id(&self) -> &ApplicationSecretKeyId {
        &self.0.key_id
    }

    #[must_use]
    pub const fn secret(&self) -> &SecretString {
        &self.0.secret
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DecryptionSecret<'a>(&'a ApplicationSecret);

impl DecryptionSecret<'_> {
    #[must_use]
    pub const fn key_id(&self) -> &ApplicationSecretKeyId {
        &self.0.key_id
    }

    #[must_use]
    pub const fn secret(&self) -> &SecretString {
        &self.0.secret
    }
}

#[derive(Debug)]
pub struct ApplicationSecretRing {
    pub(super) current: ApplicationSecret,
    pub(super) old: Vec<ApplicationSecret>,
}

impl ApplicationSecretRing {
    #[must_use]
    pub const fn current_for_encryption(&self) -> EncryptionSecret<'_> {
        EncryptionSecret(&self.current)
    }

    #[must_use]
    pub fn find_for_decryption(&self, key_id: &str) -> Option<DecryptionSecret<'_>> {
        std::iter::once(&self.current)
            .chain(&self.old)
            .find(|secret| secret.key_id.as_str() == key_id)
            .map(DecryptionSecret)
    }
}

#[derive(Debug)]
pub struct Settings {
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub storage: StorageSettings,
    pub auth: AuthSettings,
    pub mail: MailSettings,
    pub worker: WorkerSettings,
    pub observability: ObservabilitySettings,
}

#[derive(Debug)]
pub struct ServerSettings {
    pub bind_address: String,
    pub public_base_url: PublicUrl,
}

#[derive(Debug)]
pub struct DatabaseSettings {
    pub url: Option<SecretString>,
}

#[derive(Debug)]
pub struct StorageSettings {
    pub root: PathBuf,
    pub library_quota: ByteSize,
    pub upload_limit: ByteSize,
    pub free_reserve: ByteSize,
    pub dedup_scope: DedupScope,
    pub failed_retention: Duration,
    pub gc_delay: Duration,
    pub recovery_period: Duration,
}

impl StorageSettings {
    pub(super) fn from_raw(raw: RawStorage) -> Result<Self, ConfigError> {
        for (key, value) in [
            ("storage.library_quota_bytes", raw.library_quota_bytes),
            ("storage.upload_limit_bytes", raw.upload_limit_bytes),
        ] {
            if value > i64::MAX as u64 {
                return Err(ConfigError::invalid(
                    key,
                    "must fit in a signed 64-bit integer",
                ));
            }
        }
        Ok(Self {
            root: raw.root,
            library_quota: ByteSize::new("storage.library_quota_bytes", raw.library_quota_bytes)?,
            upload_limit: ByteSize::new("storage.upload_limit_bytes", raw.upload_limit_bytes)?,
            free_reserve: ByteSize::new("storage.free_reserve_bytes", raw.free_reserve_bytes)?,
            dedup_scope: raw.dedup_scope,
            failed_retention: Duration::new(
                "storage.failed_retention_seconds",
                raw.failed_retention_seconds,
            )?,
            gc_delay: Duration::new("storage.gc_delay_seconds", raw.gc_delay_seconds)?,
            recovery_period: Duration::new(
                "storage.recovery_period_seconds",
                raw.recovery_period_seconds,
            )?,
        })
    }
}

#[derive(Debug)]
// These are independent deployment feature flags, not mutually exclusive states.
#[allow(clippy::struct_excessive_bools)]
pub struct AuthSettings {
    pub registration_enabled: bool,
    pub email_verification_enabled: bool,
    pub personal_library_enabled: bool,
    pub reader_download_enabled: bool,
    pub invitation_enabled: bool,
    pub password_reset_enabled: bool,
    pub application_secrets: ApplicationSecretRing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthFeatures {
    enabled: u8,
}

impl AuthFeatures {
    const REGISTRATION: u8 = 1 << 0;
    const EMAIL_VERIFICATION: u8 = 1 << 1;
    const INVITATION: u8 = 1 << 2;
    const PASSWORD_RESET: u8 = 1 << 3;

    #[must_use]
    pub const fn new(
        [registration, email_verification, invitation, password_reset]: [bool; 4],
    ) -> Self {
        let mut enabled = 0;
        if registration {
            enabled |= Self::REGISTRATION;
        }
        if email_verification {
            enabled |= Self::EMAIL_VERIFICATION;
        }
        if invitation {
            enabled |= Self::INVITATION;
        }
        if password_reset {
            enabled |= Self::PASSWORD_RESET;
        }
        Self { enabled }
    }

    #[must_use]
    pub const fn registration_enabled(self) -> bool {
        self.enabled & Self::REGISTRATION != 0
    }

    #[must_use]
    pub const fn email_verification_enabled(self) -> bool {
        self.enabled & Self::EMAIL_VERIFICATION != 0
    }

    #[must_use]
    pub const fn invitation_enabled(self) -> bool {
        self.enabled & Self::INVITATION != 0
    }

    #[must_use]
    pub const fn password_reset_enabled(self) -> bool {
        self.enabled & Self::PASSWORD_RESET != 0
    }
}

impl AuthSettings {
    #[must_use]
    pub const fn features(&self) -> AuthFeatures {
        AuthFeatures::new([
            self.registration_enabled,
            self.email_verification_enabled,
            self.invitation_enabled,
            self.password_reset_enabled,
        ])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailMode {
    Disabled,
    Enabled,
}

impl MailMode {
    pub(super) const fn from_flags(
        email_verification: bool,
        invitation: bool,
        password_reset: bool,
    ) -> Self {
        if email_verification || invitation || password_reset {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }

    #[must_use]
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Debug)]
pub struct MailSettings {
    pub mode: MailMode,
    pub smtp_url: Option<SmtpUrl>,
    pub from_address: NormalizedEmail,
    pub username: Option<SecretString>,
    pub password: Option<SecretString>,
}

impl MailSettings {
    /// Readiness is a pure validated-configuration property; it never probes SMTP.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        !self.mode.is_enabled() || self.smtp_url.is_some()
    }
}

#[derive(Debug)]
pub struct WorkerSettings {
    pub concurrency: usize,
}

#[derive(Debug)]
pub struct ObservabilitySettings {
    pub log_filter: String,
    pub otlp_endpoint: Option<Url>,
}
