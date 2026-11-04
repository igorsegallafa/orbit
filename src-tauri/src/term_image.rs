// Files named by kitty graphics commands (`t=f`, `t=t`), which the webview can't read.
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tauri::ipc::Response;

const MAX_BYTES: u64 = 128 * 1024 * 1024;

#[tauri::command]
pub async fn pty_read_image(path: String, offset: Option<u64>, size: Option<u64>, temp: bool) -> Result<Response, String> {
    tauri::async_runtime::spawn_blocking(move || read_image(Path::new(&path), offset.unwrap_or(0), size.unwrap_or(0), temp))
        .await
        .map_err(|e| e.to_string())?
        .map(Response::new)
}

fn read_image(path: &Path, offset: u64, size: u64, temp: bool) -> Result<Vec<u8>, String> {
    if !path.is_absolute() {
        return Err("path must be absolute".into());
    }
    let real = path.canonicalize().map_err(|e| e.to_string())?;
    if ["/proc", "/sys", "/dev"].iter().any(|p| real.starts_with(p)) && !real.starts_with("/dev/shm") {
        return Err("not a regular file".into());
    }
    let meta = std::fs::metadata(&real).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    let len = if size > 0 { size } else { meta.len().saturating_sub(offset) };
    if len > MAX_BYTES {
        return Err("file too big".into());
    }
    let mut file = std::fs::File::open(&real).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(len).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    // Only files the protocol names as temporary, so a stray `t=t` can't delete anything else.
    if temp && is_graphics_temp_file(&real) {
        let _ = std::fs::remove_file(&real);
    }
    Ok(bytes)
}

fn is_graphics_temp_file(path: &Path) -> bool {
    let named = path.file_name().is_some_and(|n| n.to_string_lossy().contains("tty-graphics-protocol"));
    let dirs: Vec<PathBuf> = [std::env::temp_dir(), "/tmp".into(), "/dev/shm".into()]
        .into_iter()
        .filter_map(|d| d.canonicalize().ok())
        .collect();
    named && dirs.iter().any(|d| path.starts_with(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_slice_and_deletes_only_marked_temp_files() {
        let dir = std::env::temp_dir();
        let marked = dir.join(format!("orbit-tty-graphics-protocol-{}", std::process::id()));
        std::fs::write(&marked, b"0123456789").unwrap();
        assert_eq!(read_image(&marked, 2, 3, false).unwrap(), b"234");
        assert_eq!(read_image(&marked, 0, 0, true).unwrap(), b"0123456789");
        assert!(!marked.exists());

        let plain = dir.join(format!("orbit-image-{}", std::process::id()));
        std::fs::write(&plain, b"x").unwrap();
        assert_eq!(read_image(&plain, 0, 0, true).unwrap(), b"x");
        assert!(plain.exists());
        let _ = std::fs::remove_file(&plain);

        assert!(read_image(Path::new("relative.png"), 0, 0, false).is_err());
        assert!(read_image(&dir, 0, 0, false).is_err());
    }
}
