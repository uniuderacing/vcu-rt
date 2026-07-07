//! Definitions for tasks that are run internally by the [`Runtime`](crate::Runtime).

mod can_manager;
mod power_monitor;
mod status;
mod tacom_ping;
pub use {can_manager::*, power_monitor::*, status::*, tacom_ping::*};
