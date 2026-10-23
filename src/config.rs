use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// Platform peripheral identifier. On macOS this is a CoreBluetooth UUID
    /// that is only valid on this Mac.
    pub peripheral_id: String,
    pub name: Option<String>,
}

pub fn path() -> Result<PathBuf> {
    let dir = dirs::config_dir().ok_or_else(|| anyhow!("no configuration directory"))?;
    Ok(dir.join("deskfit").join("config.json"))
}

pub fn load() -> Result<Config> {
    let path = path()?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}; run `deskfit setup` first", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn save(config: &Config) -> Result<PathBuf> {
    let path = path()?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, serde_json::to_string_pretty(config)? + "\n")?;
    Ok(path)
}
