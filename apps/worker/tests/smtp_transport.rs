#![allow(clippy::expect_used)]

use std::collections::BTreeMap;

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

#[test]
fn disabled_mail_mode_builds_no_smtp_transport_for_the_worker() {
    let transport = SmtpMailer::for_mode(&mail_disabled_settings().mail)
        .expect("disabled mode is valid worker configuration");

    assert!(transport.is_none());
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
