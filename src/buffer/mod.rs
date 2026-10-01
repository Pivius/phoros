pub mod core;

pub use core::BPRB;
#[cfg(feature = "alloc")]
pub use core::construct::BoxedBPRB;
pub use core::construct::StackBPRB;
pub use core::iterate::EntryRange;
#[cfg(all(feature = "alloc", feature = "serde"))]
pub use core::persist::SavedState;

pub const DEFAULT_ANCHOR_INTERVAL: u64 = 60;
pub const DEFAULT_DELTA_THRESHOLD: f64 = 0.70;
