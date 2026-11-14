//! App-level settings that live outside any vault: just the last vault.
//! Written when a vault is opened, never while idle.

use std::fs;
use std::path::PathBuf;

fn file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("Obsession").join("last_vault.txt"))
}

pub fn last_vault() -> Option<PathBuf> {
    let path = fs::read_to_string(file()?).ok()?;
    let path = PathBuf::from(path.trim());
    path.is_dir().then_some(path)
}

pub fn remember_vault(vault: &std::path::Path) {
    let Some(file) = file() else { return };
    let text = vault.to_string_lossy();
    if fs::read_to_string(&file).is_ok_and(|old| old.trim() == text) {
        return;
    }
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = obsession_core::write::atomic_write(&file, text.as_bytes());
}
