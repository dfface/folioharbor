#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;

use folioharbor_application::config::{ConfigError, ConfigSources, DedupScope, Settings};
use folioharbor_application::ports::{Clock, RandomSource};
use folioharbor_domain::time::OffsetDateTime;

const GIB: u64 = 1024 * 1024 * 1024;

fn minimum_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "FOLIOHARBOR_AUTH_APPLICATION_SECRET".to_owned(),
            "a-secret-value-with-at-least-32-bytes".to_owned(),
        ),
        (
            "FOLIOHARBOR_AUTH_APPLICATION_SECRET_KEY_ID".to_owned(),
            "primary-2026".to_owned(),
        ),
        (
            "FOLIOHARBOR_MAIL_SMTP_URL".to_owned(),
            "smtp://mail.example:2525".to_owned(),
        ),
    ])
}

#[test]
fn approved_defaults_are_stable() {
    let settings = Settings::load(ConfigSources {
        environment: minimum_environment(),
        ..ConfigSources::default()
    })
    .expect("valid minimum configuration");

    assert!(settings.auth.registration_enabled);
    assert!(settings.auth.email_verification_enabled);
    assert!(settings.auth.personal_library_enabled);
    assert!(!settings.auth.reader_download_enabled);
    let features = settings.auth.features();
    assert!(settings.mail.mode.is_enabled());
    assert!(features.registration_enabled());
    assert!(features.email_verification_enabled());
    assert!(features.invitation_enabled());
    assert!(features.password_reset_enabled());
    assert_eq!(settings.storage.library_quota.as_u64(), 5 * GIB);
    assert_eq!(settings.storage.upload_limit.as_u64(), GIB);
    assert_eq!(settings.storage.free_reserve.as_u64(), GIB);
    assert_eq!(settings.storage.dedup_scope, DedupScope::Instance);
    assert_eq!(settings.storage.failed_retention.as_seconds(), 24 * 60 * 60);
    assert_eq!(settings.storage.gc_delay.as_seconds(), 24 * 60 * 60);
    assert_eq!(
        settings.storage.recovery_period.as_seconds(),
        7 * 24 * 60 * 60
    );
}

#[test]
fn otlp_exporter_is_explicitly_configurable_and_defaults_to_disabled() {
    let defaults = Settings::load(ConfigSources {
        environment: minimum_environment(),
        ..ConfigSources::default()
    })
    .expect("defaults");
    assert!(defaults.observability.otlp_endpoint.is_none());

    let mut environment = minimum_environment();
    environment.insert(
        "FOLIOHARBOR_OBSERVABILITY_OTLP_ENDPOINT".to_owned(),
        "https://otel-collector.example:4317".to_owned(),
    );
    let configured = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect("OTLP configuration");
    assert_eq!(
        configured
            .observability
            .otlp_endpoint
            .as_ref()
            .expect("endpoint")
            .as_str(),
        "https://otel-collector.example:4317/"
    );
}

#[test]
fn environment_overrides_toml_and_cli_overrides_environment() {
    let mut environment = minimum_environment();
    environment.insert(
        "FOLIOHARBOR_STORAGE_DEDUP_SCOPE".to_owned(),
        "library".to_owned(),
    );
    let settings = Settings::load(ConfigSources {
        toml: Some("[storage]\ndedup_scope = \"disabled\"\n".to_owned()),
        environment,
        cli: BTreeMap::from([("storage.dedup_scope".to_owned(), "instance".to_owned())]),
    })
    .expect("valid layered configuration");

    assert_eq!(settings.storage.dedup_scope, DedupScope::Instance);
}

#[test]
fn typed_toml_errors_name_the_key_without_echoing_source_values() {
    let error = Settings::load(ConfigSources {
        toml: Some("[worker]\nconcurrency = \"sentinel-source-value\"\n".to_owned()),
        environment: minimum_environment(),
        ..ConfigSources::default()
    })
    .expect_err("invalid typed TOML value must fail");

    assert!(matches!(
        &error,
        ConfigError::Invalid { key, .. } if key == "worker.concurrency"
    ));
    assert!(!error.to_string().contains("sentinel-source-value"));
}

#[test]
fn debug_output_redacts_secret_values() {
    let sources = ConfigSources {
        environment: minimum_environment(),
        ..ConfigSources::default()
    };

    let debug = format!("{sources:?}");
    assert!(!debug.contains("a-secret-value-with-at-least-32-bytes"));
}

#[test]
fn application_secret_can_be_loaded_from_a_secret_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let secret_path = directory.path().join("application-secret");
    fs::write(&secret_path, "a-file-secret-value-with-at-least-32-bytes\n")
        .expect("write secret file");
    let mut environment = minimum_environment();
    environment.remove("FOLIOHARBOR_AUTH_APPLICATION_SECRET");
    environment.insert(
        "FOLIOHARBOR_AUTH_APPLICATION_SECRET_FILE".to_owned(),
        secret_path.to_string_lossy().into_owned(),
    );

    let settings = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect("valid file-injected secret");

    assert_eq!(
        settings
            .auth
            .application_secrets
            .current_for_encryption()
            .key_id()
            .as_str(),
        "primary-2026"
    );
}

