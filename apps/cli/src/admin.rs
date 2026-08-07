use secrecy::{ExposeSecret as _, SecretString};
use std::io::IsTerminal as _;
use subtle::ConstantTimeEq as _;

use folioharbor_application::{
    operations::{BootstrapAdmin, BootstrapAdminCommand, BootstrapAdminOutcome},
    ports::{Argon2PasswordHasher, Clock, RandomSource},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PasswordPromptError {
    #[error("administrator password must be read from an interactive TTY")]
    NotTerminal,
    #[error("administrator password confirmation did not match")]
    Mismatch,
    #[error("administrator password is required")]
    Empty,
}

/// Validates two securely captured password prompts without exposing either value.
///
/// # Errors
///
/// Returns a safe classification for a non-TTY source, mismatch, or empty password.
pub fn confirm_password(
    is_terminal: bool,
    first: SecretString,
    second: &SecretString,
) -> Result<SecretString, PasswordPromptError> {
    if !is_terminal {
        return Err(PasswordPromptError::NotTerminal);
    }
    if first.expose_secret().is_empty() {
        return Err(PasswordPromptError::Empty);
    }
    if !bool::from(
        first
            .expose_secret()
            .as_bytes()
            .ct_eq(second.expose_secret().as_bytes()),
    ) {
        return Err(PasswordPromptError::Mismatch);
    }
    Ok(first)
}

#[derive(Clone, Copy)]
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> folioharbor_domain::time::OffsetDateTime {
        folioharbor_domain::time::OffsetDateTime::now_utc()
    }
}

#[derive(Clone, Copy)]
struct SystemRandom;

impl RandomSource for SystemRandom {
    fn fill(&self, destination: &mut [u8]) {
        if getrandom::fill(destination).is_err() {
            std::process::abort();
        }
    }
}

/// Securely prompts for and bootstraps a system administrator.
///
/// # Errors
///
/// Returns an error for forbidden password sources, a non-interactive prompt, invalid input, or
/// an unavailable bootstrap dependency.
pub async fn run(email: String) -> anyhow::Result<()> {
    if std::env::var_os("FOLIOHARBOR_ADMIN_PASSWORD").is_some()
        || std::env::var_os("FOLIOHARBOR_ADMIN_PASSWORD_FILE").is_some()
    {
        anyhow::bail!("administrator password environment sources are forbidden");
    }
    let is_terminal = std::io::stdin().is_terminal();
    if !is_terminal {
        anyhow::bail!(PasswordPromptError::NotTerminal);
    }
    let first = SecretString::from(rpassword::prompt_password("Administrator password: ")?);
    let second = SecretString::from(rpassword::prompt_password("Confirm password: ")?);
    let password = confirm_password(is_terminal, first, &second)?;
    let url = crate::migrate::owner_database_url()?;
    let pool = folioharbor_postgres::connect_owner(&url).await?;
    let repository = folioharbor_postgres::PgOperationsRepository::new(pool.clone());
    let hasher = Argon2PasswordHasher::new(SystemRandom);
    let outcome = BootstrapAdmin::new(&repository, &hasher, &SystemClock)
        .execute(BootstrapAdminCommand { email, password })
        .await?;
    match outcome {
        BootstrapAdminOutcome::Created => println!("system administrator created"),
        BootstrapAdminOutcome::AlreadyAdministrator => {
            println!("system administrator already exists; no changes made");
        }
    }
    pool.close().await;
    Ok(())
}
