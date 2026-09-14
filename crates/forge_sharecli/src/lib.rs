//! `forge_sharecli` — ShareCLI realtime relay (P3.3).
//!
//! In-process broadcast channels, bounded queues, and a cloneable hub that
//! ties them together. The crate is the local fanout substrate; the
//! `transport` module adds SSE and WebSocket adapters that stream the
//! fanout out to TCP clients and route inbound messages back through the
//! hub.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use forge_sharecli::{ShareHub, ShareMessage};
//!
//! # async fn demo() {
//! let hub = ShareHub::new();
//! let mut sub = hub.subscribe("chat").unwrap();
//! hub.publish_text("chat", "hello, world").unwrap();
//! let msg = sub.recv().await.unwrap();
//! assert_eq!(msg.payload.as_str().unwrap(), "hello, world");
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod channel;
pub mod commands;
pub mod error;
pub mod hub;
pub mod message;
pub mod queue;
pub mod transport;

pub use channel::{Channel, Subscriber};
pub use commands::{Cli, CliError, Command as ShareCommand, run_command};
pub use error::ShareError;
pub use hub::ShareHub;
pub use message::ShareMessage;
pub use queue::{Queue, QueueConsumer, QueueProducer};

/// Re-export of the CLI command tree under the module path required by the
/// `sharecli attach` ingestion workflow. Downstream callers can refer to
/// the `Attach` subcommand as `forge_sharecli::commands::Command::Attach`.
pub use commands::Command;