#[test]
fn short_application_secrets_are_rejected_with_the_key_name() {
    let mut environment = minimum_environment();
    environment.insert(
        "FOLIOHARBOR_AUTH_APPLICATION_SECRET".to_owned(),
        "too-short".to_owned(),
    );

    let error = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect_err("short secret must fail");

    assert!(error.to_string().contains("auth.application_secret"));
}

#[test]
fn enabled_mail_flows_require_smtp() {
    let mut environment = minimum_environment();
    environment.remove("FOLIOHARBOR_MAIL_SMTP_URL");

    let error = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect_err("enabled mail flows without SMTP must fail");

    assert!(error.to_string().contains("mail.smtp_url"));
}

#[test]
fn disabled_mail_mode_accepts_an_absent_smtp_relay() {
    let mut environment = minimum_environment();
    environment.remove("FOLIOHARBOR_MAIL_SMTP_URL");
    for key in [
        "FOLIOHARBOR_AUTH_REGISTRATION_ENABLED",
        "FOLIOHARBOR_AUTH_EMAIL_VERIFICATION_ENABLED",
        "FOLIOHARBOR_AUTH_INVITATION_ENABLED",
        "FOLIOHARBOR_AUTH_PASSWORD_RESET_ENABLED",
    ] {
        environment.insert(key.to_owned(), "false".to_owned());
    }

    let settings = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect("mail-disabled configuration must not need an SMTP relay");

    assert!(!settings.mail.mode.is_enabled());
    assert!(settings.mail.smtp_url.is_none());
}

#[test]
fn every_mail_flag_combination_preserves_features_or_rejects_registration_without_verification() {
    for registration in [false, true] {
        for verification in [false, true] {
            for invitation in [false, true] {
                for password_reset in [false, true] {
                    let mut environment = minimum_environment();
                    for (key, enabled) in [
                        ("FOLIOHARBOR_AUTH_REGISTRATION_ENABLED", registration),
                        ("FOLIOHARBOR_AUTH_EMAIL_VERIFICATION_ENABLED", verification),
                        ("FOLIOHARBOR_AUTH_INVITATION_ENABLED", invitation),
                        ("FOLIOHARBOR_AUTH_PASSWORD_RESET_ENABLED", password_reset),
                    ] {
                        environment.insert(key.to_owned(), enabled.to_string());
                    }
                    if !(verification || invitation || password_reset) {
                        environment.remove("FOLIOHARBOR_MAIL_SMTP_URL");
                    }

                    let result = Settings::load(ConfigSources {
                        environment,
                        ..ConfigSources::default()
                    });
                    if registration && !verification {
                        let error = result.expect_err(
                            "registration without verification must be rejected explicitly",
                        );
                        assert!(
                            error
                                .to_string()
                                .contains("auth.email_verification_enabled")
                        );
                        continue;
                    }

                    let settings = result.expect("supported feature combination must load");
                    assert_eq!(settings.auth.registration_enabled, registration);
                    assert_eq!(settings.auth.email_verification_enabled, verification);
                    assert_eq!(settings.auth.invitation_enabled, invitation);
                    assert_eq!(settings.auth.password_reset_enabled, password_reset);
                    assert_eq!(
                        settings.mail.mode.is_enabled(),
                        verification || invitation || password_reset
                    );
                }
            }
        }
    }
}

#[test]
fn enabled_mail_mode_rejects_an_opaque_smtp_url_without_an_authority() {
    let mut environment = minimum_environment();
    environment.insert(
        "FOLIOHARBOR_MAIL_SMTP_URL".to_owned(),
        "smtp:opaque".to_owned(),
    );

    let error = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect_err("SMTP configuration without a relay authority must fail");

    assert!(error.to_string().contains("mail.smtp_url"));
}

#[test]
fn mail_readiness_is_configuration_only_and_ignores_transient_relay_outages() {
    let mut environment = minimum_environment();
    environment.insert(
        "FOLIOHARBOR_MAIL_SMTP_URL".to_owned(),
        "smtp://127.0.0.1:9".to_owned(),
    );

    let settings = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect("loading valid SMTP configuration must not contact the relay");

    assert!(settings.mail.mode.is_enabled());
    assert!(settings.mail.is_ready());
}

#[test]
fn smtp_urls_reject_embedded_credentials_without_echoing_them() {
    let mut environment = minimum_environment();
    environment.insert(
        "FOLIOHARBOR_MAIL_SMTP_URL".to_owned(),
        "smtp://mail-user:sentinel-credential@mail.example:2525".to_owned(),
    );

    let error = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect_err("SMTP userinfo must fail");
    let diagnostic = error.to_string();

    assert!(diagnostic.contains("mail.smtp_url"));
    assert!(!diagnostic.contains("sentinel-credential"));
}

