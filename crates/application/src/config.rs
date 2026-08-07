mod raw;
mod secret;
mod types;

use std::{collections::BTreeMap, fmt};

use secrecy::SecretString;
use thiserror::Error;

use folioharbor_domain::identity::NormalizedEmail;

use raw::RawSettings;
use secret::{load_secret_ring, secret_environment};
pub use types::{
    ApplicationSecretKeyId, ApplicationSecretRing, AuthFeatures, AuthSettings, ByteSize,
    DatabaseSettings, DecryptionSecret, DedupScope, Duration, EncryptionSecret, MailMode,
    MailSettings, ObservabilitySettings, PublicUrl, ServerSettings, Settings, SmtpUrl,
    StorageSettings, WorkerSettings,
};

#[derive(Clone, Default)]
pub struct ConfigSources {
    pub toml: Option<String>,
    pub environment: BTreeMap<String, String>,
    pub cli: BTreeMap<String, String>,
}

impl fmt::Debug for ConfigSources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigSources")
            .field("toml", &self.toml.as_ref().map(|_| "<redacted>"))
            .field("environment_keys", &self.environment.keys())
            .field("cli_keys", &self.cli.keys())
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid configuration key `{key}`: {reason}")]
    Invalid { key: String, reason: String },
}

impl ConfigError {
    pub(super) fn invalid(key: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Invalid {
            key: key.into(),
            reason: reason.into(),
        }
    }
}

impl Settings {
    /// Loads, layers, parses, and validates all configuration sources.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when TOML cannot be parsed, a layered value is
    /// invalid, a required secret is unavailable, or cross-field validation fails.
    // The fixed public contract intentionally consumes `ConfigSources`.
    #[allow(clippy::needless_pass_by_value)]
    pub fn load(sources: ConfigSources) -> Result<Self, ConfigError> {
        let raw = RawSettings::parse(&sources)?;
        Self::validate(raw, &sources.environment)
    }

    fn validate(
        raw: RawSettings,
        environment: &BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        validate_storage_paths(&raw)?;
        if raw.worker.concurrency == 0 {
            return Err(ConfigError::invalid(
                "worker.concurrency",
                "must be greater than zero",
            ));
        }
        if raw.auth.registration_enabled && !raw.auth.email_verification_enabled {
            return Err(ConfigError::invalid(
                "auth.email_verification_enabled",
                "must be enabled when registration is enabled",
            ));
        }
        let mail_mode = MailMode::from_flags(
            raw.auth.email_verification_enabled,
            raw.auth.invitation_enabled,
            raw.auth.password_reset_enabled,
        );
        let smtp_url = raw
            .mail
            .smtp_url
            .as_deref()
            .map(SmtpUrl::parse)
            .transpose()?;
        if smtp_url.is_none() && mail_mode.is_enabled() {
            return Err(ConfigError::invalid(
                "mail.smtp_url",
                "is required when email verification, invitations, or password reset is enabled",
            ));
        }
        let from_address = NormalizedEmail::parse(&raw.mail.from_address).map_err(|_| {
            ConfigError::invalid("mail.from_address", "must be a valid email address")
        })?;
        let username = environment
            .get("FOLIOHARBOR_MAIL_USERNAME")
            .cloned()
            .map(secret_string);
        let password =
            secret_environment(environment, "FOLIOHARBOR_MAIL_PASSWORD")?.map(secret_string);
        match (username.is_some(), password.is_some()) {
            (true, false) => {
                return Err(ConfigError::invalid(
                    "FOLIOHARBOR_MAIL_PASSWORD",
                    "is required when FOLIOHARBOR_MAIL_USERNAME is configured",
                ));
            }
            (false, true) => {
                return Err(ConfigError::invalid(
                    "FOLIOHARBOR_MAIL_USERNAME",
                    "is required when FOLIOHARBOR_MAIL_PASSWORD is configured",
                ));
            }
            (true, true) | (false, false) => {}
        }
        let application_secrets = load_secret_ring(environment)?;
        Ok(Self {
            server: ServerSettings {
                bind_address: raw.server.bind_address,
                public_base_url: PublicUrl::parse(&raw.server.public_base_url)?,
            },
            database: DatabaseSettings {
                url: secret_environment(environment, "FOLIOHARBOR_DATABASE_URL")?
                    .or(raw.database.url)
                    .map(secret_string),
            },
            storage: StorageSettings::from_raw(raw.storage)?,
            auth: AuthSettings {
                registration_enabled: raw.auth.registration_enabled,
                email_verification_enabled: raw.auth.email_verification_enabled,
                personal_library_enabled: raw.auth.personal_library_enabled,
                reader_download_enabled: raw.auth.reader_download_enabled,
                invitation_enabled: raw.auth.invitation_enabled,
                password_reset_enabled: raw.auth.password_reset_enabled,
                application_secrets,
            },
            mail: MailSettings {
                mode: mail_mode,
                smtp_url,
                from_address,
                username,
                password,
            },
            worker: WorkerSettings {
                concurrency: raw.worker.concurrency,
            },
            observability: ObservabilitySettings {
                log_filter: raw.observability.log_filter,
                otlp_endpoint: raw
                    .observability
                    .otlp_endpoint
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .map(parse_otlp_endpoint)
                    .transpose()?,
            },
        })
    }
}

fn parse_otlp_endpoint(value: &str) -> Result<url::Url, ConfigError> {
    let endpoint = url::Url::parse(value)
        .map_err(|_| ConfigError::invalid("observability.otlp_endpoint", "must be a URL"))?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        return Err(ConfigError::invalid(
            "observability.otlp_endpoint",
            "must be an HTTP(S) URL without embedded credentials",
        ));
    }
    Ok(endpoint)
}

fn secret_string(value: String) -> SecretString {
    SecretString::from(value.into_boxed_str())
}

fn validate_storage_paths(raw: &RawSettings) -> Result<(), ConfigError> {
    let storage = &raw.storage;
    for (key, path) in [
        ("storage.root", &storage.root),
        ("storage.staging_root", &storage.staging_root),
    ] {
        if !path.is_absolute() || path.parent().is_none() {
            return Err(ConfigError::invalid(
                key,
                "must be an absolute path below the filesystem root",
            ));
        }
    }
    if storage.root == storage.staging_root {
        return Err(ConfigError::invalid(
            "storage.staging_root",
            "must differ from storage.root",
        ));
    }
    Ok(())
}
