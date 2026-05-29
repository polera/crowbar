use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Ensure a user-supplied name is a single, non-traversing path component so it
/// cannot escape the target directory (e.g. `../../etc/x` or an absolute path).
fn safe_name(name: &str) -> anyhow::Result<&str> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(c)), None) if c.to_str() == Some(name) => Ok(name),
        _ => Err(anyhow::anyhow!(
            "invalid name {name:?}: must be a single filename without path separators or '..'"
        )),
    }
}

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
    pub fn new(entries: Vec<HistoryEntry>, macro_requests: Vec<RequestData>) -> Self {
        let macros = if macro_requests.is_empty() {
            None
        } else {
            Some(SavedMacro { steps: macro_requests })
        };
        Self {
            version: 2,
            entries,
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

pub fn save(entries: Vec<HistoryEntry>, macro_requests: Vec<RequestData>, name: &str) -> anyhow::Result<PathBuf> {
    let dir = sessions_dir()?;
    let path = dir.join(format!("{}.json", safe_name(name)?));
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

#[cfg(test)]
mod tests {
    use super::safe_name;

    #[test]
    fn accepts_plain_names() {
        assert!(safe_name("session-123").is_ok());
        assert!(safe_name("my.session.v2").is_ok());
        assert!(safe_name("..foo").is_ok());
    }

    #[test]
    fn rejects_traversal_and_separators() {
        for bad in ["..", ".", "../../etc/cron.d/evil", "a/b", "/etc/passwd", "", "foo/"] {
            assert!(safe_name(bad).is_err(), "should reject {bad:?}");
        }
    }
}
