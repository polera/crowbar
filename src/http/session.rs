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
            Some(SavedMacro {
                steps: macro_requests,
            })
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
    crate::fs_security::ensure_private_dir(&dir)?;
    Ok(dir)
}

pub fn save(
    entries: Vec<HistoryEntry>,
    macro_requests: Vec<RequestData>,
    name: &str,
) -> anyhow::Result<PathBuf> {
    let dir = sessions_dir()?;
    let path = dir.join(format!("{}.json", safe_name(name)?));
    let session = Session::new(entries, macro_requests);
    save_to(&session, &path)?;
    Ok(path)
}

pub fn save_to(session: &Session, path: &Path) -> anyhow::Result<()> {
    crate::fs_security::write_private_with(path, |file| {
        use std::io::Write;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &session).map_err(std::io::Error::other)?;
        writer.flush()
    })?;
    Ok(())
}

pub fn load(path: &Path) -> anyhow::Result<Session> {
    let json = read_input(path)?;
    let session: Session = serde_json::from_slice(&json)?;
    anyhow::ensure!(
        matches!(session.version, 1 | 2),
        "Unsupported session version {}",
        session.version
    );
    let mut ids = std::collections::HashSet::new();
    for entry in &session.entries {
        anyhow::ensure!(
            ids.insert(entry.request.id),
            "Duplicate request ID {} in session",
            entry.request.id
        );
    }
    Ok(session)
}

/// Bound allocation before deserializing untrusted session, HAR, or rules files.
pub(crate) fn read_input(path: &Path) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    const MAX_INPUT_BYTES: u64 = 256 * 1024 * 1024;
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path)?;
    anyhow::ensure!(
        file.metadata()?.len() <= MAX_INPUT_BYTES,
        "Input file exceeds 256 MiB limit"
    );
    file.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_INPUT_BYTES,
        "Input file exceeds 256 MiB limit"
    );
    Ok(bytes)
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
    fn unsupported_versions_and_duplicate_ids_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let entry = crate::test_support::entry();
        let duplicate = super::Session::new(vec![entry.clone(), entry], vec![]);
        super::save_to(&duplicate, &path).unwrap();
        assert!(super::load(&path).is_err());
        std::fs::write(&path, r#"{"version":99,"entries":[]}"#).unwrap();
        assert!(super::load(&path).is_err());
    }

    #[test]
    fn oversized_files_are_rejected_before_allocation() {
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file().set_len(256 * 1024 * 1024 + 1).unwrap();
        assert!(super::read_input(file.path()).is_err());
    }

    #[test]
    fn accepts_plain_names() {
        assert!(safe_name("session-123").is_ok());
        assert!(safe_name("my.session.v2").is_ok());
        assert!(safe_name("..foo").is_ok());
    }

    #[test]
    fn rejects_traversal_and_separators() {
        for bad in [
            "..",
            ".",
            "../../etc/cron.d/evil",
            "a/b",
            "/etc/passwd",
            "",
            "foo/",
        ] {
            assert!(safe_name(bad).is_err(), "should reject {bad:?}");
        }
    }
}
