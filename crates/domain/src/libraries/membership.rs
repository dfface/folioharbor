use crate::id::{LibraryId, UserId};

use super::role::RoleCode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MembershipFact {
    pub library_id: LibraryId,
    pub user_id: UserId,
    pub role: RoleCode,
}
