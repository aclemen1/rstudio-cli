pub mod cli;
pub mod client_id;
pub mod commands;
pub mod error;
pub mod output;
pub mod rpc;
pub mod session;
pub mod socket;

pub const SKILL_VERSION: u32 = 1;
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
