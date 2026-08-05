use folioharbor_application::ports::Clock;
use folioharbor_domain::time::OffsetDateTime;

#[derive(Clone, Copy, Debug)]
pub struct FixedClock {
    now: OffsetDateTime,
}

impl FixedClock {
    #[must_use]
    pub const fn new(now: OffsetDateTime) -> Self {
        Self { now }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.now
    }
}
