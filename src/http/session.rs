use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::models::{HistoryEntry, RequestData};

#[derive(Serialize, Deserialize)]
pub struct SavedMacro {
    pub steps: Vec<RequestData>,
}

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub entries: Vec<HistoryEntry>,
    #[serde(default)]
    pub macros: Option<SavedMacro>,
}

impl Session {
    pub fn new(entries: &[HistoryEntry], macro_requests: Vec<RequestData>) -> Self {
        let macros = if macro_requests.is_empty() {
            None
        } else {
            Some(SavedMacro { steps: macro_requests })
        };
        Self {
            version: 2,
            entries: entries.to_vec(),
            macros,
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

pub fn save(entries: &[HistoryEntry], macro_requests: Vec<RequestData>, name: &str) -> anyhow::Result<PathBuf> {
    let dir = sessions_dir()?;
    let path = dir.join(format!("{}.json", name));
    let session = Session::new(entries, macro_requests);
    let json = serde_json::to_string_pretty(&session)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

pub fn load(path: &Path) -> anyhow::Result<Session> {
    let json = std::fs::read_to_string(path)?;
    let session: Session = serde_json::from_str(&json)?;
    Ok(session)
}

pub fn auto_save_name() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("session-{}", now)
}
