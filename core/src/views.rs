//! Where you last looked: each graph's and each function's view, kept in
//! `.obsessed/views.txt` for this machine only (Syntax.md section 11).
//!
//! A view is the graph point at the middle of the canvas and the zoom, so it
//! comes back centred on the same place whatever the window's size.
//!
//! ```text
//! # Obsession views: centre x, centre y, zoom, then the graph or function.
//! 412.0 220.5 0.850 shots/_nodes.md
//! 300.0 80.0 1.000 _funct.md#Wait For Done
//! 260.0 140.0 0.700 shots/_nodes.md#Check Lighting
//! ```
//!
//! The app holds them in memory and writes the file when you leave a graph
//! or close the app, never while idle.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::log;
use crate::write::atomic_write;

/// The file, in `.obsessed/`.
pub const FILE: &str = "views.txt";

const HEADER: &str = "# Obsession views: centre x, centre y, zoom, then the graph or function.";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// The graph point in the middle of the canvas.
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl View {
    /// Rounded the way the file keeps it, so a view read back compares equal.
    fn rounded(self) -> Self {
        Self {
            x: (self.x * 10.0).round() / 10.0,
            y: (self.y * 10.0).round() / 10.0,
            zoom: (self.zoom * 1000.0).round() / 1000.0,
        }
    }
}

/// The key for a graph: its note.
pub fn graph_key(rel: &str) -> String {
    rel.to_owned()
}

/// The key for a function: the file that holds it (`_funct.md` or the
/// graph), `#`, its name.
pub fn function_key(file: &str, name: &str) -> String {
    format!("{file}#{name}")
}

/// The file a key belongs to (a graph's note, or the file a function is in).
pub fn key_file(key: &str) -> &str {
    key.split_once('#').map_or(key, |(file, _)| file)
}

#[derive(Debug, Default, Clone)]
pub struct Views {
    map: BTreeMap<String, View>,
    /// Changed since it was last read or written.
    dirty: bool,
}

