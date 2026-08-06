use folioharbor_application::mail::{
    DeliveryFailure, Locale, MailMessage, MailTemplate, render_message,
};
use folioharbor_domain::identity::NormalizedEmail;
use secrecy::SecretString;
use url::Url;

fn message(template: MailTemplate) -> MailMessage {
    MailMessage::new(
        None,
        NormalizedEmail::parse("reader@example.com").expect("valid recipient"),
        template,
        Locale::En,
        SecretString::from("single-use-token"),
    )
}

#[test]
fn verification_text_and_html_contain_the_same_single_use_link() {
    let rendered = render_message(
        &message(MailTemplate::Verification),
        &Url::parse("https://library.example/").expect("valid public URL"),
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
    let mut invitation = message(MailTemplate::Invitation);
    invitation.set_invitation_context("<Library & friends>", "editor");
    let rendered = render_message(
        &invitation,
        &Url::parse("https://library.example/").expect("valid public URL"),
    )
    .expect("configured base URL renders a message");

    assert!(rendered.html.contains("&lt;Library &amp; friends&gt;"));
    assert!(!rendered.html.contains("<Library & friends>"));
    assert!(!rendered.html.contains("<img"));
    assert!(!rendered.html.contains("src="));
}

#[test]
fn rendering_rejects_a_base_url_with_a_non_http_scheme() {
    let message = message(MailTemplate::PasswordReset);

    let base = Url::parse("mailto:reader@example.com").expect("parseable URL");
    assert!(render_message(&message, &base).is_err());
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
