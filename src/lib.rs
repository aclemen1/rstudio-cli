pub mod cli;
pub mod client_id;
pub mod commands;
pub mod error;
pub mod output;
pub mod r_eval;
pub mod rpc;
pub mod schema;
pub mod session;
pub mod socket;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
