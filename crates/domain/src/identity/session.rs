use crate::time::OffsetDateTime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountStatus {
    PendingVerification,
    Verified,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStatus {
    Active,
    IdleExpired,
    AbsolutelyExpired,
    Revoked,
}

impl SessionStatus {
    #[must_use]
    pub fn at(
        revoked_at: Option<OffsetDateTime>,
        last_seen_at: OffsetDateTime,
        idle_expires_at: OffsetDateTime,
        absolute_expires_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> Self {
        if revoked_at.is_some() {
            Self::Revoked
        } else if now >= absolute_expires_at {
            Self::AbsolutelyExpired
        } else if now >= idle_expires_at || now < last_seen_at {
            Self::IdleExpired
        } else {
            Self::Active
        }
    }
}
