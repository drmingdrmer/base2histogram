#[allow(clippy::module_inception)]
mod slot;
mod slot_queue;

pub(crate) use slot::Slot;
pub(crate) use slot_queue::SlotQueue;
