#![allow(clippy::expect_used)]

use folioharbor_cli::commands::{Command, ParseError, parse};
use secrecy::{ExposeSecret as _, SecretString};

#[test]
fn admin_create_accepts_only_an_email_and_never_a_password_source() {
    assert_eq!(
        parse(["admin", "create", "--email", "admin@example.com"]),
        Ok(Command::CreateAdmin {
            email: "admin@example.com".to_owned(),
        })
    );

    for arguments in [
        vec![
            "admin",
            "create",
            "--email",
            "admin@example.com",
            "--password",
            "unsafe",
        ],
        vec!["admin", "create", "--password-file", "/tmp/password"],
    ] {
        assert_eq!(parse(arguments), Err(ParseError::PasswordSourceForbidden));
    }
}

#[test]
fn storage_check_and_migrate_are_explicit_commands() {
    assert_eq!(parse(["storage", "check"]), Ok(Command::CheckStorage));
    assert_eq!(parse(["migrate"]), Ok(Command::Migrate));
}

#[test]
fn password_confirmation_requires_a_tty_and_two_matching_prompts() {
    let first = SecretString::from("correct horse battery staple".to_owned());
    let second = SecretString::from("correct horse battery staple".to_owned());
    let password = folioharbor_cli::admin::confirm_password(true, first, &second)
        .expect("matching TTY prompts");
    assert_eq!(password.expose_secret(), "correct horse battery staple");

    assert!(
        folioharbor_cli::admin::confirm_password(
            false,
            SecretString::from("password".to_owned()),
            &SecretString::from("password".to_owned()),
        )
        .is_err()
    );
    assert!(
        folioharbor_cli::admin::confirm_password(
            true,
            SecretString::from("first".to_owned()),
            &SecretString::from("second".to_owned()),
        )
        .is_err()
    );
}
