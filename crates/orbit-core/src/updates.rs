//! Updates: the release manifest (`latest.json`, the Tauri updater's format),
//! version comparison, and installing a signed release over the running app.
//!
//! Install flow: [`Target::detect`] → [`install`] (download, minisign check,
//! unpack, atomic replace) → [`relaunch`].
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// The release manifest. `ORBIT_UPDATE_URL` overrides it (any URL curl reads).
pub const LATEST_URL: &str = "https://github.com/igorsegallafa/orbit/releases/latest/download/latest.json";

/// The minisign public key releases are signed with (base64 of the `.pub`
/// file; the Tauri app's `plugins.updater.pubkey`). `ORBIT_UPDATE_PUBKEY`
/// overrides it, for tests.
pub const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDMyMTcyQTlEQzQ3NjRDQUUKUldTdVRIYkVuU29YTWxDbU9vbHpyakI4dVh1dGE0L3hNV1ZKRjB1Y3dSWi9HUTVCajR0VVM3NDYK";

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Asset {
    pub url: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: Option<String>,
    /// `<os>-<arch>[-<kind>]` → download.
    pub platforms: HashMap<String, Asset>,
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
    #[derive(Deserialize)]
    struct Raw {
        version: Option<String>,
        notes: Option<String>,
        #[serde(default)]
        platforms: HashMap<String, Asset>,
    }
    let v: Raw = serde_json::from_str(raw).map_err(|e| format!("invalid latest.json: {e}"))?;
    let version = v.version.ok_or("latest.json has no version")?.trim().trim_start_matches('v').to_string();
    let notes = v.notes.filter(|n| !n.trim().is_empty());
    Ok(newer(&version, current).then_some(Release { version, notes, platforms: v.platforms }))
}

/// Whether `a` is a later version than `b` (numeric parts; a pre-release
/// sorts before its release).
pub fn newer(a: &str, b: &str) -> bool {
    key(a) > key(b)
}

fn same(a: &str, b: &str) -> bool {
    key(a) == key(b)
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

// ---------- what to replace ----------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Linux: the `.AppImage` file (`$APPIMAGE`).
    AppImage,
    /// The executable itself: a tarball install on Linux, `Orbit.exe` on Windows.
    Binary,
    /// macOS: the `.app` bundle.
    Bundle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub kind: Kind,
    pub path: PathBuf,
}

const OS: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_os = "macos") {
    "darwin"
} else {
    "linux"
};

impl Target {
    /// The running app's install, or why it can't update itself.
    pub fn detect() -> Result<Target, String> {
        let exe = std::env::current_exe().map_err(|e| format!("can't find the running executable: {e}"))?;
        detect_from(OS, std::env::var_os("APPIMAGE"), &exe)
    }

    /// The manifest's key for this install.
    pub fn platform_key(&self) -> String {
        platform_key(OS, std::env::consts::ARCH, self.kind)
    }
}

fn platform_key(os: &str, arch: &str, kind: Kind) -> String {
    let suffix = match kind {
        Kind::AppImage => "appimage",
        Kind::Binary if os == "windows" => "zip",
        Kind::Binary => "tar",
        Kind::Bundle => "app",
    };
    format!("{os}-{arch}-{suffix}")
}

fn detect_from(os: &str, appimage: Option<OsString>, exe: &Path) -> Result<Target, String> {
    if os == "linux" {
        if let Some(p) = appimage.filter(|p| !p.is_empty()) {
            return Ok(Target { kind: Kind::AppImage, path: PathBuf::from(p) });
        }
    }
    let parents: Vec<String> = exe.ancestors().skip(1).take(3).map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
    if parents.len() >= 2 && matches!(parents[0].as_str(), "debug" | "release") && (parents[1] == "target" || parents.get(2).is_some_and(|p| p == "target")) {
        return Err("This is a development build: update it from source.".into());
    }
    if os == "darwin" {
        return match exe.ancestors().nth(3) {
            Some(app) if parents.first().is_some_and(|p| p == "MacOS") && parents.get(1).is_some_and(|p| p == "Contents") && app.extension().is_some_and(|e| e == "app") => {
                Ok(Target { kind: Kind::Bundle, path: app.to_path_buf() })
            }
            _ => Err("Orbit isn't running from an app bundle.".into()),
        };
    }
    Ok(Target { kind: Kind::Binary, path: exe.to_path_buf() })
}

