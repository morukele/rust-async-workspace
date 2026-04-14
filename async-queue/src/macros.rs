//! macros
//! =========
//!
//! Author: oghenemarho
//! Created: 02/04/2026
//! Project: async-rust-workspace
//!
//! Description:
//!

#[macro_export]
macro_rules! spawn_task {
    ($future:expr) => {
        spawn_task!($future, FutureType::Low)
    };
    ($future:expr, $order:expr) => {
        spawn_task($future, $order)
    };
}
