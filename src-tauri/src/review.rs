// Pull request code review: threads (GraphQL, with resolved/outdated
// state), commentable diff lines (from each file's patch), single comments,
// replies, edits, thread resolution and review submission, all through the
// `gh` login.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

/// `gh api <endpoint>` with an optional JSON body (sent via a temp file, so
/// bodies of any size and content pass through intact).
fn gh_json(method: &str, endpoint: &str, body: Option<&Value>) -> Result<Value, String> {
    let mut args: Vec<String> = vec!["api".into(), "-X".into(), method.into(), endpoint.into()];
    let tmp = match body {
        Some(b) => {
            let p = std::env::temp_dir().join(format!(
                "orbit-gh-{}-{}.json",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::write(&p, b.to_string()).map_err(|e| e.to_string())?;
            args.push("--input".into());
            args.push(p.to_string_lossy().into());
            Some(p)
        }
        None => None,
    };
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = crate::proc::run("gh", &refs, None);
    if let Some(p) = tmp {
        let _ = std::fs::remove_file(p);
    }
    let out = out.map_err(clean_gh_error)?;
    if out.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&out).map_err(|e| format!("unexpected GitHub response: {e}"))
}

/// gh prints "gh: <message> (HTTP 422)" plus JSON; keep the readable part.
fn clean_gh_error(e: String) -> String {
    let first = e.lines().find(|l| !l.trim().is_empty()).unwrap_or(&e).trim();
    first.trim_start_matches("gh: ").to_string()
}

fn graphql(query: &str, variables: Value) -> Result<Value, String> {
    let v = gh_json("POST", "graphql", Some(&json!({ "query": query, "variables": variables })))?;
    if let Some(err) = v.pointer("/errors/0/message").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    Ok(v.get("data").cloned().unwrap_or(Value::Null))
}

fn split_owner_repo(owner_repo: &str) -> Result<(&str, &str), String> {
    owner_repo
        .split_once('/')
        .ok_or_else(|| format!("invalid repository: {owner_repo}"))
}

// ---------- read ----------

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReviewComment {
    /// REST id (replies, edits, deletes use it).
    pub id: u64,
    pub node_id: String,
    pub author: String,
    pub avatar_url: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
    /// "PENDING" for the viewer's unsubmitted review started elsewhere.
    pub state: String,
    pub is_mine: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub path: String,
    /// "LEFT" (base) or "RIGHT" (head).
    pub side: String,
    /// Current line in the diff; None when outdated and no longer mapped.
    pub line: Option<u64>,
    pub start_line: Option<u64>,
    pub original_line: Option<u64>,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub comments: Vec<ReviewComment>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSummary {
    pub author: String,
    pub avatar_url: String,
    /// APPROVED | CHANGES_REQUESTED | COMMENTED | DISMISSED | PENDING
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

/// Lines of a file that GitHub accepts comments on (inside diff hunks).
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Commentable {
    /// Inclusive [start, end] line ranges on the head side.
    pub right: Vec<[u64; 2]>,
    /// Inclusive [start, end] line ranges on the base side.
    pub left: Vec<[u64; 2]>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReviewData {
    pub viewer: String,
    pub pr_author: String,
    /// APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED | null
    pub review_decision: Option<String>,
    pub threads: Vec<ReviewThread>,
    pub reviews: Vec<ReviewSummary>,
    pub commentable: HashMap<String, Commentable>,
}

const THREADS_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      author { login }
      reviewDecision
      reviews(last: 50) {
        nodes { state body submittedAt author { login avatarUrl } }
      }
      reviewThreads(first: 100) {
        nodes {
          id isResolved isOutdated path line startLine originalLine diffSide
          comments(first: 100) {
            nodes { id databaseId body createdAt url state author { login avatarUrl } }
          }
        }
      }
    }
  }
}"#;

fn s(v: &Value, ptr: &str) -> String {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Threads and reviews; the commentable lines (one REST call per 100 files)
/// only when `with_lines`, since they don't change between comment actions.
pub fn review_data(owner_repo: &str, number: u64, with_lines: bool) -> Result<ReviewData, String> {
    let (owner, name) = split_owner_repo(owner_repo)?;
    let data = graphql(THREADS_QUERY, json!({ "owner": owner, "name": name, "number": number }))?;
    let viewer = s(&data, "/viewer/login");
    let pr = data
        .pointer("/repository/pullRequest")
        .ok_or("pull request not found")?;

    let threads = pr
        .pointer("/reviewThreads/nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .map(|t| ReviewThread {
                    id: s(t, "/id"),
                    path: s(t, "/path"),
                    side: t.get("diffSide").and_then(Value::as_str).unwrap_or("RIGHT").to_string(),
                    line: t.get("line").and_then(Value::as_u64),
                    start_line: t.get("startLine").and_then(Value::as_u64),
                    original_line: t.get("originalLine").and_then(Value::as_u64),
                    is_resolved: t.get("isResolved").and_then(Value::as_bool).unwrap_or(false),
                    is_outdated: t.get("isOutdated").and_then(Value::as_bool).unwrap_or(false),
                    comments: t
                        .pointer("/comments/nodes")
                        .and_then(Value::as_array)
                        .map(|cs| {
                            cs.iter()
                                .map(|c| {
                                    let author = s(c, "/author/login");
                                    ReviewComment {
                                        id: c.get("databaseId").and_then(Value::as_u64).unwrap_or(0),
                                        node_id: s(c, "/id"),
                                        is_mine: !author.is_empty() && author == viewer,
                                        author,
                                        avatar_url: s(c, "/author/avatarUrl"),
                                        body: s(c, "/body"),
                                        created_at: s(c, "/createdAt"),
                                        url: s(c, "/url"),
                                        state: s(c, "/state"),
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();

    let reviews = pr
        .pointer("/reviews/nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .map(|r| ReviewSummary {
                    author: s(r, "/author/login"),
                    avatar_url: s(r, "/author/avatarUrl"),
                    state: s(r, "/state"),
                    body: s(r, "/body"),
                    submitted_at: r.get("submittedAt").and_then(Value::as_str).map(String::from),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(ReviewData {
        viewer,
        pr_author: s(pr, "/author/login"),
        review_decision: pr.get("reviewDecision").and_then(Value::as_str).map(String::from),
        threads,
        reviews,
        commentable: if with_lines { commentable_lines(owner_repo, number)? } else { HashMap::new() },
    })
}

/// Commentable line ranges per file, from the PR files' patches.
fn commentable_lines(owner_repo: &str, number: u64) -> Result<HashMap<String, Commentable>, String> {
    let mut out = HashMap::new();
    for page in 1..=10 {
        let v = gh_json("GET", &format!("repos/{owner_repo}/pulls/{number}/files?per_page=100&page={page}"), None)?;
        let files = v.as_array().cloned().unwrap_or_default();
        for f in &files {
            let path = s(f, "/filename");
            let patch = f.get("patch").and_then(Value::as_str).unwrap_or("");
            out.insert(path, parse_hunks(patch));
        }
        if files.len() < 100 {
            break;
        }
    }
    Ok(out)
}

/// Hunk headers `@@ -a,b +c,d @@` -> inclusive line ranges on each side.
pub fn parse_hunks(patch: &str) -> Commentable {
    let mut c = Commentable::default();
    for line in patch.lines().filter(|l| l.starts_with("@@")) {
        let mut parts = line.split_whitespace().skip(1);
        let (Some(old), Some(new)) = (parts.next(), parts.next()) else { continue };
        let range = |spec: &str| -> Option<[u64; 2]> {
            let spec = spec.trim_start_matches(['-', '+']);
            let (start, len) = match spec.split_once(',') {
                Some((s, l)) => (s.parse::<u64>().ok()?, l.parse::<u64>().ok()?),
                None => (spec.parse::<u64>().ok()?, 1),
            };
            (len > 0).then(|| [start, start + len - 1])
        };
        if let Some(r) = range(old) {
            c.left.push(r);
        }
        if let Some(r) = range(new) {
            c.right.push(r);
        }
    }
    c
}

// ---------- write ----------

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DraftComment {
    pub path: String,
    pub side: String,
    pub line: u64,
    #[serde(default)]
    pub start_line: Option<u64>,
    pub body: String,
}

fn comment_position(c: &DraftComment) -> Value {
    let mut v = json!({ "path": c.path, "side": c.side, "line": c.line, "body": c.body });
    if let Some(start) = c.start_line.filter(|s| *s < c.line) {
        v["start_line"] = json!(start);
        v["start_side"] = json!(c.side);
    }
    v
}

/// Posts one comment right away (outside a review).
pub fn add_comment(owner_repo: &str, number: u64, commit_id: &str, c: &DraftComment) -> Result<(), String> {
    let mut body = comment_position(c);
    body["commit_id"] = json!(commit_id);
    gh_json("POST", &format!("repos/{owner_repo}/pulls/{number}/comments"), Some(&body)).map(|_| ())
}

pub fn reply(owner_repo: &str, number: u64, comment_id: u64, body: &str) -> Result<(), String> {
    gh_json(
        "POST",
        &format!("repos/{owner_repo}/pulls/{number}/comments/{comment_id}/replies"),
        Some(&json!({ "body": body })),
    )
    .map(|_| ())
}

pub fn edit_comment(owner_repo: &str, comment_id: u64, body: &str) -> Result<(), String> {
    gh_json(
        "PATCH",
        &format!("repos/{owner_repo}/pulls/comments/{comment_id}"),
        Some(&json!({ "body": body })),
    )
    .map(|_| ())
}

pub fn delete_comment(owner_repo: &str, comment_id: u64) -> Result<(), String> {
    gh_json("DELETE", &format!("repos/{owner_repo}/pulls/comments/{comment_id}"), None).map(|_| ())
}

pub fn set_thread_resolved(thread_id: &str, resolved: bool) -> Result<(), String> {
    let mutation = if resolved {
        "mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { thread { id } } }"
    } else {
        "mutation($id: ID!) { unresolveReviewThread(input: {threadId: $id}) { thread { id } } }"
    };
    graphql(mutation, json!({ "id": thread_id })).map(|_| ())
}

/// Submits a review with its pending comments in one call.
/// `event`: APPROVE | REQUEST_CHANGES | COMMENT.
pub fn submit_review(
    owner_repo: &str,
    number: u64,
    commit_id: &str,
    event: &str,
    body: &str,
    comments: &[DraftComment],
) -> Result<(), String> {
    if !["APPROVE", "REQUEST_CHANGES", "COMMENT"].contains(&event) {
        return Err(format!("unknown review event: {event}"));
    }
    if event != "APPROVE" && body.trim().is_empty() && comments.is_empty() {
        return Err("write a summary or add at least one comment".into());
    }
    let payload = json!({
        "commit_id": commit_id,
        "event": event,
        "body": body,
        "comments": comments.iter().map(comment_position).collect::<Vec<_>>(),
    });
    gh_json("POST", &format!("repos/{owner_repo}/pulls/{number}/reviews"), Some(&payload)).map(|_| ())
}

// ---------- commands ----------

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

#[tauri::command]
pub async fn pr_review_data(owner_repo: String, number: u64, with_lines: bool) -> Result<ReviewData, String> {
    blocking(move || review_data(&owner_repo, number, with_lines)).await
}

#[tauri::command]
pub async fn pr_add_comment(owner_repo: String, number: u64, commit_id: String, comment: DraftComment) -> Result<(), String> {
    blocking(move || add_comment(&owner_repo, number, &commit_id, &comment)).await
}

#[tauri::command]
pub async fn pr_reply(owner_repo: String, number: u64, comment_id: u64, body: String) -> Result<(), String> {
    blocking(move || reply(&owner_repo, number, comment_id, &body)).await
}

#[tauri::command]
pub async fn pr_edit_comment(owner_repo: String, comment_id: u64, body: String) -> Result<(), String> {
    blocking(move || edit_comment(&owner_repo, comment_id, &body)).await
}

#[tauri::command]
pub async fn pr_delete_comment(owner_repo: String, comment_id: u64) -> Result<(), String> {
    blocking(move || delete_comment(&owner_repo, comment_id)).await
}

#[tauri::command]
pub async fn pr_resolve_thread(thread_id: String, resolved: bool) -> Result<(), String> {
    blocking(move || set_thread_resolved(&thread_id, resolved)).await
}

#[tauri::command]
pub async fn pr_submit_review(
    owner_repo: String,
    number: u64,
    commit_id: String,
    event: String,
    body: String,
    comments: Vec<DraftComment>,
) -> Result<(), String> {
    blocking(move || submit_review(&owner_repo, number, &commit_id, &event, &body, &comments)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hunk_ranges_on_both_sides() {
        let patch = "@@ -1,3 +1,4 @@\n a\n+b\n c\n d\n@@ -10 +11,0 @@ fn x()\n-gone\n@@ -20,2 +21,3 @@\n";
        let c = parse_hunks(patch);
        assert_eq!(c.right, vec![[1, 4], [21, 23]]);
        assert_eq!(c.left, vec![[1, 3], [10, 10], [20, 21]]);
    }

    #[test]
    fn multi_line_comments_send_start_line_only_when_before_line() {
        let single = DraftComment { path: "a.rs".into(), side: "RIGHT".into(), line: 5, start_line: Some(5), body: "x".into() };
        assert!(comment_position(&single).get("start_line").is_none());
        let range = DraftComment { start_line: Some(2), ..single };
        let v = comment_position(&range);
        assert_eq!(v["start_line"], 2);
        assert_eq!(v["start_side"], "RIGHT");
    }

    #[test]
    fn review_needs_content_unless_approving() {
        assert!(submit_review("o/r", 1, "sha", "COMMENT", " ", &[]).is_err());
        assert!(submit_review("o/r", 1, "sha", "NOPE", "x", &[]).is_err());
    }

    #[test]
    fn gh_errors_keep_the_readable_line() {
        assert_eq!(
            clean_gh_error("gh: Validation Failed (HTTP 422)\n{\"message\":\"x\"}".into()),
            "Validation Failed (HTTP 422)"
        );
    }
}