// ---------- install ----------

pub fn public_key() -> String {
    std::env::var("ORBIT_UPDATE_PUBKEY").ok().filter(|k| !k.is_empty()).unwrap_or_else(|| PUBLIC_KEY.to_string())
}

/// Downloads `release` for `target`, checks its signature against `pubkey`
/// and replaces the install. `progress(done, total)` follows the download.
pub fn install(release: &Release, target: &Target, pubkey: &str, progress: impl FnMut(u64, Option<u64>)) -> Result<(), String> {
    let key = target.platform_key();
    let asset = release.platforms.get(&key).ok_or_else(|| format!("Release {} has no {key} build.", release.version))?;
    // Next to the install, so the final rename stays on one filesystem.
    let dir = target.path.parent().ok_or("the install has no parent folder")?;
    let work = dir.join(format!(".orbit-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("can't write to {}: {e}", dir.display()))?;
    let r = (|| {
        let payload = work.join("download");
        download(&asset.url, &payload, progress)?;
        verify(&payload, &asset.signature, pubkey, &release.version)?;
        let new = unpack(target.kind, &payload, &work)?;
        match target.kind {
            Kind::Bundle => replace_dir(&target.path, &new),
            _ => replace_file(&target.path, &new),
        }
    })();
    let _ = std::fs::remove_dir_all(&work);
    r
}

/// Fetches `url` into `dest` with curl, reporting the bytes written so far.
pub fn download(url: &str, dest: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<(), String> {
    let total = content_length(url);
    let mut child = crate::proc::cmd("curl")
        .args(["-sSfL", "--connect-timeout", "15", "--max-time", "1800", "-o"])
        .arg(dest)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        progress(std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0), total);
        std::thread::sleep(Duration::from_millis(200));
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_string(&mut err);
        }
        let err = err.trim();
        return Err(if err.is_empty() { "the download failed".into() } else { format!("download failed: {err}") });
    }
    let done = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    progress(done, total.or(Some(done)));
    Ok(())
}

fn content_length(url: &str) -> Option<u64> {
    let out = crate::proc::cmd("curl").args(["-sSfLI", "--max-time", "15", url]).output().ok()?;
    // Redirects print one header block each; the last length is the file's.
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse().ok()))
        .last()
        .filter(|n| *n > 0)
}

/// Checks a Tauri-style signature (base64 of the minisign `.sig`) over the
/// file, and that its trusted comment names `version`, so an older signed
/// build can't be served as a newer one.
pub fn verify(file: &Path, signature: &str, pubkey: &str, version: &str) -> Result<(), String> {
    use minisign_verify::{PublicKey, Signature};
    let pk = PublicKey::decode(&unbase64(pubkey)?).map_err(|e| format!("invalid public key: {e}"))?;
    let sig = Signature::decode(&unbase64(signature)?).map_err(|e| format!("invalid signature: {e}"))?;
    let mut v = pk.verify_stream(&sig).map_err(|e| format!("signature check failed: {e}"))?;
    let mut f = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        v.update(&buf[..n]);
    }
    v.finalize().map_err(|e| format!("signature check failed: {e}"))?;
    let signed = sig.trusted_comment().split('\t').find_map(|f| f.strip_prefix("version:")).ok_or("the signature names no version")?;
    if !same(signed, version) {
        return Err(format!("the download is signed for {signed}, not {version}"));
    }
    Ok(())
}

fn unbase64(s: &str) -> Result<String, String> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(|e| format!("invalid base64: {e}"))?;
    String::from_utf8(raw).map_err(|_| "invalid key or signature text".to_string())
}

