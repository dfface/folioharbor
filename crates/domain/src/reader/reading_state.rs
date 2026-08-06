use super::ReadiumLocator;
use crate::{
    id::{ContentUnitId, DeviceId, ManifestationId, PublicationPackageId},
    time::OffsetDateTime,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ReadingProgress {
    pub manifestation_id: ManifestationId,
    pub package_id: Option<PublicationPackageId>,
    pub content_unit_id: Option<ContentUnitId>,
    pub locator: ReadiumLocator,
    pub version: u64,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceReadingState {
    pub device_id: DeviceId,
    pub locator: ReadiumLocator,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReadingUpdateOutcome {
    Updated {
        global: ReadingProgress,
        device: DeviceReadingState,
    },
    Conflict {
        global: Option<ReadingProgress>,
        device: DeviceReadingState,
    },
}
