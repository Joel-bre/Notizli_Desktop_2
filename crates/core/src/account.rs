//! The pairing: the device token lives in the OS credential store (Windows
//! Credential Manager, macOS Keychain); who it belongs to and where to upload
//! are ordinary settings.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const SERVICE: &str = "ch.notizli.recorder";
const TOKEN_USER: &str = "device-token";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// The account this recorder is paired with (shown, never used to authenticate).
    #[serde(default)]
    pub paired_email: Option<String>,
    #[serde(default)]
    pub device_label: Option<String>,
    #[serde(default)]
    pub upload_url: Option<String>,
    /// Microphone device id; None follows the system default.
    #[serde(default)]
    pub mic_device: Option<String>,
}

pub struct SettingsFile(PathBuf);

impl SettingsFile {
    pub fn new(data_dir: &Path) -> Self {
        SettingsFile(data_dir.join("settings.json"))
    }

    pub fn load(&self) -> Settings {
        fs::read(&self.0).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
    }

    pub fn save(&self, s: &Settings) -> io::Result<()> {
        if let Some(dir) = self.0.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.0.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
        fs::rename(tmp, &self.0)
    }
}

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, TOKEN_USER).map_err(|e| e.to_string())
}

/// The device token, if paired.
pub fn load_token() -> Result<Option<String>, String> {
    match entry()?.get_password() {
        Ok(t) if !t.is_empty() => Ok(Some(t)),
        Ok(_) => Ok(None),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn save_token(token: &str) -> Result<(), String> {
    entry()?.set_password(token).map_err(|e| e.to_string())
}

pub fn delete_token() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
