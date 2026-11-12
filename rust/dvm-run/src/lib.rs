//! dvm.run's SDK: one recorded program and its replays.
//!
//! ```no_run
//! # async fn example() -> dvm_run::Result<()> {
//! let client = dvm_run::Client::from_env()?;
//! let sandbox = client.create_sandbox("alpine:3.20").command(["echo", "hello"]).await?;
//! assert_eq!(sandbox.wait().await?, 0);
//! assert_eq!(sandbox.output().await?.stdout.bytes(), b"hello\n");
//! let verdict = client.execution(sandbox.id()).await?.replay().await?.wait().await?;
//! assert!(verdict.is_same());
//! # Ok(())
//! # }
//! ```
mod client;
mod credentials;
mod error;
mod execution;
mod login;
mod sandbox;

pub use client::{Client, DEFAULT_API_URL};
pub use credentials::{credentials_path, save_key};
pub use dvm_api as api;
pub use error::{Error, Result};
pub use execution::{Execution, Replay};
pub use login::{LoggedIn, login};
pub use sandbox::{Sandbox, SandboxBuilder};
