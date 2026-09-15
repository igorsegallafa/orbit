// Issue-tracker integrations (Shortcut, Linear). Read-only card fetching
// via each provider's API; tokens live in the credentials file, never in
// config.yaml.
//
// HTTP is shelled out to `curl` (consistent with git/gh/sqlite3 in this
// codebase) — no reqwest dependency for two simple JSON GET/POST calls.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub const SHORTCUT_API: &str = "https://api.app.shortcut.com/api/v3";
pub const LINEAR_API: &str = "https://api.linear.app/graphql";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackerKind {
    Shortcut,
    Linear,
    Figma,
}

impl TrackerKind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "shortcut" => Ok(TrackerKind::Shortcut),
            "linear" => Ok(TrackerKind::Linear),
            "figma" => Ok(TrackerKind::Figma),
            _ => Err(format!("unknown tracker: {s}")),
        }
    }

    fn token_key(self) -> &'static str {
        match self {
            TrackerKind::Shortcut => "shortcut_token",
            TrackerKind::Linear => "linear_token",
            TrackerKind::Figma => "figma_token",
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct CardInfo {
    pub id: String,
    pub title: String,
    pub state: String,
    pub url: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct IntegrationStatus {
    pub kind: String,
    pub connected: bool,
    pub account: Option<String>,
}

// ---------- credentials file (~/.config/orbit/credentials, 0600) ----------

pub fn credentials_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let dir = home.join(".config/orbit");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("credentials"))
}

fn load_credentials() -> HashMap<String, String> {
    let path = credentials_path().ok();
    let raw = path.and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
    serde_yaml::from_str(&raw).unwrap_or_default()
}

fn save_credentials(creds: &HashMap<String, String>) -> Result<(), String> {
    let path = credentials_path()?;
    let raw = serde_yaml::to_string(creds).map_err(|e| e.to_string())?;
    fs::write(&path, raw).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn token(kind: TrackerKind) -> Option<String> {
    let creds = load_credentials();
    creds
        .get(kind.token_key())
        .filter(|t| !t.trim().is_empty())
        .cloned()
}

// ---------- HTTP via curl ----------

fn curl_json(
    url: &str,
    auth_headers: &[(&str, String)],
    body: Option<&str>,
) -> Result<serde_json::Value, String> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--max-time", "10", "-H", "Content-Type: application/json"]);
    for (name, value) in auth_headers {
        cmd.args(["-H", &format!("{name}: {value}")]);
    }
    match body {
        Some(b) => {
            cmd.args(["-X", "POST", "-d", b]);
        }
        None => {
            cmd.arg("-X");
            cmd.arg("GET");
        }
    }
    cmd.arg(url);
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    if !out.status.success() {
        return Err("network request failed".into());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        return Err("empty response from server".into());
    }
    serde_json::from_str(&stdout).map_err(|e| format!("invalid response: {e}"))
}

// ---------- provider implementations ----------

fn shortcut_headers_url(path: &str) -> String {
    format!("{SHORTCUT_API}{path}")
}

/// Shortcut: validate via /member, cards via search/stories (query required).
fn shortcut_validate(tok: &str) -> Result<String, String> {
    let headers: [(&str, String); 1] = [("Shortcut-Token", tok.to_string())];
    let v = curl_json(&shortcut_headers_url("/member"), &headers, None)?;
    if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
        return Err(msg.to_string());
    }
    v.get("name")
        .and_then(|n| n.as_str())
        .map(String::from)
        .ok_or_else(|| "unexpected Shortcut response".into())
}

fn shortcut_fetch_cards(tok: &str, query: &str) -> Result<Vec<CardInfo>, String> {
    let headers: [(&str, String); 1] = [("Shortcut-Token", tok.to_string())];
    // search/stories REQUIRES a query; prepend the user's search text.
    let q = if query.trim().is_empty() {
        "is:story".to_string()
    } else {
        format!("{} is:story", query.trim())
    };
    let encoded = url_encode(&q);
    let v = curl_json(
        &shortcut_headers_url(&format!(
            "/search/stories?query={encoded}&detail=slim&page_size=25"
        )),
        &headers,
        None,
    )?;
    let Some(data) = v.get("data").and_then(|d| d.as_array()) else {
        return Err("unexpected Shortcut response".into());
    };
    Ok(data
        .iter()
        .filter_map(|s| {
            let id = s.get("id")?.as_i64()?;
            let title = s.get("name")?.as_str()?.to_string();
            let url = s
                .get("app_url")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();
            // slim results carry state flags, not state names
            let state = if s.get("archived").and_then(|a| a.as_bool()) == Some(true) {
                "Archived"
            } else if s.get("completed").and_then(|c| c.as_bool()) == Some(true) {
                "Completed"
            } else if s.get("started").and_then(|st| st.as_bool()) == Some(true) {
                "Started"
            } else {
                "Unstarted"
            }
            .to_string();
            Some(CardInfo {
                id: format!("sc-{id}"),
                title,
                state,
                url,
            })
        })
        .collect())
}

