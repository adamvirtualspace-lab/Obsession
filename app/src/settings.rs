//! App-level settings that live outside any vault, in
//! `%APPDATA%\Obsession`: the last vault, and this computer's preferences
//! (compact mode). Written when they change, never while idle.

use std::fs;
use std::path::PathBuf;

fn file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("Obsession").join("last_vault.txt"))
}

fn prefs_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("Obsession").join("prefs.txt"))
}

/// This computer's preferences.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Prefs {
    pub compact: bool,
}

pub fn prefs() -> Prefs {
    let text = prefs_file().and_then(|f| fs::read_to_string(f).ok()).unwrap_or_default();
    let mut prefs = Prefs::default();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "compact"
        {
            prefs.compact = value.trim() == "true";
        }
    }
    prefs
}

pub fn save_prefs(prefs: Prefs) {
    let Some(file) = prefs_file() else { return };
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = obsession_core::write::atomic_write(&file, format!("compact={}\n", prefs.compact).as_bytes());
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
