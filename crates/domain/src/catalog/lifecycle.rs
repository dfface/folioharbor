use crate::time::OffsetDateTime;
use time::Duration;

pub const ITEM_RECOVERY_WINDOW: Duration = Duration::days(7);
pub const BLOB_PURGE_DELAY: Duration = Duration::hours(24);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemLifecycle {
    Active,
    Deleted {
        deleted_at: OffsetDateTime,
        purge_eligible_at: OffsetDateTime,
    },
    PurgeEligible {
        deleted_at: OffsetDateTime,
        purge_eligible_at: OffsetDateTime,
    },
    Purged {
        deleted_at: OffsetDateTime,
        purge_eligible_at: OffsetDateTime,
        purged_at: OffsetDateTime,
    },
}

impl ItemLifecycle {
    #[must_use]
    pub const fn is_accessible(&self) -> bool {
        matches!(self, Self::Active)
    }

    #[must_use]
    pub fn delete(self, now: OffsetDateTime) -> Self {
        match self {
            Self::Active => Self::Deleted {
                deleted_at: now,
                purge_eligible_at: now + ITEM_RECOVERY_WINDOW,
            },
            state => state,
        }
    }

    #[must_use]
    pub fn restore(self, now: OffsetDateTime) -> Option<Self> {
        match self {
            Self::Active => Some(Self::Active),
            Self::Deleted {
                purge_eligible_at, ..
            } if now < purge_eligible_at => Some(Self::Active),
            Self::Deleted { .. } | Self::PurgeEligible { .. } | Self::Purged { .. } => None,
        }
    }

    #[must_use]
    pub fn advance(self, now: OffsetDateTime) -> Self {
        match self {
            Self::Deleted {
                deleted_at,
                purge_eligible_at,
            } if now >= purge_eligible_at => Self::PurgeEligible {
                deleted_at,
                purge_eligible_at,
            },
            state => state,
        }
    }

    #[must_use]
    pub fn purge(self, now: OffsetDateTime) -> Option<Self> {
        match self {
            Self::PurgeEligible {
                deleted_at,
                purge_eligible_at,
            } => Some(Self::Purged {
                deleted_at,
                purge_eligible_at,
                purged_at: now,
            }),
            Self::Purged { .. } => Some(self),
            Self::Active | Self::Deleted { .. } => None,
        }
    }

    #[must_use]
    pub fn blob_purge_after(&self) -> Option<OffsetDateTime> {
        match self {
            Self::Purged { purged_at, .. } => Some(*purged_at + BLOB_PURGE_DELAY),
            Self::Active | Self::Deleted { .. } | Self::PurgeEligible { .. } => None,
        }
    }
}
