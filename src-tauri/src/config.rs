// Global Orbit config: catalog of repos and groups, persisted as YAML.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub name: String,
    pub repo: String,
    /// Build command, run through the platform shell in the repo folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildCmd>,
    /// Build output dir shared with the base clone (default `dist`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_output: Option<String>,
    /// Share only these sub-paths of the output dir instead of all of it
    /// (e.g. `vcpkg_installed` when the output bakes in worktree paths).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build_output_shared: Vec<String>,
    /// false = branch-only: the feature branch is checked out in the base
    /// clone itself (repos too heavy for a second checkout).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub worktree: bool,
    /// Where the base clone lives when not in the clones folder (an
    /// existing checkout or a folder picked at clone time). Orbit never
    /// deletes a clone at a custom path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}

impl Service {
    pub fn new(name: String, repo: String) -> Self {
        Service {
            name,
            repo,
            build: None,
            build_output: None,
            build_output_shared: Vec::new(),
            worktree: true,
            path: None,
        }
    }

    /// The custom clone location, if one was set.
    pub fn custom_path(&self) -> Option<PathBuf> {
        self.path.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(PathBuf::from)
    }

    pub fn output_dir(&self) -> &str {
        self.build_output.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("dist")
    }

    /// Worktree-relative dirs linked to the base clone so a fresh worktree
    /// reuses its build output (only for repos with a build command).
    pub fn shared_output_paths(&self) -> Vec<PathBuf> {
        if self.build.is_none() {
            return Vec::new();
        }
        let out = PathBuf::from(self.output_dir());
        if self.build_output_shared.is_empty() {
            vec![out]
        } else {
            self.build_output_shared.iter().map(|sub| out.join(sub)).collect()
        }
    }
}

/// A build command for every platform, or one per platform (keys match
/// Node's `process.platform`, as in the xm config).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum BuildCmd {
    All(String),
    PerOs {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        win32: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linux: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        darwin: Option<String>,
    },
}

impl BuildCmd {
    pub fn for_current_os(&self) -> Option<&str> {
        let cmd = match self {
            BuildCmd::All(c) => Some(c),
            BuildCmd::PerOs { win32, linux, darwin } => {
                if cfg!(windows) {
                    win32.as_ref()
                } else if cfg!(target_os = "macos") {
                    darwin.as_ref()
                } else {
                    linux.as_ref()
                }
            }
        };
        cmd.map(|c| c.as_str()).filter(|c| !c.trim().is_empty())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub groups: HashMap<String, Vec<String>>,
    /// Which CLI agent + model Orbit uses for AI features (Plan etc).
    #[serde(default)]
    pub ai: AiSettings,
    /// Folder for new base clones (default <root>/repos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repos_dir: Option<String>,
    /// Folder for workspaces and their worktrees (default <root>/workspaces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspaces_dir: Option<String>,
    /// Per-language server settings, keyed by language id ("cpp", "rust"…);
    /// languages without an entry use the first server found on PATH.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub language_servers: HashMap<String, LanguageServerSetting>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LanguageServerSetting {
    /// Command line replacing the detected server ("clangd --log=error").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// No server for this language (plain highlighting only).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// Diagnostic codes the editor never shows ("unused-includes").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_diagnostics: Vec<String>,
    /// The editor shows errors only (no warnings, infos or hints).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub errors_only: bool,
}

fn non_empty(p: &Option<String>) -> Option<PathBuf> {
    p.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from)
}

impl Config {
    pub fn repos_dir_override(&self) -> Option<PathBuf> {
        non_empty(&self.repos_dir)
    }

    pub fn workspaces_dir_override(&self) -> Option<PathBuf> {
        non_empty(&self.workspaces_dir)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AiSettings {
    /// "claude" | "opencode"
    #[serde(default = "default_agent")]
    pub agent: String,
    /// claude: model alias ("claude-sonnet-5"); opencode: "provider/model".
    #[serde(default = "default_model")]
    pub model: String,
    /// Model for short one-shot drafts (commit messages, PR descriptions).
    /// Unset: claude uses Haiku, other agents use `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_model: Option<String>,
}

fn default_agent() -> String {
    "claude".into()
}
fn default_model() -> String {
    "claude-sonnet-5".into()
}

impl AiSettings {
    /// CLI binary for the configured agent (unknown values run claude).
    pub fn agent_bin(&self) -> String {
        match self.agent.as_str() {
            "opencode" | "omp" => self.agent.clone(),
            _ => "claude".into(),
        }
    }

    /// Model for short one-shot drafts; see `fast_model`.
    pub fn draft_model(&self) -> String {
        match self.fast_model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            Some(m) => m.to_string(),
            None if self.agent_bin() == "claude" => "claude-haiku-4-5".into(),
            None => self.model.clone(),
        }
    }
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            agent: default_agent(),
            model: default_model(),
            fast_model: None,
        }
    }
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

pub(crate) fn config_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("ORBIT_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
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

/// User home. `$HOME` is usually unset on Windows outside Git Bash, so this
/// goes through `std::env::home_dir` (HOME, then USERPROFILE / profile API).
pub(crate) fn home_dir() -> Result<PathBuf, String> {
    std::env::home_dir().ok_or_else(|| "could not resolve the home directory".to_string())
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
        cfg.services.push(Service::new("svc-a".into(), "git@x:svc-a.git".into()));
        cfg.groups
            .insert("g1".into(), vec!["svc-a".into(), "ghost".into()]);
        let err = cfg
            .validate()
            .expect_err("should reject unknown member");
        assert!(err.contains("ghost"));
    }

    #[test]
    fn parses_xm_style_build_settings() {
        let raw = "services:
  - name: engine
    repo: o/engine
    build:
      win32: cmake --preset win
      linux: cmake --preset lin
    buildOutput: build
    buildOutputShared: [vcpkg_installed]
  - name: assets
    repo: o/assets
    worktree: false
  - name: web
    repo: o/web
    build: pnpm build
";
        let cfg: Config = serde_yaml::from_str(raw).unwrap();
        let engine = &cfg.services[0];
        assert_eq!(
            engine.shared_output_paths(),
            vec![PathBuf::from("build").join("vcpkg_installed")]
        );
        let expected = if cfg!(windows) { Some("cmake --preset win") } else if cfg!(target_os = "macos") { None } else { Some("cmake --preset lin") };
        assert_eq!(engine.build.as_ref().unwrap().for_current_os(), expected);
        assert!(!cfg.services[1].worktree);
        assert!(cfg.services[1].shared_output_paths().is_empty());
        assert_eq!(cfg.services[2].shared_output_paths(), vec![PathBuf::from("dist")]);
        // round-trip keeps the file minimal for plain repos
        let out = serde_yaml::to_string(&Service::new("a".into(), "o/a".into())).unwrap();
        assert_eq!(out.trim(), "name: a
repo: o/a");
    }

    #[test]
    fn validate_rejects_duplicate_service_names() {
        let mut cfg = Config::default();
        for _ in 0..2 {
            cfg.services.push(Service::new("dup".into(), "git@x:dup.git".into()));
        }
        assert!(cfg.validate().is_err());
    }
}
