#![allow(clippy::expect_used)]

use async_trait::async_trait;
use folioharbor_application::config::{ConfigSources, Settings};
use folioharbor_application::mail::{
    DeliverMailJob, DeliveryFailure, Locale, MailMessage, MailOutbox, MailTemplate, render_message,
};
use folioharbor_application::ports::{
    LeaseMail, LeasedMail, MailError, MailRepository, MailRepositoryError, Mailer,
    NewMailOutboxEntry,
};
use folioharbor_domain::identity::NormalizedEmail;
use folioharbor_domain::time::OffsetDateTime;
use secrecy::SecretString;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use time::Duration;
use url::Url;
use uuid::Uuid;

fn message(template: MailTemplate) -> MailMessage {
    localized_message(template, Locale::En)
}

fn localized_message(template: MailTemplate, locale: Locale) -> MailMessage {
    MailMessage::new(
        None,
        NormalizedEmail::parse("reader@example.com").expect("valid recipient"),
        template,
        locale,
        SecretString::from("single-use-token"),
    )
}

#[test]
fn verification_text_and_html_contain_the_same_single_use_link() {
    let settings = delivery_settings();
    let rendered = render_message(
        &message(MailTemplate::Verification),
        &Url::parse("https://library.example/").expect("valid public URL"),
        &settings.server.public_base_url,
    )
    .expect("configured base URL renders a message");

    let link = "https://library.example/verify-email?token=single-use-token";
    assert!(rendered.text.contains(link));
    assert!(rendered.html.contains(link));
    assert!(rendered.text.contains("Verify your email"));
    assert!(rendered.html.contains("Verify your email"));
}

#[test]
fn localized_invitation_escapes_untrusted_context_without_remote_content() {
    let settings = delivery_settings();
    let mut invitation = localized_message(MailTemplate::Invitation, Locale::ZhCn);
    invitation.set_invitation_context("<Library & friends>", "editor");
    let rendered = render_message(
        &invitation,
        &Url::parse("https://library.example/").expect("valid public URL"),
        &settings.server.public_base_url,
    )
    .expect("configured base URL renders a message");

    assert!(rendered.html.contains("&lt;Library &amp; friends&gt;"));
    assert!(!rendered.html.contains("<Library & friends>"));
    assert!(!rendered.html.contains("<img"));
    assert!(!rendered.html.contains("src="));
    assert!(rendered.text.contains("图书馆：<Library & friends>"));
    assert!(rendered.text.contains("角色：editor"));
    assert!(rendered.html.contains("图书馆："));
    assert!(rendered.html.contains("角色："));
    assert!(!rendered.text.contains("Library:"));
    assert!(!rendered.html.contains("Role:"));
}

#[test]
fn rendering_rejects_a_base_url_with_a_non_http_scheme() {
    let message = message(MailTemplate::PasswordReset);
    let settings = delivery_settings();

    let base = Url::parse("mailto:reader@example.com").expect("parseable URL");
    assert!(render_message(&message, &base, &settings.server.public_base_url).is_err());
}

#[test]
fn rendering_rejects_host_or_scheme_that_differs_from_validated_configuration() {
    let message = message(MailTemplate::PasswordReset);
    let settings = delivery_settings();

    for base in [
        Url::parse("https://attacker.example/").expect("valid mismatch"),
        Url::parse("http://library.example/").expect("valid mismatch"),
    ] {
        assert!(
            render_message(&message, &base, &settings.server.public_base_url).is_err(),
            "mismatched origin must fail"
        );
    }
}

#[test]
fn smtp_4xx_is_retried_and_5xx_is_terminal() {
    assert!(DeliveryFailure::smtp_status(451).is_transient());
    assert!(!DeliveryFailure::smtp_status(550).is_transient());
}

#[test]
fn retries_keep_one_idempotency_key_without_exposing_the_token() {
    let first = message(MailTemplate::PasswordReset);
    let second = message(MailTemplate::PasswordReset);

    assert_eq!(first.idempotency_key(), first.idempotency_key());
    assert_ne!(first.idempotency_key(), second.idempotency_key());
    assert!(!first.idempotency_key().contains("single-use-token"));
}

struct DeliveryRepository {
    leases: Mutex<Vec<Vec<LeasedMail>>>,
    retries: Mutex<Vec<(Uuid, String)>>,
    sent: Mutex<Vec<Uuid>>,
}

