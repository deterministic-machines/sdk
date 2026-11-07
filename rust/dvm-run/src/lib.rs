//! dvm.run's Rust SDK: sandboxes whose every run replays.
//!
//! A sandbox is a VM made from an image: your agent sends it commands and
//! files until it stops it. Everything it ran is its execution, which
//! replays, the same, once it ends.
//!
//! ```no_run
//! # async fn example() -> dvm_run::Result<()> {
//! use std::time::Duration;
//!
//! let client = dvm_run::Client::from_env()?;
//! let sbx = client.create_sandbox("python:3.12").seed(7).await?;
//! sbx.files().write("/app/main.py", "print(1 + 1)\n").await?;
//! let r = sbx
//!     .commands()
//!     .run("python main.py")
//!     .cwd("/app")
//!     .timeout(Duration::from_secs(60))
//!     .await?;
//! println!("{} {}", r.stdout, r.exit_code);
//!
//! let server = sbx.commands().spawn(["python", "-m", "http.server"]).await?;
//! server.kill().await?;
//! server.wait().await?;
//! sbx.stop().await?;
//!
//! let verdict = client.execution(sbx.id()).await?.replay().await?.wait().await?;
//! assert!(verdict.is_same());
//! # Ok(())
//! # }
//! ```
//!
//! [`Client::from_env`] takes dvm.run's address from `DVM_API_URL`, else
//! `https://sandbox.dvm.run`, and its key from `DVM_API_KEY`, else the one
//! `dvm-run login` saved for that address. The API's types are
//! [`dvm_api`]'s, here as [`api`].

mod client;
mod commands;
mod credentials;
mod error;
mod execution;
mod files;
mod login;
mod sandbox;

pub use client::{Client, DEFAULT_API_URL};
pub use commands::{
    Background, CommandBuilder, CommandHandle, CommandResult, Commands, Foreground, OutputCallback,
    WaitForCommand,
};
pub use credentials::{credentials_path, save_key};
pub use dvm_api as api;
pub use error::{Error, Result};
pub use execution::{Execution, Replay, ReplayBuilder};
pub use files::Files;
pub use login::{LoggedIn, login};
pub use sandbox::{Sandbox, SandboxBuilder};
