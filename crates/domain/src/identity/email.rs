use std::fmt;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NormalizedEmail(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmailError;

impl NormalizedEmail {
    /// Trims Unicode edge whitespace and lowercases the ASCII domain portion.
    ///
    /// # Errors
    ///
    /// Returns [`EmailError`] when the address lacks one non-empty local and domain part.
    pub fn parse(value: &str) -> Result<Self, EmailError> {
        let trimmed = value.trim();
        let (local, domain) = trimmed.rsplit_once('@').ok_or(EmailError)?;
        if local.is_empty()
            || domain.is_empty()
            || local.contains('@')
            || trimmed.chars().any(char::is_whitespace)
        {
            return Err(EmailError);
        }
        let domain = domain
            .chars()
            .map(|character| character.to_ascii_lowercase())
            .collect::<String>();
        Ok(Self(format!("{local}@{domain}")))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EmailError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid email address")
    }
}

impl std::error::Error for EmailError {}
