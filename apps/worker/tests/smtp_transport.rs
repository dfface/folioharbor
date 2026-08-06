#![allow(clippy::expect_used)]

use std::collections::BTreeMap;

use folioharbor_application::config::{ConfigSources, Settings};
use folioharbor_worker::handlers::{SmtpMailer, SmtpSecurity};

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
