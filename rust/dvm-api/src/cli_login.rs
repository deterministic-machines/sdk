use serde::{Deserialize, Serialize};

use crate::Timestamp;

/// A login to start from a terminal: `POST /api/cli-logins`, with no session
/// or key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewCliLogin {
    /// The terminal's machine, its hostname: the panel shows it, and the key
    /// is named after it.
    pub machine: String,
}

/// A login started: the panel's page that approves it, the code that page
/// shows, and the token the terminal collects its key with, which only the
/// terminal holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartedCliLogin {
    pub approve_url: String,
    /// `BCDF-GHJK`, which the terminal shows too, so the user sees the page
    /// is their terminal's.
    pub code: String,
    pub collect_token: String,
    pub expires_at: Timestamp,
}

/// A login from a terminal, as its page shows it to the user who may
/// approve it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct CliLogin {
    pub machine: String,
    pub code: String,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    /// Approved by this user: its terminal has its key, or will on its next
    /// ask.
    pub approved: bool,
}

/// What a terminal asks for its key with: `POST /api/cli-logins/key`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectCliLogin {
    pub collect_token: String,
}
