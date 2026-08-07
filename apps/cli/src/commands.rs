#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Migrate,
    CreateAdmin { email: String },
    CheckStorage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ParseError {
    #[error("password arguments and password files are forbidden; use the secure TTY prompt")]
    PasswordSourceForbidden,
    #[error("invalid command; expected migrate, admin create --email EMAIL, or storage check")]
    Usage,
}

/// Parses the deliberately narrow operator CLI surface.
///
/// # Errors
///
/// Rejects unknown syntax and every password-bearing command-line form.
pub fn parse<I, S>(arguments: I) -> Result<Command, ParseError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let arguments = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect::<Vec<_>>();
    if arguments.iter().any(|argument| {
        matches!(
            argument.as_str(),
            "--password" | "--password-file" | "--password-env"
        ) || argument.starts_with("--password=")
            || argument.starts_with("--password-file=")
            || argument.starts_with("--password-env=")
    }) {
        return Err(ParseError::PasswordSourceForbidden);
    }
    match arguments.as_slice() {
        [command] if command == "migrate" => Ok(Command::Migrate),
        [storage, check] if storage == "storage" && check == "check" => Ok(Command::CheckStorage),
        [admin, create, email_flag, email]
            if admin == "admin" && create == "create" && email_flag == "--email" =>
        {
            Ok(Command::CreateAdmin {
                email: email.clone(),
            })
        }
        _ => Err(ParseError::Usage),
    }
}
