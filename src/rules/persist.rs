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

use super::Rule;

#[derive(Serialize, Deserialize)]
struct RulesFile {
    version: u32,
    rules: Vec<Rule>,
}

impl RulesFile {
    fn from_rules(rules: &[Rule]) -> Self {
        Self {
            version: 1,
            rules: rules.to_vec(),
        }
    }
}

pub fn rules_dir() -> anyhow::Result<PathBuf> {
    let dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
        .join(".crowbar")
        .join("rules");
    crate::fs_security::ensure_private_dir(&dir)?;
    Ok(dir)
}

pub fn save(rules: &[Rule], name: &str) -> anyhow::Result<PathBuf> {
    let dir = rules_dir()?;
    let path = dir.join(format!("{}.json", safe_name(name)?));
    let file = RulesFile::from_rules(rules);
    write_rules_file(&path, &file)?;
    Ok(path)
}

pub fn save_to(rules: &[Rule], path: &Path) -> anyhow::Result<()> {
    let file = RulesFile::from_rules(rules);
    write_rules_file(path, &file)?;
    Ok(())
}

fn write_rules_file(path: &Path, rules: &RulesFile) -> anyhow::Result<()> {
    crate::fs_security::write_private_with(path, |file| {
        use std::io::Write;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, rules).map_err(std::io::Error::other)?;
        writer.flush()
    })?;
    Ok(())
}

pub fn load(path: &Path) -> anyhow::Result<Vec<Rule>> {
    let json = std::fs::read_to_string(path)?;
    let file: RulesFile = serde_json::from_str(&json)?;
    Ok(file.rules)
}

pub fn auto_save_name() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("rules-{}", now)
}

#[cfg(test)]
mod tests {
    use super::safe_name;

    #[test]
    fn accepts_plain_names() {
        assert!(safe_name("rules-123").is_ok());
        assert!(safe_name("my.rules.v2").is_ok());
        assert!(safe_name("..foo").is_ok());
    }

    #[test]
    fn rejects_traversal_and_separators() {
        for bad in ["..", ".", "../etc/passwd", "a/b", "/etc/passwd", "", "foo/"] {
            assert!(safe_name(bad).is_err(), "should reject {bad:?}");
        }
    }
}