/// Linear: GraphQL viewer query validates the token.
fn linear_validate(tok: &str) -> Result<String, String> {
    let body = r#"{"query":"{ viewer { name email } }"}"#;
    let headers: [(&str, String); 1] = [("Authorization", format!("Bearer {tok}"))];
    let v = curl_json(LINEAR_API, &headers, Some(body))?;
    if let Some(errors) = v.get("errors").and_then(|e| e.as_array()) {
        if !errors.is_empty() {
            return Err(
                errors[0]
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("authentication failed")
                    .to_string(),
            );
        }
    }
    v.pointer("/data/viewer/name")
        .and_then(|n| n.as_str())
        .map(String::from)
        .ok_or_else(|| "unexpected Linear response".into())
}

fn linear_fetch_cards(tok: &str, query: &str) -> Result<Vec<CardInfo>, String> {
    let filter = if query.trim().is_empty() {
        String::from("")
    } else {
        format!(
            r#", filter: {{ or: [{{ title: {{ contains: "{}" }} }}, {{ description: {{ contains: "{}" }} }}] }}"#,
            escape_gql(query.trim()),
            escape_gql(query.trim())
        )
    };
    let body = format!(
        r#"{{"query":"{{ issues(first: 25, orderBy: updatedAt{filter}) {{ nodes {{ identifier title url state {{ name }} }} }} }} }}"#
    );
    let headers: [(&str, String); 1] = [("Authorization", format!("Bearer {tok}"))];
    let v = curl_json(LINEAR_API, &headers, Some(&body))?;
    if let Some(errors) = v.get("errors").and_then(|e| e.as_array()) {
        if !errors.is_empty() {
            return Err(
                errors[0]
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("authentication failed")
                    .to_string(),
            );
        }
    }
    let nodes = v
        .pointer("/data/issues/nodes")
        .and_then(|n| n.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(nodes
        .iter()
        .filter_map(|i| {
            Some(CardInfo {
                id: i.get("identifier")?.as_str()?.to_string(),
                title: i.get("title")?.as_str()?.to_string(),
                state: i
                    .pointer("/state/name")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                url: i.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string(),
            })
        })
        .collect())
}

/// Figma: personal access token; validate via GET /v1/me.
fn figma_validate(tok: &str) -> Result<String, String> {
    let headers: [(&str, String); 1] = [("X-Figma-Token", tok.to_string())];
    let v = curl_json("https://api.figma.com/v1/me", &headers, None)?;
    if let Some(err) = v.get("err").and_then(|e| e.as_str()) {
        return Err(err.to_string());
    }
    v.get("handle")
        .or_else(|| v.get("email"))
        .and_then(|n| n.as_str())
        .map(String::from)
        .ok_or_else(|| "unexpected Figma response".into())
}

// ---------- public API ----------

pub fn validate(kind: TrackerKind, tok: &str) -> Result<String, String> {
    match kind {
        TrackerKind::Shortcut => shortcut_validate(tok),
        TrackerKind::Linear => linear_validate(tok),
        TrackerKind::Figma => figma_validate(tok),
    }
}

pub fn save_token(kind: TrackerKind, tok: &str) -> Result<(), String> {
    let mut creds = load_credentials();
    creds.insert(kind.token_key().to_string(), tok.trim().to_string());
    save_credentials(&creds)
}

pub fn remove_token(kind: TrackerKind) -> Result<(), String> {
    let mut creds = load_credentials();
    creds.remove(kind.token_key());
    save_credentials(&creds)
}

pub fn status(kind: TrackerKind) -> IntegrationStatus {
    let connected = token(kind).is_some();
    IntegrationStatus {
        kind: format!("{kind:?}").to_lowercase(),
        connected,
        account: None,
    }
}

pub fn fetch_cards(kind: TrackerKind, query: &str) -> Result<Vec<CardInfo>, String> {
    let tok = token(kind).ok_or_else(|| format!("{} is not connected", status_kind_label(kind)))?;
    match kind {
        TrackerKind::Shortcut => shortcut_fetch_cards(&tok, query),
        TrackerKind::Linear => linear_fetch_cards(&tok, query),
        // Figma is connection-only for now (design previews come later);
        // card fetching isn't applicable.
        TrackerKind::Figma => Ok(vec![]),
    }
}

fn status_kind_label(kind: TrackerKind) -> &'static str {
    match kind {
        TrackerKind::Shortcut => "Shortcut",
        TrackerKind::Linear => "Linear",
        TrackerKind::Figma => "Figma",
    }
}

// Minimal URL/GQL escaping for user search text (no new deps).
fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn escape_gql(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}