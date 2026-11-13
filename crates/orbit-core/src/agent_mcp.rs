// `<orbit exe> --orbit-mcp <hook url>`: the stdio MCP server Orbit adds to
// every agent session (`agent_hooks::session_launch`). Its `show_image` tool
// posts the picture's path to the session's hook route, and Orbit shows it
// beside the terminal. The picture never reaches the model. Its `permission`
// tool is chat mode's `--permission-prompt-tool`: it asks Orbit and waits.
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const IMAGE_EXT: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];

pub fn serve(url: Option<String>) {
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(reply) = handle(&msg, url.as_deref()) {
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
    }
}

fn tools() -> Value {
    json!([show_image_tool(), {
        "name": "permission",
        "description": "Claude Code's permission prompt in Orbit's chat. Not for direct use.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "tool_name": { "type": "string" },
                "input": { "type": "object" },
                "tool_use_id": { "type": "string" }
            },
            "required": ["tool_name", "input"]
        }
    }])
}

fn show_image_tool() -> Value {
    json!({
        "name": "show_image",
        "description": "Show an image file to the user in Orbit, in a viewer beside this session. \
            Use it for screenshots, charts or rendered UI you made or changed, instead of asking the user to open the file. \
            The image is not sent back to you. PNG, JPEG, GIF, WebP, BMP or SVG.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The image file, absolute or relative to the working folder" },
                "title": { "type": "string", "description": "A short caption" }
            },
            "required": ["path"]
        }
    })
}

/// The JSON-RPC reply to one message; none for notifications.
fn handle(msg: &Value, url: Option<&str>) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let params = &msg["params"];
    let result = match msg["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "orbit", "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" if params["name"] == "permission" => {
            json!({ "content": [{ "type": "text", "text": permission(&params["arguments"], url) }] })
        }
        "tools/call" if params["name"] == "show_image" => {
            let (text, is_error) = match show(&params["arguments"], url) {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
        }
        m => return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("unknown method {m}") } })),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn show(args: &Value, url: Option<&str>) -> Result<String, String> {
    let raw = args["path"].as_str().ok_or("path is required")?;
    let path = std::fs::canonicalize(raw).map_err(|e| format!("{raw}: {e}"))?;
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !path.is_file() || !IMAGE_EXT.contains(&ext.as_str()) {
        return Err(format!("not an image file: {}", path.display()));
    }
    let url = url.ok_or("this session was not started by Orbit")?;
    let body = json!({ "hook_event_name": "ShowImage", "path": path, "message": args["title"].as_str() }).to_string();
    post(url, &body, Duration::from_secs(5)).map_err(|e| format!("Orbit did not answer: {e}"))?;
    Ok(format!("Shown to the user in Orbit: {}", path.display()))
}

/// The `--permission-prompt-tool` decision JSON, once Orbit's user picks.
pub(crate) fn permission(args: &Value, url: Option<&str>) -> String {
    let body = json!({ "hook_event_name": "PermissionRequest", "tool_name": args["tool_name"], "input": args["input"] }).to_string();
    // Orbit denies after 10 minutes; wait a little longer than that.
    let answer = url.map(|u| post(u, &body, Duration::from_secs(660)));
    match answer {
        Some(Ok(a)) if a.trim() == "allow" => json!({ "behavior": "allow", "updatedInput": args["input"] }),
        // AskUserQuestion's picks: Claude reads them from the input it gets back.
        Some(Ok(a)) if a.trim_start().starts_with('{') => {
            let mut input = args["input"].clone();
            let answers = serde_json::from_str::<Value>(&a).map(|v| v["answers"].clone()).unwrap_or_default();
            input["answers"] = answers;
            json!({ "behavior": "allow", "updatedInput": input })
        }
        Some(Err(e)) => json!({ "behavior": "deny", "message": format!("Orbit did not answer: {e}") }),
        _ => json!({ "behavior": "deny", "message": "The user denied this in Orbit." }),
    }
    .to_string()
}

/// One POST to Orbit's loopback listener (`agent_hooks`), answered 200; returns the body.
pub(crate) fn post(url: &str, body: &str, timeout: Duration) -> std::io::Result<String> {
    let bad = || std::io::Error::other(format!("bad url {url}"));
    let (host, path) = url.strip_prefix("http://").and_then(|r| r.split_once('/')).ok_or_else(bad)?;
    let mut s = TcpStream::connect(host)?;
    s.set_read_timeout(Some(timeout))?;
    write!(s, "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    let mut resp = String::new();
    s.read_to_string(&mut resp)?;
    if resp.split_whitespace().nth(1) != Some("200") {
        return Err(std::io::Error::other("no 200"));
    }
    Ok(resp.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_the_mcp_handshake_and_tool_calls() {
        let init = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}), None).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}), None).is_none());
        let list = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}), None).unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], "show_image");
        assert_eq!(list["result"]["tools"][1]["name"], "permission");
        let perm = handle(&json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"permission","arguments":{"tool_name":"Bash","input":{}}}}), None).unwrap();
        assert!(perm["result"]["content"][0]["text"].as_str().unwrap().contains(r#""behavior":"deny""#));
        let call = |path: &str| handle(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"show_image","arguments":{"path":path}}}), None).unwrap();
        assert_eq!(call("/definitely/missing.png")["result"]["isError"], true);
        assert_eq!(call("Cargo.toml")["result"]["isError"], true);
        assert!(handle(&json!({"jsonrpc":"2.0","id":4,"method":"nope"}), None).unwrap()["error"].is_object());
    }
}
