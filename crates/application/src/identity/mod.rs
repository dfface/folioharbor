mod api;
mod login;
mod logout;
mod register;
mod reset_password;
mod sessions;
mod verify;

use folioharbor_domain::id::ErrorId;

use crate::error::AppError;

pub use api::*;
pub use login::{IssuedSession, Login, LoginCommand};
pub use logout::{Logout, LogoutCommand};
pub use register::{PendingAccount, RegisterAccount, RegisterAccountCommand};
pub use reset_password::{
    CompletePasswordReset, CompletePasswordResetCommand, PasswordResetComplete,
    PasswordResetRequested, RequestPasswordReset, RequestPasswordResetCommand,
};
pub use sessions::*;
pub use verify::{VerifiedAccount, VerifyEmail, VerifyEmailCommand};

pub const VERIFICATION_LIFETIME: time::Duration = time::Duration::hours(24);
pub const RESET_LIFETIME: time::Duration = time::Duration::hours(1);
pub const SESSION_IDLE_LIFETIME: time::Duration = time::Duration::days(30);
pub const SESSION_ABSOLUTE_LIFETIME: time::Duration = time::Duration::days(90);

fn internal_error() -> AppError {
    AppError::Internal {
        error_id: ErrorId::new(),
    }
}
