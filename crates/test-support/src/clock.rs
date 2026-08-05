use folioharbor_application::ports::Clock;
use folioharbor_domain::time::OffsetDateTime;
use std::sync::{Arc, Mutex};

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

#[derive(Clone, Debug)]
pub struct FakeClock(Arc<Mutex<OffsetDateTime>>);

impl FakeClock {
    #[must_use]
    pub fn new(now: OffsetDateTime) -> Self {
        Self(Arc::new(Mutex::new(now)))
    }

    pub fn set(&self, now: OffsetDateTime) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = now;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> OffsetDateTime {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
