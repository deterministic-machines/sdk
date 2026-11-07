use serde::{Deserialize, Serialize};

/// A point in time: milliseconds since the Unix epoch, UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(export_to = "controller-api.ts"))]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    pub fn from_unix_millis(millis: i64) -> Self {
        Self(millis)
    }

    pub fn unix_millis(self) -> i64 {
        self.0
    }

    /// The host's clock. Not in the browser, which has no `SystemTime`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn now() -> Self {
        let since_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970");
        Self(i64::try_from(since_epoch.as_millis()).expect("milliseconds fit an i64"))
    }
}
