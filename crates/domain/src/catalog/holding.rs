use crate::id::{HoldingId, LibraryId, ManifestationId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Holding {
    pub id: HoldingId,
    pub library_id: LibraryId,
    pub manifestation_id: ManifestationId,
}
