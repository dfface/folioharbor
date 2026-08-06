use crate::id::{HoldingId, ItemId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Item {
    pub id: ItemId,
    pub holding_id: HoldingId,
}
