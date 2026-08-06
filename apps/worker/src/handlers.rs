use std::{io::Write as _, sync::Arc, time::Duration as StdDuration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use folioharbor_application::{
    config::MailSettings,
    imports::{CleanupImports, JobFailure, ProcessImportJob},
    mail::RenderedMail,
    ports::{MailError, Mailer},
};
use folioharbor_domain::identity::NormalizedEmail;
use folioharbor_domain::imports::job::{JobKind, LeasedJob};
use lettre::{
    Address, AsyncSmtpTransport, AsyncTransport as _, Tokio1Executor,
    address::Envelope,
    transport::smtp::{authentication::Credentials, response::Response},
};
use secrecy::ExposeSecret as _;
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use zeroize::Zeroize as _;

use crate::runner::JobDispatcher;

const SMTP_TIMEOUT: StdDuration = StdDuration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmtpSecurity {
    RequiredStartTls,
    ImplicitTls,
}

#[derive(Debug, Error)]
#[error("SMTP transport configuration is invalid")]
pub struct SmtpConfigurationError;

/// TLS-enforcing SMTP adapter. Message bodies and credentials are never part of its debug surface.
pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Address,
    security: SmtpSecurity,
    port: u16,
}

impl SmtpMailer {
    /// Builds an SMTP transport without contacting the relay.
    ///
    /// # Errors
    ///
    /// Returns a redacted error when the URL has no relay host or the sender is invalid.
    pub fn new(settings: &MailSettings) -> Result<Self, SmtpConfigurationError> {
        let url = settings
            .smtp_url
            .as_ref()
            .ok_or(SmtpConfigurationError)?
            .as_url();
        let relay = url.host_str().ok_or(SmtpConfigurationError)?;
        let (mut builder, security, port) = match url.scheme() {
            "smtp" => (
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(relay)
                    .map_err(|_| SmtpConfigurationError)?,
                SmtpSecurity::RequiredStartTls,
                url.port().unwrap_or(587),
            ),
            "smtps" => (
                AsyncSmtpTransport::<Tokio1Executor>::relay(relay)
                    .map_err(|_| SmtpConfigurationError)?,
                SmtpSecurity::ImplicitTls,
                url.port().unwrap_or(465),
            ),
            _ => return Err(SmtpConfigurationError),
        };
        builder = builder.port(port).timeout(Some(SMTP_TIMEOUT));
        if let (Some(username), Some(password)) = (&settings.username, &settings.password) {
            builder = builder.credentials(Credentials::new(
                username.expose_secret().to_owned(),
                password.expose_secret().to_owned(),
            ));
        }
        let from = settings
            .from_address
            .as_str()
            .parse()
            .map_err(|_| SmtpConfigurationError)?;
        Ok(Self {
            transport: builder.build(),
            from,
            security,
            port,
        })
    }

    #[must_use]
    pub const fn security(&self) -> SmtpSecurity {
        self.security
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub const fn timeout(&self) -> StdDuration {
        SMTP_TIMEOUT
    }
}

#[async_trait]
impl Mailer for SmtpMailer {
    async fn deliver(
        &self,
        recipient: &NormalizedEmail,
        idempotency_key: &str,
        message: &RenderedMail,
    ) -> Result<(), MailError> {
        let to: Address = recipient
            .as_str()
            .parse()
            .map_err(|_| MailError::permanent("invalid_recipient"))?;
        let envelope = Envelope::new(Some(self.from.clone()), vec![to])
            .map_err(|_| MailError::permanent("invalid_envelope"))?;
        let mut bytes = smtp_message(
            self.from.as_ref(),
            recipient.as_str(),
            idempotency_key,
            message,
        )?;
        let result = self.transport.send_raw(&envelope, &bytes).await;
        bytes.zeroize();
        result.map(|_: Response| ()).map_err(classify_smtp_error)
    }
}

fn smtp_message(
    from: &str,
    recipient: &str,
    idempotency_key: &str,
    message: &RenderedMail,
) -> Result<Vec<u8>, MailError> {
    let stable_id = idempotency_key
        .rsplit(':')
        .next()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| MailError::permanent("invalid_idempotency_key"))?;
    let boundary = format!("folioharbor-{stable_id}");
    let encoded_subject = BASE64.encode(message.subject.as_bytes());
    let mut output = Vec::with_capacity(message.text.len() + message.html.len() + 768);
    write!(
        output,
        "From: <{from}>\r\nTo: <{recipient}>\r\nSubject: =?UTF-8?B?{encoded_subject}?=\r\nMessage-ID: <{stable_id}@folioharbor>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n"
    )
    .map_err(|_| MailError::permanent("message_format_failed"))?;
    write_body(&mut output, &message.text);
    write!(
        output,
        "--{boundary}\r\nContent-Type: text/html; charset=UTF-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n"
    )
    .map_err(|_| MailError::permanent("message_format_failed"))?;
    write_body(&mut output, &message.html);
    write!(output, "--{boundary}--\r\n")
        .map_err(|_| MailError::permanent("message_format_failed"))?;
    Ok(output)
}

