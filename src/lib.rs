pub mod cli;
pub mod client_id;
pub mod commands;
pub mod desktop_discovery;
pub mod error;
pub mod lock;
pub mod output;
pub mod policy;
pub mod r_eval;
pub mod rpc;
pub mod schema;
pub mod session;
pub mod transport;
pub mod update_check;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
