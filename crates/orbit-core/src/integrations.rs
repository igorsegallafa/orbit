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
    /// Tracker-suggested git branch (Linear's `branchName`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// Full card content (used by the Plan flow).
#[derive(Debug, Serialize, Clone)]
pub struct CardDetail {
    pub id: String,
    pub title: String,
    pub description: String,
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
    let dir = crate::config::home_dir()?.join(".config").join("orbit");
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
    let mut cmd = crate::proc::cmd("curl");
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
                branch: None,
            })
        })
        .collect())
}

/// Linear personal API keys go in the header raw; OAuth tokens need Bearer.
fn linear_auth(tok: &str) -> String {
    let tok = tok.trim();
    if tok.starts_with("lin_api_") {
        tok.to_string()
    } else {
        format!("Bearer {tok}")
    }
}

/// POSTs a GraphQL query (with variables) and returns `data`, surfacing
/// the first GraphQL error as the message.
fn linear_request(tok: &str, query: &str, variables: serde_json::Value) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({ "query": query, "variables": variables }).to_string();
    let headers: [(&str, String); 1] = [("Authorization", linear_auth(tok))];
    let v = curl_json(LINEAR_API, &headers, Some(&body))?;
    if let Some(err) = v.pointer("/errors/0") {
        return Err(err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Linear request failed")
            .to_string());
    }
    v.get("data").cloned().ok_or_else(|| "unexpected Linear response".into())
}

/// Linear: GraphQL viewer query validates the token.
fn linear_validate(tok: &str) -> Result<String, String> {
    let data = linear_request(tok, "{ viewer { name email } }", serde_json::json!({}))?;
    data.pointer("/viewer/name")
        .and_then(|n| n.as_str())
        .map(String::from)
        .ok_or_else(|| "unexpected Linear response".into())
}

fn linear_fetch_cards(tok: &str, query: &str) -> Result<Vec<CardInfo>, String> {
    let q = query.trim();
    let filter = if q.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!({ "or": [
            { "title": { "containsIgnoreCase": q } },
            { "description": { "containsIgnoreCase": q } }
        ]})
    };
    let data = linear_request(
        tok,
        "query($filter: IssueFilter) { issues(first: 25, orderBy: updatedAt, filter: $filter) { nodes { identifier title url branchName state { name } } } }",
        serde_json::json!({ "filter": filter }),
    )?;
    let nodes = data
        .pointer("/issues/nodes")
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
                branch: i.get("branchName").and_then(|b| b.as_str()).filter(|b| !b.is_empty()).map(str::to_string),
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

/// Fetches a single card's full content (description included).
/// Shortcut: GET /stories/<num>; Linear: GraphQL issue by identifier.
pub fn fetch_card(kind: TrackerKind, id: &str) -> Result<CardDetail, String> {
    let tok = token(kind).ok_or_else(|| format!("{} is not connected", status_kind_label(kind)))?;
    match kind {
        TrackerKind::Shortcut => shortcut_fetch_card(&tok, id),
        TrackerKind::Linear => linear_fetch_card(&tok, id),
        TrackerKind::Figma => Err("Figma has no cards".into()),
    }
}

fn shortcut_fetch_card(tok: &str, id: &str) -> Result<CardDetail, String> {
    let num = id
        .strip_prefix("sc-")
        .ok_or_else(|| format!("invalid Shortcut id: {id}"))?;
    let headers: [(&str, String); 1] = [("Shortcut-Token", tok.to_string())];
    let v = curl_json(&shortcut_headers_url(&format!("/stories/{num}")), &headers, None)?;
    if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
        return Err(msg.to_string());
    }
    Ok(CardDetail {
        id: id.to_string(),
        title: v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
        description: v
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string(),
        state: v
            .get("workflow_state_type")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        url: v
            .get("app_url")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn linear_fetch_card(tok: &str, id: &str) -> Result<CardDetail, String> {
    // Current Linear API accepts the human identifier (ENG-123) in issue(id:).
    let v = linear_request(
        tok,
        "query($id: String!) { issue(id: $id) { identifier title description url state { name } } }",
        serde_json::json!({ "id": id }),
    )?;
    let issue = v
        .get("issue")
        .ok_or("card not found in Linear")?;
    if issue.is_null() {
        return Err("card not found in Linear".into());
    }
    Ok(CardDetail {
        id: issue
            .get("identifier")
            .and_then(|i| i.as_str())
            .unwrap_or(id)
            .to_string(),
        title: issue.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string(),
        description: issue
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string(),
        state: issue
            .pointer("/state/name")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        url: issue.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string(),
    })
}

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

// Minimal URL escaping for user search text (no new deps).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_api_keys_are_sent_raw_and_oauth_tokens_as_bearer() {
        assert_eq!(linear_auth("lin_api_abc"), "lin_api_abc");
        assert_eq!(linear_auth(" lin_api_abc
"), "lin_api_abc");
        assert_eq!(linear_auth("oauth-token"), "Bearer oauth-token");
    }
}
