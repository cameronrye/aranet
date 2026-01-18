//! Command implementations for the CLI.

mod alias;
mod cache;
mod doctor;
mod history;
mod info;
mod read;
mod scan;
mod set;
mod status;
mod sync;
mod watch;

pub use alias::{AliasAction, cmd_alias};
pub use cache::cmd_cache;
pub use doctor::cmd_doctor;
pub use history::{HistoryArgs, cmd_history};
pub use info::cmd_info;
pub use read::{DeviceReading, cmd_read};
pub use scan::cmd_scan;
pub use set::cmd_set;
pub use status::cmd_status;
pub use sync::{SyncArgs, cmd_sync};
pub use watch::{WatchArgs, cmd_watch};
