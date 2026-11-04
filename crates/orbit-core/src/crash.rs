//! Crash logs: the app's panics are appended to a log file with a timestamp.
use std::io::Write;
use std::path::Path;

/// Appends one entry to `file`.
pub fn append(file: &Path, message: &str) -> Result<(), String> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file).map_err(|e| e.to_string())?;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    writeln!(f, "=== {secs}s ===").ok();
    writeln!(f, "{message}").ok();
    writeln!(f).ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn appends_entries() {
        let f = std::env::temp_dir().join(format!("orbit-crash-test-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&f);
        super::append(&f, "first").unwrap();
        super::append(&f, "second").unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("===") && text.contains("first\n") && text.contains("second\n"));
        let _ = std::fs::remove_file(&f);
    }
}
