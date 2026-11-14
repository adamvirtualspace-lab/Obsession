//! Safe writes: write a temporary file next to the target, then swap it in.
//!
//! Nobody (Obsidian, git, a teammate's sync) ever sees a half-written file.
//! On Windows another program can briefly hold the file open, so the swap is
//! retried a few times before giving up.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const RETRIES: u32 = 5;
const RETRY_DELAY: Duration = Duration::from_millis(40);

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = temp_path(path);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }

    let mut attempt = 0;
    loop {
        match fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(err) if attempt < RETRIES && is_transient(&err) => {
                attempt += 1;
                thread::sleep(RETRY_DELAY);
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                return Err(err);
            }
        }
    }
}

/// `notes/sh040.md` -> `notes/.sh040.md.obsession-tmp`. Hidden, so the vault
/// scan and other tools that skip dot-files ignore it.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.obsession-tmp"))
}

fn is_transient(err: &io::Error) -> bool {
    // Windows: ERROR_ACCESS_DENIED (5), ERROR_SHARING_VIOLATION (32),
    // ERROR_LOCK_VIOLATION (33) while another program has the file open.
    matches!(err.kind(), io::ErrorKind::PermissionDenied)
        || matches!(err.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_content_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("obsession-write-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.md");
        fs::write(&path, "old").unwrap();

        atomic_write(&path, b"new\r\nline").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new\r\nline");
        assert!(!temp_path(&path).exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
