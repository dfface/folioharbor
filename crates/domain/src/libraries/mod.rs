pub mod invitation;
pub mod membership;
pub mod role;

use crate::id::LibraryId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Library {
    pub library_id: LibraryId,
    pub name: String,
}
