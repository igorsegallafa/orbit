//! `orbit-sign <version> <file>...` writes `<file>.sig` for each file: a
//! minisign signature, base64-wrapped as `tauri signer sign` writes it, whose
//! trusted comment names the version (the updater checks it).
//! The key comes from TAURI_SIGNING_PRIVATE_KEY (base64 of the minisign secret
//! key file, as `tauri signer generate` makes it) and
//! TAURI_SIGNING_PRIVATE_KEY_PASSWORD.
use base64::{engine::general_purpose::STANDARD, Engine};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    if let Err(e) = run() {
        eprintln!("orbit-sign: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let version = args.next().ok_or("usage: orbit-sign <version> <file>...")?;
    let version = version.trim_start_matches('v');
    let files: Vec<String> = args.collect();
    if files.is_empty() {
        return Err("no files to sign".into());
    }
    let key = std::env::var("TAURI_SIGNING_PRIVATE_KEY").map_err(|_| "TAURI_SIGNING_PRIVATE_KEY is not set")?;
    let password = std::env::var("TAURI_SIGNING_PRIVATE_KEY_PASSWORD").unwrap_or_default();
    let key = String::from_utf8(STANDARD.decode(key.trim()).map_err(|e| format!("the key isn't base64: {e}"))?).map_err(|_| "the key isn't text")?;
    let sk = minisign::SecretKeyBox::from_string(&key)
        .and_then(|b| b.into_secret_key(Some(password)))
        .map_err(|e| format!("can't open the key: {e}"))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    for file in files {
        let path = Path::new(&file);
        let name = path.file_name().ok_or("bad file name")?.to_string_lossy();
        let data = std::fs::File::open(path).map_err(|e| format!("{file}: {e}"))?;
        let trusted = format!("timestamp:{now}\tfile:{name}\tversion:{version}");
        let sig = minisign::sign(None, &sk, data, Some(&trusted), Some("signature from tauri secret key")).map_err(|e| format!("{file}: {e}"))?;
        let out = format!("{file}.sig");
        std::fs::write(&out, STANDARD.encode(sig.into_string())).map_err(|e| format!("{out}: {e}"))?;
        println!("{out}");
    }
    Ok(())
}