#[async_trait]
impl MailRepository for DeliveryRepository {
    async fn enqueue(&self, entry: NewMailOutboxEntry) -> Result<Uuid, MailRepositoryError> {
        Ok(entry.mail_id)
    }

    async fn lease(&self, _: LeaseMail) -> Result<Vec<LeasedMail>, MailRepositoryError> {
        Ok(self
            .leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
            .unwrap_or_default())
    }

    async fn mark_sent(
        &self,
        mail_id: Uuid,
        _: &str,
        _: OffsetDateTime,
    ) -> Result<bool, MailRepositoryError> {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(mail_id);
        Ok(true)
    }

    async fn retry(
        &self,
        mail_id: Uuid,
        _: &str,
        _: OffsetDateTime,
        _: OffsetDateTime,
        code: &str,
    ) -> Result<bool, MailRepositoryError> {
        self.retries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((mail_id, code.to_owned()));
        Ok(true)
    }
}

struct TransientThenSuccess {
    keys: Mutex<Vec<String>>,
}

#[async_trait]
impl Mailer for TransientThenSuccess {
    async fn deliver(
        &self,
        _: &NormalizedEmail,
        idempotency_key: &str,
        _: &folioharbor_application::mail::RenderedMail,
    ) -> Result<(), MailError> {
        let mut keys = self
            .keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        keys.push(idempotency_key.to_owned());
        if keys.len() == 1 {
            Err(MailError::transient("smtp_451"))
        } else {
            Ok(())
        }
    }
}

fn delivery_settings() -> Settings {
    Settings::load(ConfigSources {
        toml: Some(include_str!("../../../deploy/example.folioharbor.toml").to_owned()),
        environment: BTreeMap::from([
            (
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET_KEY_ID".to_owned(),
                "mail-test".to_owned(),
            ),
            (
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET".to_owned(),
                "0123456789abcdef0123456789abcdef".to_owned(),
            ),
        ]),
        ..ConfigSources::default()
    })
    .expect("delivery fixture configuration is valid")
}

#[tokio::test]
async fn transient_retry_reuses_idempotency_key_then_marks_terminal_success() {
    let settings = delivery_settings();
    let public_base_url = settings.server.public_base_url.clone();
    let secrets = Arc::new(settings.auth.application_secrets);
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("valid fixture time");
    let entry = MailOutbox::new(Arc::clone(&secrets))
        .enqueue(
            message(MailTemplate::PasswordReset),
            now,
            now + Duration::hours(1),
        )
        .expect("fixture intent encrypts");
    let leased = LeasedMail {
        mail_id: entry.mail_id,
        recipient_account_id: entry.recipient_account_id,
        delivery_address: entry.delivery_address,
        template_code: entry.template_code.to_owned(),
        template_version: entry.template_version,
        locale: entry.locale.to_owned(),
        token_ciphertext: entry.token_ciphertext,
        encryption_key_id: entry.encryption_key_id,
        nonce: entry.nonce,
        idempotency_key: entry.idempotency_key,
        invitation_library_id: entry.invitation_library_id,
        invitation_role: entry.invitation_role,
        attempt: 1,
        expires_at: entry.expires_at,
        lease_expires_at: now + Duration::minutes(5),
    };
    let second = LeasedMail {
        attempt: 2,
        ..leased.clone()
    };
    let repository = DeliveryRepository {
        leases: Mutex::new(vec![vec![second], vec![leased]]),
        retries: Mutex::new(Vec::new()),
        sent: Mutex::new(Vec::new()),
    };
    let mailer = TransientThenSuccess {
        keys: Mutex::new(Vec::new()),
    };
    let delivery = DeliverMailJob::new(
        &repository,
        &mailer,
        secrets,
        public_base_url,
        "worker-1".to_owned(),
    );

    assert_eq!(delivery.run_once(now, 1).await.expect("retry handled"), 1);
    assert_eq!(
        delivery
            .run_once(now + Duration::minutes(1), 1)
            .await
            .expect("success handled"),
        1
    );
    let keys = mailer
        .keys
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], keys[1]);
    assert!(!keys[0].contains("single-use-token"));
    assert_eq!(
        repository
            .retries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1
    );
    assert_eq!(
        repository
            .sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1
    );
}
