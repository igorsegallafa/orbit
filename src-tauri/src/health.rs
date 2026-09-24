// Environment checks: the CLIs Orbit itself relies on, the configured AI
// agent, and whatever tools the user's own build commands call.
use crate::config::Config;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    /// "orbit" | "agent" | "build"
    pub group: String,
    /// Orbit cannot work without it.
    pub required: bool,
    pub ok: bool,
    /// Version line, or what's wrong and how to fix it.
    pub detail: String,
    /// Repos whose build command uses this tool (build group only).
    pub used_by: Vec<String>,
}

fn check(name: &str, group: &str, required: bool, result: Result<String, String>, hint: &str) -> Check {
    let (ok, detail) = match result {
        Ok(out) => (true, out.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("installed").to_string()),
        Err(_) => (false, hint.to_string()),
    };
    Check { name: name.into(), group: group.into(), required, ok, detail, used_by: vec![] }
}

fn version(bin: &str) -> Result<String, String> {
    if !crate::proc::exists(bin) {
        return Err("not found".into());
    }
    crate::proc::run(bin, &["--version"], None).or_else(|_| Ok("installed".into()))
}

/// Programs a shell command line starts: the first word of each `&&`,
/// `||`, `;` or `|` segment, skipping env assignments and shell builtins.
pub fn programs_in(cmdline: &str) -> Vec<String> {
    const BUILTINS: &[&str] = &["cd", "set", "export", "echo", "call", "exit", "true", "false", "if", "then", "fi"];
    let mut out: Vec<String> = Vec::new();
    for seg in cmdline.split(['&', '|', ';']) {
        let first = seg
            .split_whitespace()
            .find(|w| !w.contains('=') || w.starts_with('-'))
            .unwrap_or("")
            .trim_matches(|c| c == '"' || c == '\'' || c == '(' || c == ')');
        if first.is_empty() || first.starts_with('-') || BUILTINS.contains(&first.to_lowercase().as_str()) {
            continue;
        }
        // Paths (./gradlew, scripts\build.bat) run a repo file, not a tool on PATH.
        if first.contains('/') || first.contains('\\') {
            continue;
        }
        if !out.iter().any(|p| p == first) {
            out.push(first.to_string());
        }
    }
    out
}

pub fn run() -> Vec<Check> {
    let cfg = Config::load().unwrap_or_default();
    let gh_auth = crate::proc::run("gh", &["auth", "status"], None).map(|_| "logged in".to_string());
    let helper = crate::proc::run("git", &["config", "--get-regexp", "^credential\\..*helper$"], None)
        .ok()
        .filter(|o| o.contains("gh auth git-credential"))
        .map(|_| "git push uses your gh login".to_string())
        .ok_or_else(|| "missing".to_string());

    let mut checks = vec![
        check("git", "orbit", true, version("git"), "Install Git: https://git-scm.com"),
        check("gh", "orbit", true, version("gh"), "Install the GitHub CLI: https://cli.github.com"),
        check("gh auth", "orbit", true, gh_auth, "Run `gh auth login`"),
        check("git credential helper", "orbit", false, helper, "Optional: run `gh auth setup-git` so git uses your gh login"),
        check("curl", "orbit", false, version("curl"), "Needed for the Linear, Shortcut and Figma integrations"),
    ];

    let agent = cfg.ai.agent_bin();
    checks.push(check(
        &agent,
        "agent",
        false,
        version(&agent),
        "The agent selected in Settings → AI is not installed; AI features and Ralph won't run",
    ));

    let mut tools: Vec<(String, Vec<String>)> = Vec::new();
    for svc in &cfg.services {
        let Some(cmd) = svc.build.as_ref().and_then(|b| b.for_current_os()) else { continue };
        for prog in programs_in(cmd) {
            match tools.iter_mut().find(|(p, _)| *p == prog) {
                Some((_, repos)) => repos.push(svc.name.clone()),
                None => tools.push((prog, vec![svc.name.clone()])),
            }
        }
    }
    for (prog, used_by) in tools {
        let mut c = check(&prog, "build", false, version(&prog), "Not found on PATH; builds using it will fail");
        c.used_by = used_by;
        checks.push(c);
    }

    // Language servers for the languages the cloned repos use.
    let mut langs: Vec<(&'static str, Vec<String>)> = Vec::new();
    for svc in &cfg.services {
        let Ok(dir) = crate::workspace::clone_dir(svc) else { continue };
        for lang in crate::lsp::languages_in(&dir) {
            match langs.iter_mut().find(|(l, _)| *l == lang) {
                Some((_, repos)) => repos.push(svc.name.clone()),
                None => langs.push((lang, vec![svc.name.clone()])),
            }
        }
    }
    for (id, used_by) in langs {
        let Some(lang) = crate::lsp::language(id) else { continue };
        let setting = cfg.language_servers.get(id);
        let result = match crate::lsp::resolve(lang, setting) {
            Some((bin, _)) => Ok(format!("{} · {bin}", lang.name)),
            None if setting.is_some_and(|s| s.disabled) => Ok(format!("{} · disabled in Settings → Languages", lang.name)),
            None => Err("missing".to_string()),
        };
        let hint = format!("{}: no language server, so no code navigation. {}", lang.name, lang.servers[0].install);
        let mut c = check(lang.servers[0].bin, "language", false, result, &hint);
        c.used_by = used_by;
        checks.push(c);
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::programs_in;

    #[test]
    fn finds_the_tools_a_build_command_runs() {
        assert_eq!(programs_in("pnpm install --frozen-lockfile && pnpm run build"), ["pnpm"]);
        assert_eq!(programs_in("dotnet restore && dotnet build Machina.sln"), ["dotnet"]);
        assert_eq!(programs_in("cd app; CI=1 npm ci | tee log && cargo build"), ["npm", "tee", "cargo"]);
        assert!(programs_in("./gradlew build").is_empty());
        assert_eq!(programs_in("cmake --preset windows-x64-vs"), ["cmake"]);
    }
}