#[test]
fn smtp_credentials_must_be_configured_as_a_pair_without_echoing_values() {
    for (present, missing) in [
        ("FOLIOHARBOR_MAIL_USERNAME", "FOLIOHARBOR_MAIL_PASSWORD"),
        ("FOLIOHARBOR_MAIL_PASSWORD", "FOLIOHARBOR_MAIL_USERNAME"),
    ] {
        let mut environment = minimum_environment();
        environment.insert(present.to_owned(), "sentinel-mail-credential".to_owned());

        let error = Settings::load(ConfigSources {
            environment,
            ..ConfigSources::default()
        })
        .expect_err("partial SMTP credentials must fail");
        let diagnostic = error.to_string();

        assert!(diagnostic.contains(missing));
        assert!(!diagnostic.contains("sentinel-mail-credential"));
    }
}

#[test]
fn smtp_from_address_is_validated_without_echoing_it() {
    let mut environment = minimum_environment();
    environment.insert(
        "FOLIOHARBOR_MAIL_FROM_ADDRESS".to_owned(),
        "sentinel-invalid-address".to_owned(),
    );

    let error = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect_err("invalid mail from address must fail");
    let diagnostic = error.to_string();

    assert!(diagnostic.contains("mail.from_address"));
    assert!(!diagnostic.contains("sentinel-invalid-address"));
}

#[test]
fn storage_paths_must_be_absolute_non_root_and_distinct() {
    for (key, value) in [
        ("storage.root", "relative"),
        ("storage.root", "/"),
        ("storage.staging_root", "/var/lib/folioharbor/blobs"),
    ] {
        let error = Settings::load(ConfigSources {
            environment: minimum_environment(),
            cli: BTreeMap::from([(key.to_owned(), value.to_owned())]),
            ..ConfigSources::default()
        })
        .expect_err("invalid storage path must fail");

        assert!(error.to_string().contains(key));
    }
}

#[test]
fn old_application_secrets_are_retained_for_decryption_only() {
    let mut environment = minimum_environment();
    environment.insert(
        "FOLIOHARBOR_AUTH_OLD_APPLICATION_SECRETS".to_owned(),
        "previous-2025=an-old-secret-value-with-at-least-32-bytes".to_owned(),
    );

    let settings = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect("valid rotated secret ring");

    assert_eq!(
        settings
            .auth
            .application_secrets
            .current_for_encryption()
            .key_id()
            .as_str(),
        "primary-2026"
    );
    assert!(
        settings
            .auth
            .application_secrets
            .find_for_decryption("previous-2025")
            .is_some()
    );
}

#[test]
fn malformed_application_secret_key_ids_are_rejected() {
    let mut environment = minimum_environment();
    environment.insert(
        "FOLIOHARBOR_AUTH_APPLICATION_SECRET_KEY_ID".to_owned(),
        " bad key ".to_owned(),
    );

    let error = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect_err("malformed key ID must fail");

    assert!(error.to_string().contains("application_secret_key_id"));
}

#[test]
fn duplicate_old_application_secret_key_ids_are_rejected() {
    let mut environment = minimum_environment();
    environment.insert(
        "FOLIOHARBOR_AUTH_OLD_APPLICATION_SECRETS".to_owned(),
        concat!(
            "previous=an-old-secret-value-with-at-least-32-bytes,",
            "previous=another-old-secret-value-with-at-least-32-bytes"
        )
        .to_owned(),
    );

    let error = Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect_err("duplicate key IDs must fail");

    assert!(error.to_string().contains("old_application_secrets"));
}

#[test]
fn deployment_example_is_valid_when_runtime_secrets_are_injected() {
    let settings = Settings::load(ConfigSources {
        toml: Some(include_str!("../../../deploy/example.folioharbor.toml").to_owned()),
        environment: minimum_environment(),
        ..ConfigSources::default()
    })
    .expect("valid example configuration");

    assert_eq!(settings.storage.dedup_scope, DedupScope::Instance);
}

#[test]
fn clock_and_random_ports_have_transport_neutral_contracts() {
    struct FixedClock;
    impl Clock for FixedClock {
        fn now(&self) -> OffsetDateTime {
            OffsetDateTime::UNIX_EPOCH
        }
    }

    struct FixedRandom;
    impl RandomSource for FixedRandom {
        fn fill(&self, destination: &mut [u8]) {
            destination.fill(7);
        }
    }

    let mut bytes = [0; 4];
    FixedRandom.fill(&mut bytes);
    assert_eq!(FixedClock.now(), OffsetDateTime::UNIX_EPOCH);
    assert_eq!(bytes, [7; 4]);
}