impl Views {
    /// Reads the file's text. Lines it doesn't understand are skipped.
    pub fn parse(text: &str) -> Self {
        let mut map = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.splitn(4, ' ');
            let (Some(x), Some(y), Some(zoom), Some(key)) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            let (Ok(x), Ok(y), Ok(zoom)) = (x.parse::<f32>(), y.parse::<f32>(), zoom.parse::<f32>()) else {
                continue;
            };
            let key = key.trim();
            if key.is_empty() || !x.is_finite() || !y.is_finite() || !zoom.is_finite() || zoom <= 0.0 {
                continue;
            }
            map.insert(key.to_owned(), View { x, y, zoom });
        }
        Self { map, dirty: false }
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from(HEADER);
        out.push('\n');
        for (key, v) in &self.map {
            out.push_str(&format!("{:.1} {:.1} {:.3} {key}\n", v.x, v.y, v.zoom));
        }
        out
    }

    pub fn get(&self, key: &str) -> Option<View> {
        self.map.get(key).copied()
    }

    /// Remembers a view. Marks the views changed only if it really moved.
    pub fn set(&mut self, key: &str, view: View) {
        if !(view.x.is_finite() && view.y.is_finite() && view.zoom.is_finite() && view.zoom > 0.0) {
            return;
        }
        let view = view.rounded();
        if self.map.get(key) != Some(&view) {
            self.map.insert(key.to_owned(), view);
            self.dirty = true;
        }
    }

    /// Forgets views whose graph or function file is gone.
    pub fn prune(&mut self, exists: impl Fn(&str) -> bool) {
        let before = self.map.len();
        self.map.retain(|key, _| exists(key_file(key)));
        self.dirty |= self.map.len() != before;
    }

    /// A function was renamed: its view follows it.
    pub fn move_view(&mut self, from: &str, to: &str) {
        if from != to
            && let Some(view) = self.map.remove(from)
        {
            self.map.insert(to.to_owned(), view);
            self.dirty = true;
        }
    }

    /// A note was renamed: its views follow it.
    pub fn rename(&mut self, old: &str, new: &str) {
        let moved: Vec<String> = self.map.keys().filter(|k| key_file(k) == old).cloned().collect();
        for key in moved {
            if let Some(view) = self.map.remove(&key) {
                let rest = &key[old.len()..];
                self.map.insert(format!("{new}{rest}"), view);
                self.dirty = true;
            }
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn path(root: &Path) -> PathBuf {
        root.join(log::FOLDER).join(FILE)
    }

    /// The vault's views, or none if there's no file yet.
    pub fn load(root: &Path) -> Self {
        std::fs::read_to_string(Self::path(root)).map_or_else(|_| Self::default(), |text| Self::parse(&text))
    }

    /// Writes the file if anything changed (atomically, creating
    /// `.obsessed/` with its `.gitignore` if needed).
    pub fn save(&mut self, root: &Path) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        log::ensure(root)?;
        atomic_write(&Self::path(root), self.to_text().as_bytes())?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, zoom: f32) -> View {
        View { x, y, zoom }
    }

    #[test]
    fn round_trips_graphs_and_functions_with_spaces() {
        let mut views = Views::default();
        views.set(&graph_key("ep04 delivery_nodes.md"), v(412.04, 220.56, 0.85));
        views.set(&function_key("_funct.md", "Wait For Done"), v(300.0, 80.0, 1.0));
        let text = views.to_text();
        assert_eq!(
            text,
            "# Obsession views: centre x, centre y, zoom, then the graph or function.\n\
             300.0 80.0 1.000 _funct.md#Wait For Done\n\
             412.0 220.6 0.850 ep04 delivery_nodes.md\n"
        );
        let back = Views::parse(&text);
        assert!(!back.is_dirty());
        assert_eq!(back.get("ep04 delivery_nodes.md"), Some(v(412.0, 220.6, 0.85)));
        assert_eq!(back.get("_funct.md#Wait For Done"), Some(v(300.0, 80.0, 1.0)));
    }

    #[test]
    fn skips_lines_it_does_not_understand() {
        let views = Views::parse(
            "# comment\n\nnot a view\n1 2 shots/_nodes.md\n1 2 0 zero_nodes.md\n1 2 -1 neg_nodes.md\n\
             x 2 1 bad_nodes.md\nNaN 2 1 nan_nodes.md\n1 2 1 \n  10 -20.5 0.5 a/_nodes.md  \n",
        );
        assert_eq!(views.len(), 1);
        assert_eq!(views.get("a/_nodes.md"), Some(v(10.0, -20.5, 0.5)));
    }

    #[test]
    fn only_a_real_change_marks_it_dirty() {
        let mut views = Views::parse("10.0 20.0 1.000 g_nodes.md\n");
        views.set("g_nodes.md", v(10.01, 19.99, 1.0001));
        assert!(!views.is_dirty(), "the same view after rounding");
        views.set("g_nodes.md", v(11.0, 20.0, 1.0));
        assert!(views.is_dirty());
        views.set("bad_nodes.md", v(f32::NAN, 0.0, 1.0));
        assert_eq!(views.get("bad_nodes.md"), None);
    }

    #[test]
    fn prune_and_rename_follow_the_files() {
        let mut views = Views::default();
        views.set("shots/_nodes.md", v(1.0, 1.0, 1.0));
        views.set("shots/_nodes.md#Check Lighting", v(2.0, 2.0, 1.0));
        views.set("old_nodes.md", v(3.0, 3.0, 1.0));
        views.set("_funct.md#Copy Status", v(4.0, 4.0, 1.0));
        views.rename("shots/_nodes.md", "seq12/_nodes.md");
        assert_eq!(views.get("seq12/_nodes.md"), Some(v(1.0, 1.0, 1.0)));
        assert_eq!(views.get("seq12/_nodes.md#Check Lighting"), Some(v(2.0, 2.0, 1.0)));
        assert_eq!(views.get("shots/_nodes.md"), None);
        views.prune(|file| file != "old_nodes.md");
        assert_eq!(views.get("old_nodes.md"), None);
        assert_eq!(views.len(), 3);
    }

    #[test]
    fn saves_only_when_changed_and_reads_back() {
        let root = std::env::temp_dir().join(format!("obsession-views-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut views = Views::load(&root);
        assert!(views.is_empty());
        views.save(&root).unwrap();
        assert!(!Views::path(&root).exists(), "nothing changed, nothing written");
        views.set("a_nodes.md", v(5.0, 6.0, 0.75));
        views.save(&root).unwrap();
        assert!(!views.is_dirty());
        assert!(root.join(log::FOLDER).join(".gitignore").exists());
        assert_eq!(Views::load(&root).get("a_nodes.md"), Some(v(5.0, 6.0, 0.75)));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
