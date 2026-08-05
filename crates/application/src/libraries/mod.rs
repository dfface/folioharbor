mod accept_invitation;
mod api;
mod change_role;
mod invite_member;
mod provision_personal;
mod remove_member;
mod settings;

pub use accept_invitation::{AcceptInvitation, AcceptInvitationCommand};
pub use api::*;
pub use change_role::{ChangeMemberRole, ChangeMemberRoleCommand};
pub use invite_member::{InviteMember, InviteMemberCommand};
pub use provision_personal::{ProvisionPersonalLibrary, ProvisionPersonalLibraryCommand};
pub use remove_member::{RemoveMember, RemoveMemberCommand};
pub use settings::{UpdateLibrarySettings, UpdateLibrarySettingsCommand};

use crate::{error::AppError, ports::LibraryMutationOutcome};

fn mutation_result(outcome: LibraryMutationOutcome) -> Result<(), AppError> {
    match outcome {
        LibraryMutationOutcome::Applied => Ok(()),
        LibraryMutationOutcome::Forbidden => Err(AppError::Forbidden {
            code: "library_owner_required",
        }),
        LibraryMutationOutcome::NotFound => Err(AppError::NotFound {
            code: "membership_not_found",
        }),
        LibraryMutationOutcome::LastOwner => Err(AppError::Conflict {
            code: "library_requires_owner",
        }),
    }
}
