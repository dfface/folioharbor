use std::collections::{BTreeMap, HashSet};

use secrecy::SecretString;

use super::{ApplicationSecretKeyId, ApplicationSecretRing, ConfigError, types::ApplicationSecret};

const MINIMUM_SECRET_BYTES: usize = 32;

pub(super) fn load_secret_ring(
    environment: &BTreeMap<String, String>,
) -> Result<ApplicationSecretRing, ConfigError> {
    let key_id = required_environment(environment, "FOLIOHARBOR_AUTH_APPLICATION_SECRET_KEY_ID")?;
    let value = secret_environment(environment, "FOLIOHARBOR_AUTH_APPLICATION_SECRET")?
        .ok_or_else(|| {
            ConfigError::invalid(
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET",
                "is required and must come from the environment or a secret file",
            )
        })?;
    let current = parse_secret(
        key_id,
        &value,
        "auth.application_secret_key_id",
        "auth.application_secret",
    )?;
    let old = secret_environment(environment, "FOLIOHARBOR_AUTH_OLD_APPLICATION_SECRETS")?.map_or(
        Ok(Vec::new()),
        |encoded| {
            encoded
                .split(',')
                .map(|entry| {
                    let (old_key_id, old_value) = entry.split_once('=').ok_or_else(|| {
                        ConfigError::invalid(
                            "auth.old_application_secrets",
                            "entries must use key-id=secret format",
                        )
                    })?;
                    parse_secret(
                        old_key_id,
                        old_value,
                        "auth.old_application_secrets",
                        "auth.old_application_secrets",
                    )
                })
                .collect()
        },
    )?;
    let key_count = old.len() + 1;
    let unique_key_count = std::iter::once(&current)
        .chain(&old)
        .map(|secret| &secret.key_id)
        .collect::<HashSet<_>>()
        .len();
    if unique_key_count != key_count {
        return Err(ConfigError::invalid(
            "auth.old_application_secrets",
            "key IDs must be unique",
        ));
    }
    Ok(ApplicationSecretRing { current, old })
}

pub(super) fn secret_environment(
    environment: &BTreeMap<String, String>,
    key: &'static str,
) -> Result<Option<String>, ConfigError> {
    let file_key = format!("{key}_FILE");
    match (environment.get(key), environment.get(&file_key)) {
        (Some(_), Some(_)) => Err(ConfigError::invalid(
            key,
            "must not be supplied together with its _FILE variant",
        )),
        (Some(value), None) => Ok(Some(value.clone())),
        (None, Some(path)) => std::fs::read_to_string(path)
            .map(|value| Some(value.trim_end_matches(['\r', '\n']).to_owned()))
            .map_err(|_| ConfigError::invalid(file_key, "could not read secret file")),
        (None, None) => Ok(None),
    }
}

fn required_environment<'a>(
    environment: &'a BTreeMap<String, String>,
    key: &'static str,
) -> Result<&'a str, ConfigError> {
    environment
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ConfigError::invalid(key, "is required and must come from the environment"))
}

fn parse_secret(
    key_id: &str,
    value: &str,
    key_id_key: &'static str,
    secret_key: &'static str,
) -> Result<ApplicationSecret, ConfigError> {
    let key_id = ApplicationSecretKeyId::parse(key_id, key_id_key)?;
    if value.len() < MINIMUM_SECRET_BYTES || matches!(value, "change-me" | "default" | "secret") {
        return Err(ConfigError::invalid(
            secret_key,
            "must contain at least 32 bytes and must not be a default value",
        ));
    }
    Ok(ApplicationSecret {
        key_id,
        secret: SecretString::from(value.to_owned().into_boxed_str()),
    })
}
