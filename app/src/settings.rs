//! App-level settings that live outside any vault, in
//! `%APPDATA%\Obsession`: the last vault, and this computer's preferences
//! (compact mode, the file tree's width and whether it shows). Written when
//! they change, never while idle.

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

/// The file tree's width when nothing is saved.
pub const SIDEBAR_WIDTH: u32 = 260;

/// This computer's preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    pub compact: bool,
    /// The file tree's width in logical pixels.
    pub sidebar_width: u32,
    pub sidebar_hidden: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { compact: false, sidebar_width: SIDEBAR_WIDTH, sidebar_hidden: false }
    }
}

pub fn prefs() -> Prefs {
    let text = prefs_file().and_then(|f| fs::read_to_string(f).ok()).unwrap_or_default();
    let mut prefs = Prefs::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        match key.trim() {
            "compact" => prefs.compact = value == "true",
            "sidebar_width" => prefs.sidebar_width = value.parse().unwrap_or(SIDEBAR_WIDTH),
            "sidebar" => prefs.sidebar_hidden = value == "hidden",
            _ => {}
        }
    }
    prefs
}

pub fn save_prefs(prefs: Prefs) {
    let Some(file) = prefs_file() else { return };
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let text = format!(
        "compact={}\nsidebar_width={}\nsidebar={}\n",
        prefs.compact,
        prefs.sidebar_width,
        if prefs.sidebar_hidden { "hidden" } else { "shown" }
    );
    let _ = obsession_core::write::atomic_write(&file, text.as_bytes());
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