/// The new executable or bundle inside the download.
fn unpack(kind: Kind, payload: &Path, work: &Path) -> Result<PathBuf, String> {
    if kind == Kind::AppImage {
        return Ok(payload.to_path_buf());
    }
    let out = work.join("unpacked");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    // bsdtar on Windows and macOS reads zip and tar.gz alike.
    let tar = if cfg!(windows) {
        std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("tar.exe")).unwrap_or_else(|| "tar.exe".into())
    } else {
        "tar".into()
    };
    let o = Command::new(tar).arg("-xf").arg(payload).arg("-C").arg(&out).output().map_err(|e| format!("failed to run tar: {e}"))?;
    if !o.status.success() {
        return Err(format!("can't unpack the download: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    let want: &dyn Fn(&Path) -> bool = match kind {
        Kind::Bundle => &|p: &Path| p.is_dir() && p.extension().is_some_and(|e| e == "app"),
        _ if cfg!(windows) => &|p: &Path| p.is_file() && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Orbit.exe")),
        _ => &|p: &Path| p.is_file() && p.file_name().is_some_and(|n| n == "orbit") && p.parent().and_then(|d| d.file_name()).is_some_and(|d| d == "bin"),
    };
    find(&out, want, 4).ok_or_else(|| "the download doesn't contain Orbit".to_string())
}

fn find(dir: &Path, want: &dyn Fn(&Path) -> bool, depth: usize) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    if let Some(p) = entries.iter().find(|p| want(p)) {
        return Some(p.clone());
    }
    if depth == 0 {
        return None;
    }
    entries.iter().filter(|p| p.is_dir()).find_map(|p| find(p, want, depth - 1))
}

/// Puts `new` in place of `target`. Unix: one rename over it (the running
/// process keeps the old inode). Windows: the running exe can be renamed but
/// not overwritten, so it moves aside to `<exe>.old` first.
pub fn replace_file(target: &Path, new: &Path) -> Result<(), String> {
    if cfg!(windows) {
        replace_aside(target, new)
    } else {
        replace_atomic(target, new)
    }
}

fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    target.with_file_name(name)
}

fn replace_atomic(target: &Path, new: &Path) -> Result<(), String> {
    let staged = sibling(target, ".update");
    let r = (|| {
        std::fs::copy(new, &staged)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(target).map(|m| m.permissions().mode()).unwrap_or(0o755) | 0o111;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(mode))?;
        }
        std::fs::File::open(&staged)?.sync_all()?;
        std::fs::rename(&staged, target)
    })();
    r.map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("can't replace {}: {e}", target.display())
    })
}

fn replace_aside(target: &Path, new: &Path) -> Result<(), String> {
    let staged = sibling(target, ".new");
    let old = sibling(target, ".old");
    let fail = |e: std::io::Error| format!("can't replace {}: {e}", target.display());
    std::fs::copy(new, &staged).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        fail(e)
    })?;
    // A previous update's `.old` may still be running; then it stays.
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(target, &old) {
        let _ = std::fs::remove_file(&staged);
        return Err(fail(e));
    }
    if let Err(e) = std::fs::rename(&staged, target) {
        let _ = std::fs::rename(&old, target);
        let _ = std::fs::remove_file(&staged);
        return Err(fail(e));
    }
    Ok(())
}

