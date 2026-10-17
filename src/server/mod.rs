//! The server: accepting connections and answering them.
//!
//! At the moment this answers the handshake and the status exchange, which is
//! what makes a server visible in the multiplayer browser. Login, configuration
//! and the world come after.

pub mod log;
pub mod status;

pub use status::{StatusServer, PROTOCOL_VERSION};