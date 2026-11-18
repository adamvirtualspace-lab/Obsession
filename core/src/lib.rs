//! Obsession core: everything that isn't UI.
//!
//! - [`vault`]: scanning a folder into a sorted list of folders and notes.
//! - [`store`]: the RAM-first file store (read once, write only results).
//! - [`write`]: safe writes (temporary file, then swap it in).
//! - [`preview`]: the read-only pre-pass that turns Obsidian syntax into
//!   plain markdown for the preview. It never touches the file.
//! - [`fuzzy`]: matching for the quick switcher.
//! - [`syntax`]: line-by-line markdown tokens for editor highlighting.
//! - [`status`]: tasks, status formats and what a click changes.
//! - [`links`]: links in a note, the vault's link index, renames.
//! - [`nodes`]: graph files (`_nodes.md`): nodes, wires, layout.
//! - [`func`]: functions: the text format, node library, type checking,
//!   evaluation.
//! - [`runner`]: which functions a change affects, runs with chains and the
//!   loop guard, undo.
//! - [`log`]: the run log in `.obsessed/log/`.
//! - [`crew`]: `@mentions`, crew cards in `_crew/`, badges.
//! - [`search`]: vault search (built on first use).

pub mod crew;
pub mod doc;
pub mod func;
pub mod fuzzy;
pub mod history;
pub mod links;
pub mod log;
pub mod live;
pub mod nodes;
pub mod preview;
pub mod runner;
pub mod search;
pub mod status;
pub mod store;
pub mod syntax;
pub mod vault;
pub mod write;

pub use store::Store;
pub use vault::Vault;