/// Swaps a bundle directory: the old one moves aside, the new one in, and the
/// old one is deleted (put back if the move fails).
pub fn replace_dir(target: &Path, new: &Path) -> Result<(), String> {
    let old = sibling(target, ".old");
    let _ = std::fs::remove_dir_all(&old);
    let fail = |e: std::io::Error| format!("can't replace {}: {e}", target.display());
    std::fs::rename(target, &old).map_err(fail)?;
    if let Err(e) = std::fs::rename(new, target) {
        let _ = std::fs::rename(&old, target);
        return Err(fail(e));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Removes what an update on Windows leaves next to the exe.
pub fn cleanup() {
    if let Ok(t) = Target::detect() {
        if t.kind == Kind::Binary {
            let _ = std::fs::remove_file(sibling(&t.path, ".old"));
        }
    }
}

/// Starts the updated app with `args`; the caller then quits.
pub fn relaunch(target: &Target, args: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let mut c = if target.kind == Kind::Bundle {
        let mut c = Command::new("/usr/bin/open");
        c.arg("-n").arg(&target.path).arg("--args");
        c
    } else {
        Command::new(&target.path)
    };
    c.args(args);
    // Leave the running AppImage's environment behind.
    for (k, v) in crate::proc::host_env() {
        match v {
            Some(v) => c.env(k, v),
            None => c.env_remove(k),
        };
    }
    for k in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
        c.env_remove(k);
    }
    c.spawn().map(|_| ()).map_err(|e| format!("can't start {}: {e}", target.path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway key pair (`tauri signer generate`, password `testpass`);
    // FIXTURE_SIG signs FIXTURE with `--app-version 1.2.3`.
    const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEM3MTJGREJGREJEMjc3QUUKUldTdWQ5TGJ2LzBTeDVYcHdhRDlPUWJhUytrQlNxbDhUaHUzYkpwWXN5YWdMR1NHdG9zZzh4cHEK";
    const FIXTURE: &[u8] = b"orbit update test payload\n";
    const FIXTURE_SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTdWQ5TGJ2LzBTeDd3YTBrZlNsMm01OTRnZTB0V0NXVEZBSEVzMVdsOGNuRzB6eVpjb3BxZmoxQ1dpc21kd3Q3QXE2c2NFcDltV2Y5QWxpVU0zc25KWG45MmdRR0FHcHdjPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzODExNDI4CWZpbGU6Zml4dHVyZS5iaW4JdmVyc2lvbjoxLjIuMwpvaVN4bXY3aWZRNzhua2VNbTlJdDVxckJvMlNEUUJENlp4dzNNQ2pNeXVIK0lFRW9uSXJYQ1Y5L2QxaERSeEh0M0F5dVNmL2lmbGpwK21ZUkNHQ2lBUT09Cg==";

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("orbit-updates-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn compares_versions() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("v1.10.0", "1.9.3"));
        assert!(!newer("0.1.0", "0.1"));
        assert!(newer("0.2.0", "0.2.0-beta.1"));
        assert!(!newer("0.1.9", "0.2.0"));
        assert!(same("v1.2.0", "1.2"));
    }

    #[test]
    fn reads_the_manifest() {
        let raw = r#"{"version":"v0.3.0","notes":"Fixes","pub_date":"x","platforms":{
            "linux-x86_64-appimage":{"signature":"c2ln","url":"https://x/Orbit.AppImage"}}}"#;
        let r = parse(raw, "0.1.0").unwrap().unwrap();
        assert_eq!((r.version.as_str(), r.notes.as_deref()), ("0.3.0", Some("Fixes")));
        assert_eq!(r.platforms["linux-x86_64-appimage"], Asset { url: "https://x/Orbit.AppImage".into(), signature: "c2ln".into() });
        assert_eq!(parse(raw, "0.3.0").unwrap(), None);
        assert_eq!(parse(r#"{"version":"1.0.0"}"#, "0.1.0").unwrap().unwrap().platforms.len(), 0);
        assert!(parse("{}", "0.1.0").is_err());
        assert!(parse("not json", "0.1.0").is_err());
    }

    #[test]
    fn detects_the_install() {
        let t = |os, img: Option<&str>, exe: &str| detect_from(os, img.map(OsString::from), Path::new(exe));
        assert_eq!(t("linux", Some("/home/u/Orbit.AppImage"), "/tmp/.mount_x/usr/bin/orbit").unwrap(), Target { kind: Kind::AppImage, path: "/home/u/Orbit.AppImage".into() });
        assert_eq!(t("linux", None, "/home/u/.local/bin/orbit").unwrap().kind, Kind::Binary);
        assert!(t("linux", None, "/src/orbit/native/target/release/orbit-native").is_err());
        assert!(t("linux", None, "/src/native/target/x86_64-unknown-linux-gnu/release/orbit-native").is_err());
        assert_eq!(t("darwin", None, "/Applications/Orbit.app/Contents/MacOS/Orbit").unwrap(), Target { kind: Kind::Bundle, path: "/Applications/Orbit.app".into() });
        assert!(t("darwin", None, "/usr/local/bin/orbit").is_err());
        assert_eq!(t("windows", None, r"C:\Users\u\AppData\Local\Orbit\Orbit.exe").unwrap().kind, Kind::Binary);
        assert_eq!(platform_key("linux", "x86_64", Kind::AppImage), "linux-x86_64-appimage");
        assert_eq!(platform_key("linux", "x86_64", Kind::Binary), "linux-x86_64-tar");
        assert_eq!(platform_key("windows", "x86_64", Kind::Binary), "windows-x86_64-zip");
        assert_eq!(platform_key("darwin", "aarch64", Kind::Bundle), "darwin-aarch64-app");
    }

    #[test]
    fn verifies_signatures() {
        let d = scratch("verify");
        let f = d.join("fixture.bin");
        std::fs::write(&f, FIXTURE).unwrap();
        verify(&f, FIXTURE_SIG, TEST_PUBKEY, "1.2.3").unwrap();
        verify(&f, FIXTURE_SIG, TEST_PUBKEY, "v1.2.3").unwrap();
        // Signed for another version.
        assert!(verify(&f, FIXTURE_SIG, TEST_PUBKEY, "1.2.4").unwrap_err().contains("signed for 1.2.3"));
        // Another key (the release key).
        assert!(verify(&f, FIXTURE_SIG, PUBLIC_KEY, "1.2.3").is_err());
        // Tampered data.
        std::fs::write(&f, b"orbit update test payload!\n").unwrap();
        assert!(verify(&f, FIXTURE_SIG, TEST_PUBKEY, "1.2.3").is_err());
        assert!(verify(&f, "not base64!", TEST_PUBKEY, "1.2.3").is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_release_key_decodes() {
        assert!(minisign_verify::PublicKey::decode(&unbase64(PUBLIC_KEY).unwrap()).is_ok());
    }

    #[test]
    fn replaces_a_file() {
        let d = scratch("replace");
        let (target, new) = (d.join("Orbit.AppImage"), d.join("new"));
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        #[cfg(unix)]
        let before = {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
            // An open handle keeps reading the old file, as the running app does.
            let h = std::fs::File::open(&target).unwrap();
            (h, std::fs::metadata(&target).unwrap().ino())
        };
        replace_atomic(&target, &new).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!sibling(&target, ".update").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let m = std::fs::metadata(&target).unwrap();
            assert_eq!(m.permissions().mode() & 0o777, 0o751);
            assert_ne!(m.ino(), before.1);
            let mut old = String::new();
            let mut h = before.0;
            h.read_to_string(&mut old).unwrap();
            assert_eq!(old, "old");
        }
        // A missing source leaves the target alone.
        assert!(replace_atomic(&target, &d.join("missing")).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"new");

        // The Windows rename dance.
        std::fs::write(&new, b"newer").unwrap();
        replace_aside(&target, &new).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"newer");
        assert_eq!(std::fs::read(sibling(&target, ".old")).unwrap(), b"new");
        assert!(!sibling(&target, ".new").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn replaces_a_bundle() {
        let d = scratch("bundle");
        let (app, new) = (d.join("Orbit.app"), d.join("work/Orbit.app"));
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/Orbit"), b"old").unwrap();
        std::fs::create_dir_all(new.join("Contents/MacOS")).unwrap();
        std::fs::write(new.join("Contents/MacOS/Orbit"), b"new").unwrap();
        replace_dir(&app, &new).unwrap();
        assert_eq!(std::fs::read(app.join("Contents/MacOS/Orbit")).unwrap(), b"new");
        assert!(!sibling(&app, ".old").exists() && !new.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn unpacks_a_tarball() {
        let d = scratch("unpack");
        let tree = d.join("src/Orbit-1.0.0-linux-x86_64");
        std::fs::create_dir_all(tree.join("bin")).unwrap();
        std::fs::create_dir_all(tree.join("share/orbit")).unwrap();
        std::fs::write(tree.join("bin/orbit"), b"exe").unwrap();
        std::fs::write(tree.join("share/orbit/orbit"), b"not this").unwrap();
        let tgz = d.join("o.tar.gz");
        assert!(Command::new("tar").arg("-czf").arg(&tgz).arg("-C").arg(d.join("src")).arg("Orbit-1.0.0-linux-x86_64").status().unwrap().success());
        let work = d.join("work");
        let exe = unpack(Kind::Binary, &tgz, &work).unwrap();
        assert!(exe.ends_with("Orbit-1.0.0-linux-x86_64/bin/orbit"));
        assert_eq!(unpack(Kind::AppImage, &tgz, &work).unwrap(), tgz);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
