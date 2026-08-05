use std::{collections::BTreeMap, fmt, path::PathBuf};

use serde::Deserialize;

use super::{ConfigError, ConfigSources, DedupScope};

const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RawSettings {
    pub(super) server: RawServer,
    pub(super) database: RawDatabase,
    pub(super) storage: RawStorage,
    pub(super) auth: RawAuth,
    pub(super) mail: RawMail,
    pub(super) worker: RawWorker,
    pub(super) observability: RawObservability,
}

impl RawSettings {
    pub(super) fn parse(sources: &ConfigSources) -> Result<Self, ConfigError> {
        let mut raw = match sources.toml.as_deref() {
            Some(document) => parse_toml(document)?,
            None => Self::default(),
        };
        apply_environment(&mut raw, &sources.environment)?;
        apply_values(&mut raw, &sources.cli)?;
        Ok(raw)
    }
}

fn parse_toml(document: &str) -> Result<RawSettings, ConfigError> {
    let deserializer = toml::Deserializer::parse(document)
        .map_err(|_| ConfigError::invalid("toml", "document syntax is invalid"))?;
    serde_path_to_error::deserialize(deserializer).map_err(|error| {
        let path = error.path().to_string();
        let key = if path.is_empty() || path == "." {
            "toml".to_owned()
        } else {
            path
        };
        ConfigError::invalid(key, "value has an invalid type")
    })
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RawServer {
    pub(super) bind_address: String,
    pub(super) public_base_url: String,
}

impl Default for RawServer {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:8080".to_owned(),
            public_base_url: "http://localhost:8080/".to_owned(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RawDatabase {
    pub(super) url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RawStorage {
    pub(super) root: PathBuf,
    pub(super) staging_root: PathBuf,
    pub(super) library_quota_bytes: u64,
    pub(super) upload_limit_bytes: u64,
    pub(super) free_reserve_bytes: u64,
    pub(super) dedup_scope: DedupScope,
    pub(super) failed_retention_seconds: u64,
    pub(super) gc_delay_seconds: u64,
    pub(super) recovery_period_seconds: u64,
}

impl Default for RawStorage {
    fn default() -> Self {
        Self {
            root: PathBuf::from("/var/lib/folioharbor/blobs"),
            staging_root: PathBuf::from("/var/lib/folioharbor/staging"),
            library_quota_bytes: 5 * GIB,
            upload_limit_bytes: GIB,
            free_reserve_bytes: GIB,
            dedup_scope: DedupScope::Instance,
            failed_retention_seconds: 24 * 60 * 60,
            gc_delay_seconds: 24 * 60 * 60,
            recovery_period_seconds: 7 * 24 * 60 * 60,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
// These mirror independent deployment feature flags during parsing.
#[allow(clippy::struct_excessive_bools)]
pub(super) struct RawAuth {
    pub(super) registration_enabled: bool,
    pub(super) email_verification_enabled: bool,
    pub(super) personal_library_enabled: bool,
    pub(super) reader_download_enabled: bool,
    pub(super) invitation_enabled: bool,
    pub(super) password_reset_enabled: bool,
}

impl RawAuth {
    pub(super) const fn any_mail_flow_enabled(&self) -> bool {
        self.email_verification_enabled || self.invitation_enabled || self.password_reset_enabled
    }
}

impl Default for RawAuth {
    fn default() -> Self {
        Self {
            registration_enabled: true,
            email_verification_enabled: true,
            personal_library_enabled: true,
            reader_download_enabled: false,
            invitation_enabled: true,
            password_reset_enabled: true,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RawMail {
    pub(super) smtp_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RawWorker {
    pub(super) concurrency: usize,
}

impl Default for RawWorker {
    fn default() -> Self {
        Self { concurrency: 1 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RawObservability {
    pub(super) log_filter: String,
}

impl Default for RawObservability {
    fn default() -> Self {
        Self {
            log_filter: "info".to_owned(),
        }
    }
}

fn apply_environment(
    raw: &mut RawSettings,
    environment: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    let values = environment
        .iter()
        .filter_map(|(key, value)| {
            environment_path(key).map(|path| (path.to_owned(), value.clone()))
        })
        .collect();
    apply_values(raw, &values)
}

fn environment_path(key: &str) -> Option<&'static str> {
    match key {
        "FOLIOHARBOR_SERVER_BIND_ADDRESS" => Some("server.bind_address"),
        "FOLIOHARBOR_SERVER_PUBLIC_BASE_URL" => Some("server.public_base_url"),
        "FOLIOHARBOR_STORAGE_ROOT" => Some("storage.root"),
        "FOLIOHARBOR_STORAGE_STAGING_ROOT" => Some("storage.staging_root"),
        "FOLIOHARBOR_STORAGE_LIBRARY_QUOTA_BYTES" => Some("storage.library_quota_bytes"),
        "FOLIOHARBOR_STORAGE_UPLOAD_LIMIT_BYTES" => Some("storage.upload_limit_bytes"),
        "FOLIOHARBOR_STORAGE_FREE_RESERVE_BYTES" => Some("storage.free_reserve_bytes"),
        "FOLIOHARBOR_STORAGE_DEDUP_SCOPE" => Some("storage.dedup_scope"),
        "FOLIOHARBOR_STORAGE_FAILED_RETENTION_SECONDS" => Some("storage.failed_retention_seconds"),
        "FOLIOHARBOR_STORAGE_GC_DELAY_SECONDS" => Some("storage.gc_delay_seconds"),
        "FOLIOHARBOR_STORAGE_RECOVERY_PERIOD_SECONDS" => Some("storage.recovery_period_seconds"),
        "FOLIOHARBOR_AUTH_REGISTRATION_ENABLED" => Some("auth.registration_enabled"),
        "FOLIOHARBOR_AUTH_EMAIL_VERIFICATION_ENABLED" => Some("auth.email_verification_enabled"),
        "FOLIOHARBOR_AUTH_PERSONAL_LIBRARY_ENABLED" => Some("auth.personal_library_enabled"),
        "FOLIOHARBOR_AUTH_READER_DOWNLOAD_ENABLED" => Some("auth.reader_download_enabled"),
        "FOLIOHARBOR_AUTH_INVITATION_ENABLED" => Some("auth.invitation_enabled"),
        "FOLIOHARBOR_AUTH_PASSWORD_RESET_ENABLED" => Some("auth.password_reset_enabled"),
        "FOLIOHARBOR_MAIL_SMTP_URL" => Some("mail.smtp_url"),
        "FOLIOHARBOR_WORKER_CONCURRENCY" => Some("worker.concurrency"),
        "FOLIOHARBOR_OBSERVABILITY_LOG_FILTER" => Some("observability.log_filter"),
        _ => None,
    }
}

fn apply_values(
    raw: &mut RawSettings,
    values: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    for (key, value) in values {
        match key.as_str() {
            "server.bind_address" => raw.server.bind_address.clone_from(value),
            "server.public_base_url" => raw.server.public_base_url.clone_from(value),
            "storage.root" => raw.storage.root = PathBuf::from(value),
            "storage.staging_root" => raw.storage.staging_root = PathBuf::from(value),
            "storage.library_quota_bytes" => raw.storage.library_quota_bytes = parse(key, value)?,
            "storage.upload_limit_bytes" => raw.storage.upload_limit_bytes = parse(key, value)?,
            "storage.free_reserve_bytes" => raw.storage.free_reserve_bytes = parse(key, value)?,
            "storage.dedup_scope" => raw.storage.dedup_scope = parse_dedup_scope(key, value)?,
            "storage.failed_retention_seconds" => {
                raw.storage.failed_retention_seconds = parse(key, value)?;
            }
            "storage.gc_delay_seconds" => raw.storage.gc_delay_seconds = parse(key, value)?,
            "storage.recovery_period_seconds" => {
                raw.storage.recovery_period_seconds = parse(key, value)?;
            }
            "auth.registration_enabled" => raw.auth.registration_enabled = parse(key, value)?,
            "auth.email_verification_enabled" => {
                raw.auth.email_verification_enabled = parse(key, value)?;
            }
            "auth.personal_library_enabled" => {
                raw.auth.personal_library_enabled = parse(key, value)?;
            }
            "auth.reader_download_enabled" => {
                raw.auth.reader_download_enabled = parse(key, value)?;
            }
            "auth.invitation_enabled" => raw.auth.invitation_enabled = parse(key, value)?,
            "auth.password_reset_enabled" => {
                raw.auth.password_reset_enabled = parse(key, value)?;
            }
            "mail.smtp_url" => raw.mail.smtp_url = Some(value.clone()),
            "worker.concurrency" => raw.worker.concurrency = parse(key, value)?,
            "observability.log_filter" => raw.observability.log_filter.clone_from(value),
            _ => return Err(ConfigError::invalid(key, "unknown configuration key")),
        }
    }
    Ok(())
}

fn parse_dedup_scope(key: &str, value: &str) -> Result<DedupScope, ConfigError> {
    match value {
        "instance" => Ok(DedupScope::Instance),
        "library" => Ok(DedupScope::Library),
        "disabled" => Ok(DedupScope::Disabled),
        _ => Err(ConfigError::invalid(
            key,
            "must be instance, library, or disabled",
        )),
    }
}

fn parse<T>(key: &str, value: &str) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    value
        .parse()
        .map_err(|error: T::Err| ConfigError::invalid(key, error.to_string()))
}
