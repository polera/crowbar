use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn save(rules: &[Rule], name: &str) -> anyhow::Result<PathBuf> {
    let dir = rules_dir()?;
    let path = dir.join(format!("{}.json", name));
    let file = RulesFile::from_rules(rules);
    let json = serde_json::to_string_pretty(&file)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

pub fn save_to(rules: &[Rule], path: &Path) -> anyhow::Result<()> {
    let file = RulesFile::from_rules(rules);
    let json = serde_json::to_string_pretty(&file)?;
    std::fs::write(path, json)?;
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
