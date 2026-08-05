use crate::{
    id::{InvitationId, LibraryId, UserId},
    identity::NormalizedEmail,
    time::OffsetDateTime,
};

use super::role::RoleCode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invitation {
    pub invitation_id: InvitationId,
    pub library_id: LibraryId,
    pub invited_by: UserId,
    pub normalized_email: NormalizedEmail,
    pub display_email: String,
    pub role: RoleCode,
    pub expires_at: OffsetDateTime,
}