fn write_body(output: &mut Vec<u8>, body: &str) {
    for line in body.split('\n') {
        output.extend_from_slice(line.trim_end_matches('\r').as_bytes());
        output.extend_from_slice(b"\r\n");
    }
}

fn classify_smtp_error(error: lettre::transport::smtp::Error) -> MailError {
    if error.is_permanent() {
        MailError::permanent("smtp_permanent")
    } else {
        MailError::transient("smtp_transient")
    }
}

pub struct WorkerHandlers {
    imports: Arc<ProcessImportJob>,
    cleanup: Option<Arc<CleanupImports>>,
}

impl WorkerHandlers {
    #[must_use]
    pub const fn new(imports: Arc<ProcessImportJob>) -> Self {
        Self {
            imports,
            cleanup: None,
        }
    }

    #[must_use]
    pub fn with_cleanup(imports: Arc<ProcessImportJob>, cleanup: Arc<CleanupImports>) -> Self {
        Self {
            imports,
            cleanup: Some(cleanup),
        }
    }
}

#[async_trait]
impl JobDispatcher for WorkerHandlers {
    async fn dispatch(&self, job: LeasedJob) -> Result<(), JobFailure> {
        match job.kind {
            JobKind::ImportEpub => self.imports.execute(job).await.map(|_| ()),
            kind @ (JobKind::ExpireUploadsAndReservations
            | JobKind::PurgeFailedUploads
            | JobKind::CollectBlobsLater) => {
                let cleanup =
                    self.cleanup
                        .as_ref()
                        .ok_or_else(|| JobFailure::OperatorRequired {
                            code: "cleanup_not_configured",
                            summary: "cleanup handler is not configured".to_owned(),
                        })?;
                cleanup
                    .run_kind(
                        &format!("cleanup-{}", job.job_id.as_uuid()),
                        kind,
                        OffsetDateTime::now_utc(),
                        100,
                    )
                    .await
                    .map(|_| ())
                    .map_err(|_| JobFailure::Transient {
                        code: "cleanup_unavailable",
                        retry_at: OffsetDateTime::now_utc() + Duration::minutes(1),
                    })
            }
        }
    }
}

#[cfg(test)]
mod smtp_capture_tests {
    #![allow(clippy::expect_used)]

    use std::env;

    use folioharbor_application::{mail::RenderedMail, ports::Mailer as _};
    use folioharbor_domain::identity::NormalizedEmail;
    use lettre::{
        AsyncSmtpTransport, Tokio1Executor,
        transport::smtp::{client::Tls, client::TlsParameters},
    };

    use super::{SMTP_TIMEOUT, SmtpMailer, SmtpSecurity};

    #[tokio::test]
    #[ignore = "requires a STARTTLS Mailpit instance"]
    async fn starttls_capture_receives_multipart_message() {
        let port = env::var("FOLIOHARBOR_SMTP_CAPTURE_PORT")
            .expect("capture port")
            .parse::<u16>()
            .expect("numeric capture port");
        let tls = TlsParameters::builder("localhost".to_owned())
            // The capture service uses an ephemeral self-signed certificate. Production
            // construction above always retains certificate and hostname verification.
            .dangerous_accept_invalid_certs(true)
            .build()
            .expect("capture TLS parameters");
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("localhost")
            .port(port)
            .tls(Tls::Required(tls))
            .timeout(Some(SMTP_TIMEOUT))
            .build();
        let mailer = SmtpMailer {
            transport,
            from: "noreply@example.com".parse().expect("sender"),
            security: SmtpSecurity::RequiredStartTls,
            port,
        };
        let recipient = NormalizedEmail::parse("capture@example.com").expect("recipient");
        let rendered = RenderedMail {
            subject: "Capture sentinel".to_owned(),
            text: "Plain capture https://library.example/verify-email?token=capture-token\n"
                .to_owned(),
            html: "<p>HTML capture <a href=\"https://library.example/verify-email?token=capture-token\">verify</a></p>".to_owned(),
        };

        mailer
            .deliver(
                &recipient,
                "mail:verification:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                &rendered,
            )
            .await
            .expect("STARTTLS capture delivery");
    }
}
