// Global Orbit config: catalog of repos and groups, persisted as YAML.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Service {
    pub name: String,
    pub repo: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub groups: HashMap<String, Vec<String>>,
}

impl Config {
    fn path() -> Result<PathBuf, String> {
        let dir = config_dir()?;
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir.join("config.yaml"))
    }

    pub fn load() -> Result<Config, String> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Config::default());
        }
        let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_yaml::from_str(&raw).map_err(|e| format!("invalid config.yaml: {e}"))
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path()?;
        let raw = serde_yaml::to_string(self).map_err(|e| e.to_string())?;
        fs::write(&path, raw).map_err(|e| e.to_string())
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut names = std::collections::HashSet::new();
        for s in &self.services {
            if s.name.trim().is_empty() {
                return Err("repo with empty name".into());
            }
            if !names.insert(s.name.clone()) {
                return Err(format!("duplicate repo: {}", s.name));
            }
        }
        for (group, members) in &self.groups {
            for m in members {
                if !names.contains(m) {
                    return Err(format!(
                        "group '{group}' references unknown repo '{m}'"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn config_dir() -> Result<PathBuf, String> {
    let home = home_dir()?;
    #[cfg(target_os = "macos")]
    {
        Ok(home.join("Library/Application Support/orbit"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(home.join(".config/orbit"))
    }
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "could not resolve $HOME".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let sample = r#"
services:
  - name: audience-svc
    repo: git@github.com:acme/audience-svc.git
  - name: billing-svc
    repo: git@github.com:acme/billing-svc.git
groups:
  core:
    - audience-svc
    - billing-svc
"#;
        let cfg: Config = serde_yaml::from_str(sample).expect("should parse config");
        assert_eq!(cfg.services.len(), 2);
        assert_eq!(cfg.groups["core"].len(), 2);
        cfg.validate().expect("valid config");
    }

    #[test]
    fn validate_rejects_group_with_unknown_service() {
        let mut cfg = Config::default();
        cfg.services.push(Service {
            name: "svc-a".into(),
            repo: "git@x:svc-a.git".into(),
        });
        cfg.groups
            .insert("g1".into(), vec!["svc-a".into(), "ghost".into()]);
        let err = cfg
            .validate()
            .expect_err("should reject unknown member");
        assert!(err.contains("ghost"));
    }

    #[test]
    fn validate_rejects_duplicate_service_names() {
        let mut cfg = Config::default();
        for _ in 0..2 {
            cfg.services.push(Service {
                name: "dup".into(),
                repo: "git@x:dup.git".into(),
            });
        }
        assert!(cfg.validate().is_err());
    }
}
