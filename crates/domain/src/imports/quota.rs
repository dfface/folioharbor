pub use super::blob::ByteCount;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaReservationState {
    Active,
    Consumed,
    Released,
}
