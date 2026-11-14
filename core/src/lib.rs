//! Obsession core: everything that isn't UI.
//!
//! - [`vault`]: scanning a folder into a sorted list of folders and notes.
//! - [`store`]: the RAM-first file store (read once, write only results).
//! - [`write`]: safe writes (temporary file, then swap it in).
//! - [`preview`]: the read-only pre-pass that turns Obsidian syntax into
//!   plain markdown for the preview. It never touches the file.
//! - [`fuzzy`]: matching for the quick switcher.
//! - [`syntax`]: line-by-line markdown tokens for editor highlighting.

pub mod fuzzy;
pub mod preview;
pub mod store;
pub mod syntax;
pub mod vault;
pub mod write;

pub use store::Store;
pub use vault::Vault;
