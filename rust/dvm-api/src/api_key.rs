use serde::{Deserialize, Serialize};

use crate::Timestamp;

/// One of a user's API keys, as they list them: never the key itself, of
/// which the controller keeps only the SHA-256. A request that bears it
/// (`Authorization: Bearer <key>`) acts as its user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    /// The key's last four characters, to tell it from the user's others.
    pub ending: String,
    pub created_at: Timestamp,
    /// When a request last bore it, to the minute; `None` if none has.
    pub last_used_at: Option<Timestamp>,
}

impl ApiKey {
    /// What every key starts with, so that a key pasted somewhere is known
    /// for one.
    pub const PREFIX: &str = "dvm_";

    /// The key as shown, all but its prefix and ending left out:
    /// `dvm_…3f9a`.
    pub fn masked(&self) -> String {
        format!("{}…{}", Self::PREFIX, self.ending)
    }
}

/// A key to make: `POST /api/keys`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct NewApiKey {
    pub name: String,
}

/// A key just made, and the key itself: shown this once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
pub struct CreatedApiKey {
    pub key: ApiKey,
    pub secret: String,
}
