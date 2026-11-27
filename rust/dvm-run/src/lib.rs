//! dvm.run's SDK: command sessions or supplied programs, and point-in-time inspection.
//!
//! ```no_run
//! # async fn example() -> dvm_run::Result<()> {
//! let client = dvm_run::Client::from_env()?;
//! let sandbox = client.create_sandbox("alpine:3.20").command(["echo", "hello"]).await?;
//! assert_eq!(sandbox.wait().await?, 0);
//! assert_eq!(sandbox.output().await?.stdout.bytes(), b"hello\n");
//! let point = sandbox.seek_end().await?;
//! println!("{:?}", point.stat("/etc/os-release").await?.kind);
//! # Ok(())
//! # }
//! ```
mod client;
mod command;
mod credentials;
mod error;
mod login;
mod point;
mod sandbox;
mod sandbox_session;

pub use client::{Client, DEFAULT_API_URL};
pub use credentials::{credentials_path, save_key};
pub use dvm_api as api;
pub use error::{Error, Result};
pub use login::{LoggedIn, login};
pub use point::{SandboxPoint, SeekBuilder, StatBuilder, StepBuilder};
pub use sandbox::{Sandbox, SandboxBuilder};

pub use command::{
    Command, CommandLine, CommandResult, RunBuilder, StartBuilder, WriteFileBuilder,
};
