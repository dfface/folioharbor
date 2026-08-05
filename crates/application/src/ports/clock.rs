use folioharbor_domain::time::OffsetDateTime;

pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}
