use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::models::HistoryEntry;

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub entries: Vec<HistoryEntry>,
}

impl Session {
    pub fn from_entries(entries: &[HistoryEntry]) -> Self {
        Self {
            version: 1,
            entries: entries.to_vec(),
        }
    }
}

pub fn sessions_dir() -> anyhow::Result<PathBuf> {
    let dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
        .join(".crowbar")
        .join("sessions");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn save(entries: &[HistoryEntry], name: &str) -> anyhow::Result<PathBuf> {
    let dir = sessions_dir()?;
    let path = dir.join(format!("{}.json", name));
    let session = Session::from_entries(entries);
    let json = serde_json::to_string_pretty(&session)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

pub fn load(path: &Path) -> anyhow::Result<Vec<HistoryEntry>> {
    let json = std::fs::read_to_string(path)?;
    let session: Session = serde_json::from_str(&json)?;
    Ok(session.entries)
}

pub fn auto_save_name() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("session-{}", now)
}
