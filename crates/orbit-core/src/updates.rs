//! The update check: asks GitHub for the latest release's `latest.json` (the
//! Tauri updater's manifest) and compares versions. Installing is the
//! updater's job; this only tells whether there is something to install.
use serde::Serialize;

/// The Tauri updater's endpoint (`tauri.conf.json`). `ORBIT_UPDATE_URL`
/// overrides it (any URL curl reads, `file://` included), for tests.
pub const LATEST_URL: &str = "https://github.com/igorsegallafa/orbit/releases/latest/download/latest.json";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: Option<String>,
}

/// `Ok(Some(release))` when the latest release is newer than `current`.
pub fn check(current: &str) -> Result<Option<Release>, String> {
    let url = std::env::var("ORBIT_UPDATE_URL").unwrap_or_else(|_| LATEST_URL.to_string());
    let out = crate::proc::cmd("curl")
        .args(["-sSfL", "--max-time", "15", &url])
        .output()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { "the release check failed".into() } else { err });
    }
    parse(&String::from_utf8_lossy(&out.stdout), current)
}

fn parse(raw: &str, current: &str) -> Result<Option<Release>, String> {
    let v: serde_json::Value = serde_json::from_str(raw).map_err(|e| format!("invalid latest.json: {e}"))?;
    let version = v["version"].as_str().ok_or("latest.json has no version")?.trim_start_matches('v').to_string();
    let notes = v["notes"].as_str().filter(|n| !n.trim().is_empty()).map(str::to_string);
    Ok(newer(&version, current).then_some(Release { version, notes }))
}

/// Whether `a` is a later version than `b` (numeric parts; a pre-release
/// sorts before its release).
pub fn newer(a: &str, b: &str) -> bool {
    key(a) > key(b)
}

fn key(v: &str) -> (Vec<u64>, bool) {
    let v = v.trim().trim_start_matches('v');
    let (main, pre) = match v.split_once('-') {
        Some((m, _)) => (m, true),
        None => (v, false),
    };
    let mut parts: Vec<u64> = main.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    while parts.last() == Some(&0) {
        parts.pop();
    }
    (parts, !pre)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("v1.10.0", "1.9.3"));
        assert!(!newer("0.1.0", "0.1"));
        assert!(newer("0.2.0", "0.2.0-beta.1"));
        assert!(!newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn reads_the_manifest() {
        let raw = r#"{"version":"v0.3.0","notes":"Fixes","pub_date":"x","platforms":{}}"#;
        assert_eq!(parse(raw, "0.1.0").unwrap(), Some(Release { version: "0.3.0".into(), notes: Some("Fixes".into()) }));
        assert_eq!(parse(raw, "0.3.0").unwrap(), None);
        assert!(parse("{}", "0.1.0").is_err());
    }
}
