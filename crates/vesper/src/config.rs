//! Client provided settings, read from the initialization options the editor
//! sends once at startup.

use serde::Deserialize;

/// Settings vesper understands. Unknown keys are ignored, so a newer editor
/// config never breaks an older server.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// The directory that holds the dusk `lib` standard library. Vesper sets it
    /// as `DUSK_HOME` so the loader can resolve `@import std.*`. When it is
    /// absent, the loader falls back to the checkout it was built against.
    pub dusk_home: Option<String>,
}

impl Config {
    /// Reads settings from the raw `initializationOptions` value, falling back to
    /// the defaults when the value is missing or malformed.
    pub fn from_value(value: Option<serde_json::Value>) -> Self {
        value
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    /// Applies settings that must live in the process environment before any
    /// analysis runs.
    pub fn apply(&self) {
        if let Some(home) = &self.dusk_home {
            std::env::set_var("DUSK_HOME", home);
        }
    }
}
