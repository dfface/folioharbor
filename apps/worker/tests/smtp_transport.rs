#![allow(clippy::expect_used)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use folioharbor_application::config::{ConfigSources, Settings};
use folioharbor_application::{mail::RenderedMail, ports::Mailer as _};
use folioharbor_domain::identity::NormalizedEmail;
use folioharbor_worker::handlers::{SmtpMailer, SmtpSecurity};
use tokio::{
    net::TcpListener,
    time::{Duration, timeout},
};

fn settings(smtp_url: &str) -> Settings {
    Settings::load(ConfigSources {
        environment: BTreeMap::from([
            (
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET_KEY_ID".to_owned(),
                "smtp-test".to_owned(),
            ),
            (
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET".to_owned(),
                "0123456789abcdef0123456789abcdef".to_owned(),
            ),
            ("FOLIOHARBOR_MAIL_SMTP_URL".to_owned(), smtp_url.to_owned()),
            (
                "FOLIOHARBOR_MAIL_FROM_ADDRESS".to_owned(),
                "noreply@example.com".to_owned(),
            ),
        ]),
        ..ConfigSources::default()
    })
    .expect("valid SMTP fixture settings")
}

fn mail_disabled_settings() -> Settings {
    Settings::load(ConfigSources {
        environment: BTreeMap::from([
            (
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET_KEY_ID".to_owned(),
                "smtp-test".to_owned(),
            ),
            (
                "FOLIOHARBOR_AUTH_APPLICATION_SECRET".to_owned(),
                "0123456789abcdef0123456789abcdef".to_owned(),
            ),
            (
                "FOLIOHARBOR_AUTH_REGISTRATION_ENABLED".to_owned(),
                "false".to_owned(),
            ),
            (
                "FOLIOHARBOR_AUTH_EMAIL_VERIFICATION_ENABLED".to_owned(),
                "false".to_owned(),
            ),
            (
                "FOLIOHARBOR_AUTH_INVITATION_ENABLED".to_owned(),
                "false".to_owned(),
            ),
            (
                "FOLIOHARBOR_AUTH_PASSWORD_RESET_ENABLED".to_owned(),
                "false".to_owned(),
            ),
        ]),
        ..ConfigSources::default()
    })
    .expect("valid mail-disabled settings")
}

fn credentialed_settings() -> Settings {
    let mut environment = BTreeMap::from([
        (
            "FOLIOHARBOR_AUTH_APPLICATION_SECRET_KEY_ID".to_owned(),
            "smtp-test".to_owned(),
        ),
        (
            "FOLIOHARBOR_AUTH_APPLICATION_SECRET".to_owned(),
            "0123456789abcdef0123456789abcdef".to_owned(),
        ),
        (
            "FOLIOHARBOR_MAIL_SMTP_URL".to_owned(),
            "smtp://mail.example:2525".to_owned(),
        ),
        (
            "FOLIOHARBOR_MAIL_FROM_ADDRESS".to_owned(),
            "noreply@example.com".to_owned(),
        ),
        (
            "FOLIOHARBOR_MAIL_USERNAME".to_owned(),
            "credential-user-sentinel".to_owned(),
        ),
    ]);
    environment.insert(
        "FOLIOHARBOR_MAIL_PASSWORD".to_owned(),
        "credential-password-sentinel".to_owned(),
    );
    Settings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })
    .expect("valid credentialed SMTP settings")
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CapturedWriter(Arc::clone(&self.0))
    }
}

impl std::io::Write for CapturedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn disabled_mail_mode_builds_no_smtp_transport_for_the_worker() {
    let transport = SmtpMailer::for_mode(&mail_disabled_settings().mail)
        .expect("disabled mode is valid worker configuration");

    assert!(transport.is_none());
}

#[tokio::test]
async fn smtp_configuration_and_failure_logs_exclude_credentials_tokens_and_full_links() {
    let capture = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(capture.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::info!("mail-redaction-capture-active");

    let mailer = SmtpMailer::new(&credentialed_settings().mail).expect("transport configures");
    let recipient = NormalizedEmail::parse("reader@example.com").expect("recipient");
    let token = "token-redaction-sentinel";
    let full_link = format!("https://library.example/reset-password?token={token}");
    let rendered = RenderedMail {
        subject: "Redaction".to_owned(),
        text: full_link.clone(),
        html: format!("<a href=\"{full_link}\">reset</a>"),
    };
    let failure = mailer
        .deliver(&recipient, "invalid-idempotency-key", &rendered)
        .await
        .expect_err("invalid key fails before network I/O");
    assert_eq!(failure.code(), "invalid_idempotency_key");

    let logs = String::from_utf8(
        capture
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    )
    .expect("captured logs are UTF-8");
    assert!(logs.contains("mail-redaction-capture-active"));
    for secret in [
        "credential-user-sentinel",
        "credential-password-sentinel",
        token,
        full_link.as_str(),
    ] {
        assert!(!logs.contains(secret), "logs leaked {secret}");
    }
}

#[tokio::test]
async fn smtp_scheme_requires_starttls_and_smtps_requires_implicit_tls() {
    let starttls = SmtpMailer::new(&settings("smtp://mail.example:2525").mail)
        .expect("STARTTLS transport configures");
    let implicit = SmtpMailer::new(&settings("smtps://mail.example:2465").mail)
        .expect("implicit TLS transport configures");

    assert_eq!(starttls.security(), SmtpSecurity::RequiredStartTls);
    assert_eq!(implicit.security(), SmtpSecurity::ImplicitTls);
    assert_eq!(starttls.port(), 2525);
    assert_eq!(implicit.port(), 2465);
    assert_eq!(starttls.timeout().as_secs(), 10);
}

#[tokio::test(start_paused = true)]
async fn a_server_that_accepts_but_never_greets_hits_the_complete_exchange_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("stalling listener");
    let port = listener.local_addr().expect("listener address").port();
    let server = tokio::spawn(async move {
        let (_connection, _) = listener.accept().await.expect("accepted connection");
        std::future::pending::<()>().await;
    });
    let mailer = SmtpMailer::new(&settings(&format!("smtp://127.0.0.1:{port}")).mail)
        .expect("stalling transport configures");
    let recipient = NormalizedEmail::parse("reader@example.com").expect("recipient");
    let rendered = RenderedMail {
        subject: "Deadline".to_owned(),
        text: "deadline body".to_owned(),
        html: "<p>deadline body</p>".to_owned(),
    };

    let result = timeout(
        Duration::from_secs(11),
        mailer.deliver(
            &recipient,
            "mail:verification:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            &rendered,
        ),
    )
    .await
    .expect("adapter must finish before the caller's guard")
    .expect_err("stalling SMTP exchange must fail");

    assert!(result.is_transient());
    assert_eq!(result.code(), "smtp_exchange_timeout");
    server.abort();
}
