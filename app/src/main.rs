// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backlinks;
#[cfg(debug_assertions)]
mod dev;
mod find;
mod func_editor;
mod graph;
mod source_scroll;
mod highlight;
mod live;
mod live_text;
mod new_note;
mod note;
mod pictures;
mod right_click;
mod runs;
mod settings;
mod sidebar;
mod smooth;
mod status_menu;
mod switcher;
mod theme;
mod timer;
mod watch;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use iced::keyboard::{self, Key, key::Named};
use iced::widget::{
    Space, button, column, container, operation, row, rule, stack, text, text_editor,
};
use iced::{Element, Fill, Point, Size, Subscription, Task, event, window};

use obsession_core::status::{self, Format, Source};
use obsession_core::store::{SaveResult, Store};
use obsession_core::links::{self, Backlink, LinkIndex};
use obsession_core::crew::{self, Member};
use obsession_core::func::{self, Library, Origin};
use obsession_core::func::eval::Bind;
use obsession_core::nodes::{self, End, Section};
use obsession_core::runner::{self, Change, Index, Outcome};
use obsession_core::views::{self, Views};
use obsession_core::history::{self, Elsewhere};
use obsession_core::store::Outside;

use crate::highlight::LinkStates;
use obsession_core::vault::{self, Kind, Resolved, Vault};

use func_editor::{Context, Editor, Picker};
use new_note::NewNote;
use note::{Mode, Note, Timer};
use pictures::Pictures;
use sidebar::Tree;
use status_menu::StatusMenu;
use switcher::Switcher;

pub fn main() -> iced::Result {
    graphics_defaults();
    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .theme(|_: &App| theme::theme())
        .subscription(App::subscription)
        .default_font(theme::UI_FONT)
        .antialiasing(dev_msaa())
        .window_size(window_size())
        .exit_on_close_request(false)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    /// The window's scale factor (at start, and when it moves to another
    /// screen), for decoding pictures sharp.
    ScaleFactor(f32),
    /// A picture finished decoding (for the store's generation then).
    PictureLoaded(pictures::Key, u64, Result<pictures::Decoded, String>),
    PickFolder,
    FolderPicked(Option<PathBuf>),
    Scanned(Result<Vault, String>),
    /// The vault's links were read (for the vault at this root).
    Indexed(PathBuf, Arc<LinkIndex>),
    Backlinks(backlinks::Message),
    ToggleBacklinks,
    Graph(graph::Message),
    /// Show the open `_nodes.md` as a graph (or as text).
    ToggleGraph,
    ToggleLinkWires,
    /// Show or hide the crew hand in the graph.
    ToggleHand,
    Settings(SettingsMessage),
    Find(find::Message),
    /// The vault search index is ready (for the vault at this root).
    SearchBuilt(PathBuf, Built),
    /// From the "assign to which task" box: (node card, task row), or close.
    AssignTo(Option<(usize, usize)>),
    /// From "Remove the link?": remove it, or keep it.
    Unlink(bool),
    /// Pick a note to add to the open graph.
    AddNode,
    /// Pick a function to add to the open graph as a node.
    AddFunctionNode,
    Editor(func_editor::Message),
    Picker(func_editor::PickerMessage),
    /// Nothing to do; just lays the window out again.
    Settle,
    /// The vault was scanned again after a note was created; open `rel`.
    Rescanned(Result<Vault, String>, Box<str>),
    NewNote(new_note::Message),
    StartNewNote,
    Sidebar(sidebar::Message),
    Edit(text_editor::Action),
    Timer(u64, Timer),
    Saved(SaveResult),
    SetMode(Mode),
    Shortcut(Shortcut),
    Switcher(switcher::Message),
    Live(live::Message),
    /// Right-click in the editor: (editor-local point, window point).
    EditorRightClick(Point, Point),
    StatusMenu(status_menu::Message),
    Resized(Size),
    CloseRequested(window::Id),
    /// The watcher saw files change.
    Fs(Vec<runs::FsEvent>),
    /// Read the next slice of notes changed in other apps.
    DrainOutside,
    /// Notes changed in other apps, read off the UI thread: each one's text,
    /// or `Err(true)` when it's gone.
    OutsideRead(Vec<(String, Result<String, bool>)>),
    /// Read the vault folder again (a delayed, coalesced rescan).
    Rescan,
    /// The vault's list was read again after files came or went.
    VaultRefreshed(Result<Vault, String>),
    /// Time for the pending run.
    RunNow,
    RunDone(Arc<RunResult>),
    Runs(runs::Message),
    ToggleRuns,
    /// Complete the `@name` being typed (with this crew key, or the first
    /// suggestion).
    CompleteMention(Option<String>),
    /// Run the open graph's out-of-date functions.
    RunGraph,
    #[cfg(debug_assertions)]
    Dev(dev::Step),
}

#[derive(Debug, Clone, Copy)]
enum Shortcut {
    OpenFolder,
    NewNote,
    QuickSwitch,
    CycleMode,
    Save,
    Undo,
    Redo,
    Rename,
    FindInVault,
    Up,
    Down,
    Escape,
    /// Tab that no widget used: completes an `@name` if suggestions show.
    Tab,
    /// Ctrl+C that no text box used: copies a selection made in Reading.
    Copy,
}

struct Workspace {
    vault: Vault,
    store: Store,
    tree: Tree,
    note: Option<Note>,
    /// `statuses:` from `_settings.md`, the team default.
    team: Option<Format>,
    /// Who links to whom (empty until `indexed`).
    links: LinkIndex,
    indexed: bool,
    /// Links to the open note.
    backlinks: Vec<Backlink>,
    /// Functions: what runs when, and the run log.
    runs: runs::Runs,
    /// The link index changed since its cache was written.
    links_dirty: bool,
    /// Vault search, once it's been used.
    search: Option<obsession_core::search::SearchIndex>,
    search_building: bool,
    /// The crew folder (`crew:` in `_settings.md`, else `_crew`).
    crew_folder: String,
    /// Where each graph and function was last looked at
    /// (`.obsessed/views.txt`), written on leaving a graph and on close.
    views: Views,
    /// Undo for what the open note's view changed in other notes (a link
    /// removed from the graph, a crew card dropped on a task).
    elsewhere: Elsewhere,
    /// The crew, by name: cards in the crew folder, and people's folders.
    crew: Vec<Member>,
    /// Open `@mentions` per person across the vault.
    mentions: HashMap<String, u32>,
    /// Notes changed in other apps, waiting to be read a slice at a time.
    outside: OutsideQueue,
    /// Rescans of the vault folder, throttled (`App::rescan`).
    rescans: Rescans,
}

#[derive(Default)]
struct Rescans {
    running: bool,
    /// Asked for while one was running.
    again: bool,
    /// One is waiting for the gap to pass.
    scheduled: bool,
    last: Option<std::time::Instant>,
}

/// A burst of outside changes being read (`App::drain_outside`).
#[derive(Default)]
struct OutsideQueue {
    queue: std::collections::VecDeque<PathBuf>,
    /// What's in `queue`, so a note changed twice is read once.
    queued: std::collections::HashSet<PathBuf>,
    /// A slice is scheduled.
    draining: bool,
    /// Read so far in this burst, and warnings to show at its end.
    changed: Vec<String>,
    warnings: Vec<String>,
    /// The queue has been empty since then (the burst ends after
    /// `OUTSIDE_QUIET` of that).
    quiet_since: Option<std::time::Instant>,
    /// For the dev timing: when the burst started, slices, notes looked at.
    started: Option<std::time::Instant>,
    slices: u32,
    read: usize,
}

/// How long one slice of outside changes may take, and the pause after it.
const OUTSIDE_SLICE: std::time::Duration = std::time::Duration::from_millis(4);
const OUTSIDE_PAUSE: std::time::Duration = std::time::Duration::from_millis(12);
/// Notes the store doesn't hold, read off the UI thread per slice.
const OUTSIDE_BATCH: usize = 64;
/// A burst ends when no change has come for this long.
const OUTSIDE_QUIET: std::time::Duration = std::time::Duration::from_millis(150);
/// Rescans of the vault folder start at most this often.
const RESCAN_GAP: std::time::Duration = std::time::Duration::from_millis(250);

impl Workspace {
    /// Reads the crew folder: each card note in it (through the store),
    /// and each person's folder (with or without its card yet).
    fn load_crew(&mut self) {
        let seats = crew::seats(
            &self.crew_folder,
            self.vault.entries.iter().map(|e| (&*e.rel, e.kind == Kind::Folder)),
        );
        let mut crew: Vec<Member> = Vec::new();
        for seat in seats {
            let exists = self.vault.note(&seat.card).is_some();
            let text = if exists { self.store.get(&seat.card).ok().map(str::to_owned) } else { None };
            let mut m = match text {
                Some(text) => crew::card(&seat.card, &text),
                None => {
                    let mut m = crew::card(&seat.card, "");
                    m.exists = false;
                    m
                }
            };
            // A person's folder names them unless their card does.
            if let Some(folder) = &seat.folder_name
                && m.name.eq_ignore_ascii_case(folder)
            {
                m.name = folder.clone();
            }
            m.key = seat.key;
            crew.push(m);
        }
        crew.sort_by_key(|m| m.name.to_lowercase());
        self.crew = crew;
    }

    /// Whether people in the crew folder have folders of their own.
    fn crew_as_folders(&self) -> bool {
        self.crew.iter().any(|m| m.rel.matches('/').count() > self.crew_folder.matches('/').count() + 1)
    }

    /// Each crew member's badge from vault-wide open mentions.
    fn vault_badges(&self) -> Vec<Option<crew::Badge>> {
        let counts: Vec<u32> = self.crew.iter().map(|m| self.mentions.get(&m.key).copied().unwrap_or(0)).collect();
        crew::badges(&counts)
    }
}

/// `@name` suggestions while typing.
struct Completion {
    typed: String,
    /// (key, name, role)
    matches: Vec<(String, String, String)>,
}

/// A finished run, with its summary (made against the index it ran on).
#[derive(Debug)]
pub struct RunResult {
    outcome: Outcome,
    summary: String,
    /// What each blocked job is (for messages).
    names: HashMap<usize, String>,
    /// (graph, line) of each job that ran, for the glow.
    ran: Vec<(String, usize)>,
}

/// The note texts a run reads: every note the index touches.
fn run_files(ws: &mut Workspace) -> HashMap<String, String> {
    let open = ws.note.as_ref().map(|n| (n.rel.to_string(), n.text()));
    ws.runs
        .index
        .files()
        .into_iter()
        .filter_map(|rel| {
            let text = match &open {
                Some((r, t)) if *r == rel => t.clone(),
                _ => ws.store.get(&rel).ok()?.to_owned(),
            };
            Some((rel, text))
        })
        .collect()
}

/// The vault-root settings note.
const SETTINGS_NOTE: &str = "_settings.md";

impl Workspace {
    /// Reads the team format from `_settings.md` (through the store, so the
    /// file is read at most once). Returns its warnings.
    fn load_team(&mut self) -> Vec<String> {
        let has_settings = self.vault.notes().any(|e| &*e.rel == SETTINGS_NOTE);
        let lookup = if has_settings {
            self.store
                .get(SETTINGS_NOTE)
                .ok()
                .and_then(|text| status::from_frontmatter(text.lines()))
        } else {
            None
        };
        let lookup = lookup.unwrap_or_default();
        self.team = lookup.format;
        self.crew_folder = if has_settings {
            self.store.get(SETTINGS_NOTE).map(crew::folder_setting).unwrap_or_else(|_| crew::FOLDER.to_owned())
        } else {
            crew::FOLDER.to_owned()
        };
        lookup.warnings
    }
}

struct App {
    workspace: Option<Workspace>,
    mode: Mode,
    switcher: Option<Switcher>,
    new_note: Option<NewNote>,
    menu: Option<StatusMenu>,
    window: Size,
    status: Option<String>,
    scanning: bool,
    next_note_id: u64,
    show_backlinks: bool,
    /// The open `_nodes.md`, as a graph.
    graph: Option<graph::Graph>,
    show_graph: bool,
    show_link_wires: bool,
    /// The quick switcher is picking a note for the graph, not opening one.
    adding_node: bool,
    /// Bumped when a different graph opens, so the view fits it.
    graph_fit: u64,
    /// The open graph's functions and the saved ones.
    functions: Library,
    /// The function being edited (shown instead of the graph).
    editor: Option<Editor>,
    editor_fit: u64,
    /// Choosing a function for a wire or a function node.
    picker: Option<(Picker, PickTarget)>,
    /// `@name` suggestions for what's being typed.
    completion: Option<Completion>,
    show_hand: bool,
    /// A crew card dropped on a node: choosing which task.
    assigning: Option<Assigning>,
    /// "Remove the link to [[b]] from a?", asked before a link wire goes.
    unlinking: Option<Unlinking>,
    /// The Settings page, while open.
    settings: Option<SettingsForm>,
    /// Vault search, while open.
    find: Option<find::Find>,
    /// Decoded pictures for the note on screen (and recent ones).
    pictures: Pictures,
}

/// A search index built in the background, handed over once.
#[derive(Clone)]
pub struct Built(Arc<std::sync::Mutex<Option<obsession_core::search::SearchIndex>>>);

impl std::fmt::Debug for Built {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Built")
    }
}

/// The Settings page: vault-wide settings, saved in `_settings.md`.
struct SettingsForm {
    crew: String,
    compact: bool,
    /// Why the picked folder can't be used.
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    Open,
    Crew(String),
    /// Open the folder picker.
    Browse,
    /// The folder picker closed.
    Picked(Option<PathBuf>),
    Compact(bool),
    Save,
    Close,
}

/// A link wire about to be removed: the links in `source` that lead to
/// `target`.
struct Unlinking {
    source: Box<str>,
    target: Box<str>,
    count: usize,
}

struct Assigning {
    member: String,
    card: usize,
    /// (row, task text, already mentions them)
    tasks: Vec<(usize, String, bool)>,
    title: String,
}

/// What the function picker is choosing for.
#[derive(Debug, Clone)]
enum PickTarget {
    /// A wire: its line in `## Wires` (`None` for a link wire, which gets a
    /// line when it gets a function) and its ends.
    Wire { line: Option<usize>, from: End, to: End },
    /// A new `fn Name` node.
    Node,
}

/// An end as written in a wire line.
fn end_text(end: &End) -> String {
    match end {
        End::Note { target, sub: Some(sub) } => format!("[[{target}#{sub}]]"),
        End::Note { target, sub: None } => format!("[[{target}]]"),
        End::Pin { function, pin } => format!("{function}.{pin}"),
    }
}

fn end_kind(end: &End) -> func::PortKind {
    match end {
        End::Note { sub, .. } => func_editor::kind_of(sub.as_deref()),
        End::Pin { .. } => func::PortKind::File,
    }
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        theme::set_compact(settings::prefs().compact);
        let app = Self {
            workspace: None,
            mode: Mode::default(),
            switcher: None,
            new_note: None,
            menu: None,
            window: Size::new(window_size().0, window_size().1),
            status: None,
            scanning: false,
            next_note_id: 0,
            show_backlinks: true,
            graph: None,
            show_graph: true,
            show_link_wires: true,
            adding_node: false,
            graph_fit: 0,
            functions: Library::default(),
            editor: None,
            editor_fit: 0,
            picker: None,
            completion: None,
            show_hand: true,
            assigning: None,
            unlinking: None,
            settings: None,
            find: None,
            pictures: Pictures::new(),
        };
        let scale = window::latest().and_then(window::scale_factor).map(Message::ScaleFactor);
        let start = std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(settings::last_vault);
        match start {
            Some(path) => {
                let mut app = app;
                let task = app.scan(path);
                (app, Task::batch([scale, task]))
            }
            None => (app, scale),
        }
    }

    fn title(&self) -> String {
        match &self.workspace {
            Some(ws) => match &ws.note {
                Some(note) => format!("{} – {} – Obsession", note.title(), ws.vault.name()),
                None => format!("{} – Obsession", ws.vault.name()),
            },
            None => "Obsession".into(),
        }
    }

    /// Looks up the pictures near the visible part of the note on screen
    /// and starts decoding the ones not held yet (each off the UI thread).
    fn want_pictures(&mut self) -> Task<Message> {
        let Some(ws) = &self.workspace else {
            return Task::none();
        };
        if !self.pictures.ready() {
            return Task::none();
        }
        let graph = self.show_graph && self.graph.is_some();
        let showing = self.mode != Mode::Source && !graph;
        let near = match &ws.note {
            Some(note) if showing => note.pictures_near(),
            _ => Vec::new(),
        };
        let from = ws.note.as_ref().map(|n| n.rel.to_string());
        let mut targets = HashMap::new();
        let mut keys = Vec::new();
        for (target, width) in near {
            let rel: Option<Box<str>> = ws.vault.attachment(&target, from.as_deref()).map(Box::from);
            if let Some(rel) = &rel {
                keys.push(pictures::Key { rel: rel.clone(), width, square: false });
            }
            targets.insert(target, rel);
        }
        self.pictures.set_targets(targets);
        // Portraits: the graph's crew, and the open crew card's own.
        let header = ws
            .crew
            .iter()
            .find(|m| showing && from.as_deref().is_some_and(|f| m.rel.eq_ignore_ascii_case(f)))
            .and_then(|m| portrait_of(&ws.vault, m));
        let faces = match &self.graph {
            Some(g) if graph => g.portraits(),
            _ => Vec::new(),
        };
        keys.extend(faces.iter().chain(&header).map(|rel| pictures::Key::portrait(rel.clone())));
        self.pictures.set_header(header);
        let start = self.pictures.want(keys);
        if let Some(g) = &mut self.graph {
            let decoded = faces.into_iter().filter_map(|rel| Some((rel.clone(), self.pictures.portrait(&rel)?))).collect();
            g.set_faces(decoded);
        }
        let (scale, generation) = (self.pictures.scale(), self.pictures.generation());
        let timing = std::env::var_os("OBSESSION_DEV_TIMING").is_some();
        Task::batch(start.into_iter().map(|key| {
            let path = ws.vault.path(&key.rel);
            let tag = key.clone();
            Task::perform(
                async move {
                    let started = std::time::Instant::now();
                    let result = pictures::decode(path, key.width, scale, key.square);
                    if timing && let Ok(d) = &result {
                        eprintln!(
                            "TIMING picture {}: {} ms, {}x{} kept as {:.1} MB",
                            key.rel,
                            started.elapsed().as_millis(),
                            d.natural.0,
                            d.natural.1,
                            d.bytes as f64 / 1048576.0
                        );
                    }
                    result
                },
                move |result| Message::PictureLoaded(tag.clone(), generation, result),
            )
        }))
    }

    fn scan(&mut self, path: PathBuf) -> Task<Message> {
        self.scanning = true;
        Task::perform(
            async move {
                let started = std::time::Instant::now();
                let vault = Vault::scan(path).map_err(|e| e.to_string());
                if std::env::var_os("OBSESSION_DEV_TIMING").is_some()
                    && let Ok(v) = &vault
                {
                    eprintln!(
                        "TIMING scan: {} ms, {} notes, {} attachments",
                        started.elapsed().as_millis(),
                        v.notes().count(),
                        v.files.len()
                    );
                }
                vault
            },
            Message::Scanned,
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        let task = Task::batch([task, self.want_pictures()]);
        // Split mode: scroll the read-only view to stay level with the editor.
        match self.note_mut().and_then(Note::take_mirror_scroll) {
            Some(y) => Task::batch([
                task,
                operation::scroll_to(
                    live::scroll_id(),
                    iced::widget::scrollable::AbsoluteOffset { x: 0.0, y },
                ),
            ]),
            None => task,
        }
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ScaleFactor(scale) => {
                self.pictures.set_scale(scale);
                smooth::set_scale(scale);
                Task::none()
            }
            Message::PictureLoaded(key, generation, result) => {
                self.pictures.loaded(key, generation, result);
                if std::env::var_os("OBSESSION_DEV_TIMING").is_some() {
                    let (bytes, count) = self.pictures.held();
                    eprintln!("TIMING pictures held: {:.1} MB in {count}", bytes as f64 / 1048576.0);
                }
                Task::none()
            }
            Message::PickFolder | Message::Shortcut(Shortcut::OpenFolder) => {
                let save = self.save_now();
                let pick = Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Open a vault folder")
                            .pick_folder()
                            .await
                            .map(|handle| handle.path().to_path_buf())
                    },
                    Message::FolderPicked,
                );
                Task::batch([save, pick])
            }
            Message::FolderPicked(Some(path)) => self.scan(path),
            Message::FolderPicked(None) => Task::none(),
            Message::Scanned(Ok(vault)) => {
                self.scanning = false;
                // Leaving the old vault, if any.
                self.save_views();
                settings::remember_vault(&vault.root);
                let store = Store::new(&vault.root);
                let tree = Tree::new(&vault);
                let runs = runs::Runs::new(&vault.root);
                let root_for_views = vault.root.clone();
                self.status = Some(format!("{} notes", vault.notes().count()));
                // Read every note's links on a background thread.
                let root = vault.root.clone();
                let rels: Vec<Box<str>> = vault.notes().map(|e| e.rel.clone()).collect();
                // Notes unchanged since the last time come from the cache in
                // `.obsessed/cache/`; only changed ones are read.
                let index = Task::perform(
                    async move {
                        let started = std::time::Instant::now();
                        let cache = std::fs::read_to_string(root.join(links::CACHE)).ok();
                        let (index, stats) = LinkIndex::build_cached(&root, rels.iter().map(|r| &**r), cache.as_deref());
                        if stats.parsed > 0 {
                            let _ = links::save_cache(&root, &index);
                        }
                        if std::env::var_os("OBSESSION_DEV_TIMING").is_some() {
                            eprintln!(
                                "TIMING index: {} ms, {} read, {} from cache",
                                started.elapsed().as_millis(),
                                stats.parsed,
                                stats.cached
                            );
                        }
                        (root, Arc::new(index))
                    },
                    |(root, index)| Message::Indexed(root, index),
                );
                self.pictures.clear();
                let mut ws = Workspace {
                    vault,
                    store,
                    tree,
                    note: None,
                    team: None,
                    links: LinkIndex::default(),
                    indexed: false,
                    backlinks: Vec::new(),
                    runs,
                    links_dirty: false,
                    search: None,
                    search_building: false,
                    crew_folder: crew::FOLDER.to_owned(),
                    views: Views::load(&root_for_views),
                    elsewhere: Elsewhere::default(),
                    crew: Vec::new(),
                    mentions: HashMap::new(),
                    outside: OutsideQueue::default(),
                    rescans: Rescans::default(),
                };
                if let Some(warning) = ws.load_team().into_iter().next() {
                    self.status = Some(format!("_settings.md: {warning}"));
                }
                ws.load_crew();
                self.workspace = Some(ws);
                self.rebuild_index();
                self.menu = None;
                self.switcher = None;
                self.new_note = None;
                #[cfg(debug_assertions)]
                return Task::batch([index, dev::after_scan(self)]);
                #[cfg(not(debug_assertions))]
                index
            }
            Message::Indexed(root, index) => {
                let Some(ws) = &mut self.workspace else {
                    return Task::none();
                };
                if ws.vault.root != root {
                    return Task::none();
                }
                let mut index = Arc::try_unwrap(index).unwrap_or_else(|shared| (*shared).clone());
                // The open note may have changed while the vault was read.
                if let Some(note) = &ws.note {
                    index.update(&note.rel, &note.text());
                }
                ws.links = index;
                ws.indexed = true;
                self.refresh_links();
                self.rebuild_graph();
                Task::none()
            }
            Message::Backlinks(backlinks::Message::Open(rel, line)) => {
                self.open_at(rel, Some(Jump::Line(line)))
            }
            Message::Settle => Task::none(),
            Message::Graph(message) => self.graph_message(message),
            Message::ToggleGraph => {
                self.show_graph = !self.show_graph;
                if !self.show_graph {
                    self.save_views();
                }
                if self.show_graph {
                    self.rebuild_graph();
                }
                Task::none()
            }
            Message::Settings(message) => self.settings_message(message),
            Message::Find(message) => self.find_message(message),
            Message::SearchBuilt(root, built) => {
                let Some(ws) = &mut self.workspace else {
                    return Task::none();
                };
                if ws.vault.root != root {
                    return Task::none();
                }
                ws.search_building = false;
                ws.search = built.0.lock().ok().and_then(|mut b| b.take());
                // The open note may hold newer text than the disk.
                if let (Some(search), Some(note)) = (&mut ws.search, &ws.note) {
                    search.update(&note.rel, &note.text());
                }
                if let Some(find) = &mut self.find {
                    find.refresh(ws.search.as_ref());
                }
                Task::none()
            }
            Message::ToggleHand => {
                self.show_hand = !self.show_hand;
                if let Some(graph) = &mut self.graph {
                    graph.show_hand = self.show_hand;
                    graph.touch();
                }
                Task::none()
            }
            Message::Unlink(remove) => match self.unlinking.take() {
                Some(u) if remove => self.unlink(&u),
                _ => Task::none(),
            },
            Message::AssignTo(None) => {
                self.assigning = None;
                Task::none()
            }
            Message::AssignTo(Some((card, row))) => {
                let Some(a) = self.assigning.take() else {
                    return Task::none();
                };
                self.assign(&a.member, card, row)
            }
            Message::ToggleLinkWires => {
                self.show_link_wires = !self.show_link_wires;
                if let Some(graph) = &mut self.graph {
                    graph.show_links = self.show_link_wires;
                    graph.touch();
                }
                Task::none()
            }
            Message::AddFunctionNode => {
                self.open_picker(PickTarget::Node, None);
                Task::none()
            }
            Message::Editor(message) => self.editor_message(message),
            Message::Picker(message) => self.picker_message(message),
            Message::AddNode => {
                let Some(ws) = &self.workspace else {
                    return Task::none();
                };
                self.adding_node = true;
                self.switcher = Some(Switcher::new(&ws.vault));
                operation::focus(switcher::input_id())
            }
            Message::ToggleBacklinks => {
                if let Some(ws) = &mut self.workspace {
                    ws.runs.show = false;
                }
                self.show_backlinks = !self.show_backlinks;
                Task::none()
            }
            Message::Rescanned(result, rel) => {
                let Some(ws) = &mut self.workspace else {
                    return Task::none();
                };
                match result {
                    Ok(vault) if vault.root == ws.vault.root => {
                        ws.vault = vault;
                        ws.tree.refresh(&ws.vault);
                        ws.load_crew();
                        // A new note: empty, so nothing to read for its links.
                        if ws.links.outline(&rel).is_none() {
                            ws.links.update(&rel, "");
                            if let Some(search) = &mut ws.search { search.update(&rel, ""); }
                        }
                        let open = self.open(rel);
                        // Links to it from the note we came from are fine now.
                        self.refresh_links();
                        open
                    }
                    Ok(_) => Task::none(),
                    Err(error) => {
                        self.status = Some(format!("Couldn't re-read the folder: {error}"));
                        Task::none()
                    }
                }
            }
            Message::StartNewNote => self.shortcut(Shortcut::NewNote),
            Message::NewNote(message) => self.new_note_message(message),
            Message::Scanned(Err(error)) => {
                self.scanning = false;
                self.status = Some(format!("Couldn't open the folder: {error}"));
                Task::none()
            }
            Message::Sidebar(message) => {
                let Some(ws) = &mut self.workspace else {
                    return Task::none();
                };
                match message {
                    sidebar::Message::Toggle(index) => {
                        ws.tree.toggle(&ws.vault, index);
                        Task::none()
                    }
                    sidebar::Message::Scrolled(viewport) => {
                        ws.tree.scrolled(viewport);
                        Task::none()
                    }
                    sidebar::Message::Open(index) => self.open_index(index),
                    sidebar::Message::Rename(index) => match ws.vault.entries.get(index as usize) {
                        Some(entry) if entry.kind == Kind::Note => {
                            let rel = entry.rel.clone();
                            self.start_rename(&rel)
                        }
                        _ => Task::none(),
                    },
                }
            }
            Message::Edit(action) => {
                let Some(ws) = &mut self.workspace else {
                    return Task::none();
                };
                let Some(note) = &mut ws.note else {
                    return Task::none();
                };
                let performed = note.perform(action, ws.team.as_ref());
                let id = note.id;
                if performed.message.is_some() {
                    self.status = performed.message;
                }
                self.update_completion();
                start_timers(id, performed.timers)
            }
            Message::CompleteMention(key) => {
                let Some(done) = self.completion.take() else {
                    return Task::none();
                };
                let Some(key) = key.or_else(|| done.matches.first().map(|m| m.0.clone())) else {
                    return Task::none();
                };
                let Some(ws) = &mut self.workspace else {
                    return Task::none();
                };
                let Some(note) = &mut ws.note else {
                    return Task::none();
                };
                note.set_completing(false);
                let performed = note.complete_mention(done.typed.chars().count(), &key, ws.team.as_ref());
                let id = note.id;
                let focus = match self.mode {
                    Mode::Source | Mode::Split => Task::none(),
                    _ => operation::focus(live::editor_id()),
                };
                Task::batch([start_timers(id, performed.timers), focus])
            }
            Message::EditorRightClick(local, window) => {
                let Some(note) = self.note_mut() else {
                    return Task::none();
                };
                self.menu = note
                    .task_at_point(local)
                    .map(|task| StatusMenu { task, at: window });
                Task::none()
            }
            Message::StatusMenu(message) => {
                let Some(menu) = self.menu.take() else {
                    return Task::none();
                };
                match message {
                    status_menu::Message::Close => Task::none(),
                    status_menu::Message::Pick(ch) => {
                        let Some(note) = self.note_mut() else {
                            return Task::none();
                        };
                        // The line may have changed since the menu opened.
                        if note.task_at_line(menu.task.line) != Some(menu.task) {
                            return Task::none();
                        }
                        let id = note.id;
                        let timers = note.set_status(menu.task, ch);
                        start_timers(id, timers)
                    }
                }
            }
            Message::Resized(size) => {
                self.window = size;
                Task::none()
            }
            Message::Timer(id, timer) => {
                let mode = self.mode;
                let Some(note) = self.note_mut().filter(|n| n.id == id) else {
                    return Task::none();
                };
                match note.timer_fired(timer) {
                    Err(left) => {
                        Task::perform(timer::after(left), move |()| Message::Timer(id, timer))
                    }
                    Ok(()) => match timer {
                        Timer::Preview => {
                            if mode.shows_preview() {
                                note.refresh_preview_if_stale();
                            }
                            // Links typed since: are they there?
                            self.refresh_links();
                            Task::none()
                        }
                        Timer::Save => self.save_now(),
                    },
                }
            }
            Message::Fs(events) => self.files_changed(events),
            Message::DrainOutside => self.drain_outside(),
            Message::OutsideRead(texts) => self.outside_read(texts),
            Message::Rescan => {
                if let Some(ws) = &mut self.workspace {
                    ws.rescans.scheduled = false;
                }
                self.rescan()
            }
            Message::VaultRefreshed(Ok(vault)) => {
                let again = self.rescan_landed();
                let Some(ws) = &mut self.workspace else {
                    return again;
                };
                if ws.vault.root != vault.root {
                    return again;
                }
                let started = std::time::Instant::now();
                ws.vault = vault;
                ws.tree.refresh(&ws.vault);
                ws.load_crew();
                let crew = started.elapsed();
                self.pictures.forget_failed();
                self.rebuild_index();
                let index = started.elapsed();
                self.refresh_links();
                let links = started.elapsed();
                self.rebuild_graph();
                if std::env::var_os("OBSESSION_DEV_TIMING").is_some() {
                    eprintln!(
                        "TIMING rescan landed: tree+crew {} ms, index {} ms, links {} ms, all {} ms",
                        crew.as_millis(),
                        (index - crew).as_millis(),
                        (links - index).as_millis(),
                        started.elapsed().as_millis()
                    );
                }
                again
            }
            Message::VaultRefreshed(Err(error)) => {
                self.status = Some(format!("Couldn't re-read the folder: {error}"));
                self.rescan_landed()
            }
            Message::RunNow => {
                let Some(ws) = &mut self.workspace else {
                    return Task::none();
                };
                ws.runs.scheduled = false;
                ws.runs.running = true;
                // Typing in the open note goes into this run.
                let save = self.save_now();
                let Some(ws) = &mut self.workspace else {
                    return save;
                };
                let changed = std::mem::take(&mut ws.runs.pending);
                let start = ws.runs.index.affected(changed.iter().map(String::as_str));
                if start.is_empty() {
                    ws.runs.running = false;
                    return save;
                }
                Task::batch([save, self.start_run(start)])
            }
            Message::RunDone(result) => self.run_done(&result),
            Message::Runs(runs::Message::Undo(id)) => self.undo_run(id),
            Message::Runs(runs::Message::Close) | Message::ToggleRuns => {
                if let Some(ws) = &mut self.workspace {
                    ws.runs.show = !ws.runs.show;
                }
                Task::none()
            }
            Message::RunGraph => {
                let Some(ws) = &mut self.workspace else {
                    return Task::none();
                };
                if ws.runs.running {
                    self.status = Some("A run is going; try again in a moment".into());
                    return Task::none();
                }
                let Some(graph) = ws.note.as_ref().map(|n| n.rel.to_string()) else {
                    return Task::none();
                };
                let start: Vec<usize> = ws
                    .runs
                    .index
                    .jobs
                    .iter()
                    .enumerate()
                    .filter(|(_, j)| ws.runs.out_of_date.contains_key(&(graph.clone(), j.line)) && j.graph == graph)
                    .map(|(i, _)| i)
                    .collect();
                if start.is_empty() {
                    return Task::none();
                }
                ws.runs.running = true;
                let save = self.save_now();
                Task::batch([save, self.start_run(start)])
            }
            Message::Saved(result) => {
                if let Some(ws) = &mut self.workspace {
                    ws.store.finish_save(&result);
                    // The index holds what was just written: the cache can
                    // trust it next time.
                    if result.error().is_none() {
                        let root = ws.vault.root.clone();
                        ws.links.stamp(&root, result.rel());
                        ws.links_dirty = true;
                    }
                }
                if let Some(error) = result.error() {
                    self.status = Some(format!("Couldn't save {}: {error}", result.rel()));
                }
                Task::none()
            }
            Message::SetMode(mode) => {
                if self.show_graph {
                    self.save_views();
                }
                self.show_graph = false;
                self.set_mode(mode)
            }
            Message::Live(live::Message::Key(live::Intent::Complete)) => self.handle(Message::CompleteMention(None)),
            Message::Live(message) => {
                let typing = matches!(message, live::Message::Edit(_) | live::Message::Key(_) | live::Message::Press { .. });
                let task = self.live_message(message);
                if typing {
                    self.update_completion();
                }
                task
            }
            Message::Shortcut(shortcut) => self.shortcut(shortcut),
            Message::Switcher(message) => self.switcher_message(message),
            Message::CloseRequested(id) => {
                self.save_blocking();
                window::close(id)
            }
            #[cfg(debug_assertions)]
            Message::Dev(dev::Step::Later(message)) => self.update(*message),
            #[cfg(debug_assertions)]
            Message::Dev(dev::Step::Follow(uri)) => self.follow_link(&uri),
            #[cfg(debug_assertions)]
            Message::Dev(dev::Step::UndoRun) => match self.workspace.as_ref().and_then(|ws| ws.runs.undoable()) {
                Some(id) => self.undo_run(id),
                None => Task::none(),
            },
            #[cfg(debug_assertions)]
            Message::Dev(dev::Step::AddNode(name)) => {
                let index = self.workspace.as_ref().and_then(|ws| {
                    ws.vault.entries.iter().position(|e| e.kind == Kind::Note && e.title() == name)
                });
                match index {
                    Some(index) => {
                        self.adding_node = true;
                        self.picked(index as u32)
                    }
                    None => Task::none(),
                }
            }
            #[cfg(debug_assertions)]
            Message::Dev(step) => dev::update(step),
        }
    }

    fn note_mut(&mut self) -> Option<&mut Note> {
        self.workspace.as_mut()?.note.as_mut()
    }

    fn set_mode(&mut self, mode: Mode) -> Task<Message> {
        self.mode = mode;
        let Some(note) = self.note_mut() else {
            return Task::none();
        };
        note.set_mode(mode);
        if mode == Mode::Live && note.is_live_editing() {
            operation::focus(live::editor_id())
        } else {
            Task::none()
        }
    }

    fn undo(&mut self, redo: bool) -> Task<Message> {
        // A change made from here to other notes goes first when it's the
        // newest (the open note is as it was then).
        if let Some(open) = self.open_fingerprint()
            && let Some(ws) = &mut self.workspace
        {
            let step = if redo { ws.elsewhere.redo(open) } else { ws.elsewhere.undo(open) };
            if let Some(step) = step {
                let (task, done) = self.undo_elsewhere(&step, redo);
                if redo && done && step.joined {
                    // Made with the open note's own edit: that comes next.
                    return Task::batch([task, self.undo_open(true)]);
                }
                return task;
            }
        }
        let task = self.undo_open(redo);
        if !redo
            && let Some(open) = self.open_fingerprint()
            && let Some(step) = self.workspace.as_mut().and_then(|ws| ws.elsewhere.undo_joined(open))
        {
            let (more, _) = self.undo_elsewhere(&step, false);
            return Task::batch([task, more]);
        }
        task
    }

    /// The open note's text, fingerprinted for `ws.elsewhere`.
    fn open_fingerprint(&self) -> Option<u64> {
        Some(history::fingerprint(&self.workspace.as_ref()?.note.as_ref()?.text()))
    }

    /// Undoes (or redoes) the open note's own last edit.
    fn undo_open(&mut self, redo: bool) -> Task<Message> {
        let mode = self.mode;
        let Some(note) = self.note_mut() else {
            return Task::none();
        };
        let performed = note.undo(redo);
        let id = note.id;
        let graph = nodes::is_graph(&note.rel);
        let focus = mode == Mode::Live && note.is_live_editing();
        if mode.shows_preview() {
            note.refresh_preview_if_stale();
        }
        if performed.message.is_some() {
            self.status = performed.message;
        }
        if graph {
            self.rebuild_graph();
        }
        let timers = start_timers(id, performed.timers);
        if focus {
            Task::batch([timers, operation::focus(live::editor_id())])
        } else {
            timers
        }
    }

    /// Undoes (or redoes) a change made to other notes, if they all still
    /// hold what the change left. Says whether it did.
    fn undo_elsewhere(&mut self, step: &history::Step, redo: bool) -> (Task<Message>, bool) {
        for f in &step.files {
            let expect = if redo { &f.before } else { &f.after };
            if self.current_text(&f.rel).as_ref() != Some(expect) {
                let name = f.rel.trim_end_matches(".md");
                self.status = Some(format!("{name} changed since, so that can't be undone from here"));
                return (Task::none(), false);
            }
        }
        self.status = Some(format!("{}: {}", if redo { "Redone" } else { "Undone" }, step.what));
        let puts: Vec<(String, String)> = step
            .files
            .iter()
            .map(|f| (f.rel.clone(), if redo { f.after.clone() } else { f.before.clone() }))
            .collect();
        (self.put_files(&puts), true)
    }

    fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        if self.menu.take().is_some() && matches!(shortcut, Shortcut::Escape) {
            return Task::none();
        }
        if self.picker.is_some() && matches!(shortcut, Shortcut::Escape) {
            self.picker = None;
            return Task::none();
        }
        if self.settings.is_some() && matches!(shortcut, Shortcut::Escape) {
            self.settings = None;
            return Task::none();
        }
        if self.unlinking.is_some() && matches!(shortcut, Shortcut::Escape) {
            self.unlinking = None;
            return Task::none();
        }
        if self.assigning.is_some() && matches!(shortcut, Shortcut::Escape) {
            self.assigning = None;
            return Task::none();
        }
        if self.completion.is_some() && matches!(shortcut, Shortcut::Escape) {
            self.completion = None;
            if let Some(note) = self.note_mut() {
                note.set_completing(false);
            }
        }
        // The editors' own Tab binding completes when it sees the key; this
        // catches a Tab they didn't take.
        if matches!(shortcut, Shortcut::Tab) {
            return match &self.completion {
                Some(c) if !c.matches.is_empty() && self.new_note.is_none() && self.find.is_none() && self.switcher.is_none() => {
                    self.handle(Message::CompleteMention(None))
                }
                _ => Task::none(),
            };
        }
        if self.new_note.is_some() {
            if matches!(shortcut, Shortcut::Escape | Shortcut::NewNote) {
                self.new_note = None;
            }
            return Task::none();
        }
        if let Some(find) = &mut self.find {
            match shortcut {
                Shortcut::Up => find.move_selection(false),
                Shortcut::Down => find.move_selection(true),
                Shortcut::Escape | Shortcut::FindInVault => self.find = None,
                _ => {}
            }
            return Task::none();
        }
        if let Some(switcher) = &mut self.switcher {
            match shortcut {
                Shortcut::Up => switcher.move_selection(false),
                Shortcut::Down => switcher.move_selection(true),
                Shortcut::Escape | Shortcut::QuickSwitch => {
                    self.switcher = None;
                    // It may have been opened by + Node.
                    self.adding_node = false;
                }
                _ => {}
            }
            return Task::none();
        }
        match shortcut {
            Shortcut::FindInVault => self.open_find(),
            Shortcut::QuickSwitch => {
                let Some(ws) = &self.workspace else {
                    return Task::none();
                };
                // Ctrl+P opens a note; only + Node adds one.
                self.adding_node = false;
                self.switcher = Some(Switcher::new(&ws.vault));
                operation::focus(switcher::input_id())
            }
            Shortcut::NewNote => {
                let Some(ws) = &self.workspace else {
                    return Task::none();
                };
                // Start in the open note's folder.
                let folder = ws
                    .note
                    .as_ref()
                    .and_then(|n| n.rel.rsplit_once('/').map(|(dir, _)| dir))
                    .unwrap_or("");
                self.new_note = Some(NewNote::new(folder));
                Task::batch([
                    operation::focus(new_note::input_id()),
                    operation::move_cursor_to_end(new_note::input_id()),
                ])
            }
            Shortcut::CycleMode => {
                self.show_graph = false;
                self.set_mode(self.mode.next())
            }
            Shortcut::Rename => {
                match self.workspace.as_ref().and_then(|ws| ws.note.as_ref()).map(|n| n.rel.clone()) {
                    Some(rel) => self.start_rename(&rel),
                    None => Task::none(),
                }
            }
            Shortcut::Undo => self.undo(false),
            Shortcut::Redo => self.undo(true),
            Shortcut::Save => self.save_now(),
            Shortcut::OpenFolder => self.update(Message::PickFolder),
            Shortcut::Copy => match self.workspace.as_ref().and_then(|ws| ws.note.as_ref()).and_then(Note::selection_text) {
                Some(text) => iced::clipboard::write(text),
                None => Task::none(),
            },
            Shortcut::Escape => {
                if let Some(note) = self.note_mut() {
                    note.clear_selection();
                }
                Task::none()
            }
            Shortcut::Up | Shortcut::Down | Shortcut::Tab => Task::none(),
        }
    }

    /// Opens vault search, building its index the first time.
    fn open_find(&mut self) -> Task<Message> {
        if self.workspace.is_none() {
            return Task::none();
        }
        let save = self.save_now();
        let Some(ws) = &mut self.workspace else {
            return save;
        };
        let mut find = find::Find::default();
        find.refresh(ws.search.as_ref());
        self.find = Some(find);
        let focus = operation::focus(find::input_id());
        if ws.search.is_some() || ws.search_building {
            return Task::batch([save, focus]);
        }
        ws.search_building = true;
        let root = ws.vault.root.clone();
        let rels: Vec<Box<str>> = ws.vault.notes().map(|e| e.rel.clone()).collect();
        let build = Task::perform(
            async move {
                let started = std::time::Instant::now();
                let index = obsession_core::search::SearchIndex::build(&root, rels.iter().map(|r| &**r));
                if std::env::var_os("OBSESSION_DEV_TIMING").is_some() {
                    eprintln!("TIMING search: {} notes in {} ms, {} KB", index.len(), started.elapsed().as_millis(), index.bytes() / 1024);
                }
                (root, Built(Arc::new(std::sync::Mutex::new(Some(index)))))
            },
            |(root, built)| Message::SearchBuilt(root, built),
        );
        Task::batch([save, focus, build])
    }

    fn find_message(&mut self, message: find::Message) -> Task<Message> {
        let Some(find) = &mut self.find else {
            return Task::none();
        };
        let pick = match message {
            find::Message::Query(query) => {
                let index = self.workspace.as_ref().and_then(|ws| ws.search.as_ref());
                find.set_query(index, query);
                return Task::none();
            }
            find::Message::Close => {
                self.find = None;
                return Task::none();
            }
            find::Message::Submit => find.selected().cloned(),
            find::Message::Pick(i) => find.hit(i).cloned(),
        };
        let Some(hit) = pick else {
            return Task::none();
        };
        self.find = None;
        self.open_at(hit.rel, Some(Jump::Line(hit.line)))
    }

    fn switcher_message(&mut self, message: switcher::Message) -> Task<Message> {
        let (Some(switcher), Some(ws)) = (&mut self.switcher, &self.workspace) else {
            return Task::none();
        };
        match message {
            switcher::Message::Query(query) => {
                switcher.set_query(&ws.vault, query);
                Task::none()
            }
            switcher::Message::Submit => match switcher.selected() {
                Some(index) => {
                    self.switcher = None;
                    self.picked(index)
                }
                None => Task::none(),
            },
            switcher::Message::Pick(index) => {
                self.switcher = None;
                self.picked(index)
            }
            switcher::Message::Close => {
                self.switcher = None;
                self.adding_node = false;
                Task::none()
            }
        }
    }

    fn start_rename(&mut self, rel: &str) -> Task<Message> {
        self.switcher = None;
        self.menu = None;
        self.new_note = Some(NewNote::rename(rel));
        Task::batch([
            operation::focus(new_note::input_id()),
            operation::move_cursor_to_end(new_note::input_id()),
        ])
    }

    fn new_note_message(&mut self, message: new_note::Message) -> Task<Message> {
        if let (new_note::Message::Submit, Some(form)) = (&message, &self.new_note)
            && let new_note::Purpose::Rename(old) = &form.purpose
        {
            let (old, input) = (old.clone(), form.input.clone());
            return match self.rename(&old, &input) {
                Ok(task) => {
                    self.new_note = None;
                    task
                }
                Err(error) => {
                    if let Some(form) = &mut self.new_note {
                        form.error = Some(error);
                    }
                    Task::none()
                }
            };
        }
        let (Some(form), Some(ws)) = (&mut self.new_note, &mut self.workspace) else {
            return Task::none();
        };
        match message {
            new_note::Message::Input(input) => {
                form.input = input;
                form.error = None;
                Task::none()
            }
            new_note::Message::Close => {
                self.new_note = None;
                Task::none()
            }
            new_note::Message::Submit => {
                let rel = match vault::note_rel_from_input(&form.input) {
                    Ok(rel) => rel,
                    Err(error) => {
                        form.error = Some(error.to_owned());
                        return Task::none();
                    }
                };
                match vault::create_note(&ws.vault.root, &rel) {
                    Ok(()) => {
                        // Known to the store at once, so the watcher's event
                        // for it isn't taken for another app's edit.
                        let _ = ws.store.get(&rel);
                        self.new_note = None;
                        self.status = Some(format!("Created {rel}"));
                        let root = ws.vault.root.clone();
                        let rel: Box<str> = rel.into();
                        Task::perform(
                            async move { Vault::scan(root).map_err(|e| e.to_string()) },
                            move |result| Message::Rescanned(result, rel.clone()),
                        )
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        if ws.vault.notes().any(|e| e.rel.eq_ignore_ascii_case(&rel)) {
                            self.new_note = None;
                            self.status = Some(format!("{rel} already exists, opened it"));
                            self.open(rel.into())
                        } else {
                            form.error = Some("Something with that name is already there".into());
                            Task::none()
                        }
                    }
                    Err(error) => {
                        form.error = Some(format!("Couldn't create it: {error}"));
                        Task::none()
                    }
                }
            }
        }
    }

    /// The quick switcher picked a note: open it, or add it to the graph.
    fn picked(&mut self, index: u32) -> Task<Message> {
        if !std::mem::take(&mut self.adding_node) {
            return self.open_index(index);
        }
        let Some(ws) = &self.workspace else {
            return Task::none();
        };
        let Some(entry) = ws.vault.entries.get(index as usize).filter(|e| e.kind == Kind::Note) else {
            return Task::none();
        };
        if self.graph.as_ref().is_some_and(|g| g.cards.iter().any(|c| c.rel.as_deref() == Some(&*entry.rel))) {
            self.status = Some(format!("{} is already in this graph", entry.title()));
            return Task::none();
        }
        // The shortest link that still finds it.
        let name = entry.title();
        let key = if ws.vault.count_named(name) == 1 {
            name.to_owned()
        } else {
            entry.rel.trim_end_matches(".md").to_owned()
        };
        self.show_graph = true;
        self.edit_graph(|text| nodes::add_item(text, Section::Nodes, &format!("[[{key}]]")), true)
    }

    /// (Re)reads the open `_nodes.md` into the graph view.
    fn rebuild_graph(&mut self) {
        let Some(ws) = &mut self.workspace else {
            self.graph = None;
            return;
        };
        let Some(note) = ws.note.as_ref().filter(|n| nodes::is_graph(&n.rel)) else {
            self.graph = None;
            return;
        };
        let text = note.text();
        let rel = note.rel.clone();
        let library = load_library(ws, &text);
        let previous = self.graph.take();
        let mut graph = graph::Graph::build(
            &text,
            &rel,
            &ws.vault,
            &mut ws.store,
            ws.team.as_ref(),
            &ws.links,
            &library,
            previous.as_ref(),
            self.show_link_wires,
        );
        graph.fit = self.graph_fit;
        graph.saved_view = ws.views.get(&views::graph_key(&rel));
        for w in &mut graph.wires {
            w.stale = w.line.and_then(|l| ws.runs.out_of_date.get(&(rel.to_string(), l)).cloned());
        }
        for c in &mut graph.cards {
            c.stale = c.line.and_then(|l| ws.runs.out_of_date.get(&(rel.to_string(), l)).cloned());
        }
        let portraits: Vec<Option<Box<str>>> = ws.crew.iter().map(|m| portrait_of(&ws.vault, m)).collect();
        graph.set_crew(&ws.crew, &portraits);
        graph.show_hand = self.show_hand;
        graph.pulses = ws
            .runs
            .glow
            .iter()
            .filter(|((g, _), _)| **g == *rel)
            .map(|((_, line), at)| (*line, *at))
            .collect();
        self.functions = library;
        self.graph = Some(graph);
        self.refresh_editor();
    }

    /// Opens the editor on a function.
    fn open_editor(&mut self, name: &str, context: Context) {
        let Some((block, origin, shadowed)) = self.functions.find(name) else {
            self.status = Some(format!("⚠ There's no function called {name}"));
            return;
        };
        let mut editor = Editor::new(block.function.name.clone(), origin, context);
        self.editor_fit += 1;
        editor.fit = self.editor_fit;
        editor.saved_view = self.function_view_key(&editor).and_then(|key| self.workspace.as_ref()?.views.get(&key));
        if shadowed {
            self.status = Some(format!("⚠ {name} in this graph hides the saved one in _funct.md"));
        }
        self.editor = Some(editor);
        self.refresh_editor();
    }

    /// Re-reads the edited function, its wire's values and the files it's
    /// wired to, then checks and runs it for the live values.
    fn refresh_editor(&mut self) {
        let Some(editor) = &self.editor else { return };
        let (name, origin, context) = (editor.name.clone(), editor.origin, editor.context.clone());
        let was = editor.block.as_ref().map(|b| (b.open, b.close));
        let Some(ws) = &mut self.workspace else { return };
        let Some(note) = ws.note.as_ref().filter(|n| nodes::is_graph(&n.rel)) else {
            self.editor = None;
            return;
        };
        let graph_text = note.text();
        let graph_rel = note.rel.clone();
        let pool = match origin {
            Origin::Local => &self.functions.local,
            Origin::Saved => &self.functions.saved,
        };
        let block = pool.iter().find(|b| b.function.name.eq_ignore_ascii_case(&name)).cloned().or_else(|| {
            // Renamed, or a rename undone: the block in the same place.
            let (open, close) = was?;
            pool.iter().find(|b| b.open == open && b.close == close).cloned()
        });
        if let Some(b) = &block
            && b.function.name != name
        {
            // Its view follows the new name.
            let file = match origin {
                Origin::Saved => func::SAVED.to_owned(),
                Origin::Local => graph_rel.to_string(),
            };
            ws.views.move_view(&views::function_key(&file, &name), &views::function_key(&file, &b.function.name));
        }
        let parsed = nodes::parse(&graph_text);
        let (ins, outs) = block
            .as_ref()
            .map_or((0, 0), |b| (b.function.inputs.len(), b.function.outputs.len()));
        let mut ends: Vec<Option<(String, Option<String>)>> = vec![None; ins + outs];
        let mut args = Vec::new();
        let mut label = None;
        let note_end = |e: &End| match e {
            End::Note { target, sub } => Some((target.clone(), sub.clone())),
            End::Pin { .. } => None,
        };
        match context {
            Context::Wire { line } => {
                if let Some(w) = parsed.wires.iter().find(|w| w.line == line) {
                    if let Some(Ok((_, a))) = w.function.as_deref().map(func::parse_call) {
                        args = a;
                    }
                    if ins == 1 {
                        ends[0] = note_end(&w.from);
                    }
                    if outs == 1 {
                        ends[ins] = note_end(&w.to);
                    }
                    label = Some(format!("{} → {}", end_text(&w.from), end_text(&w.to)));
                }
            }
            Context::Node => {
                if let Some(f) = block.as_ref().map(|b| &b.function) {
                    for w in &parsed.wires {
                        if let End::Pin { function, pin } = &w.to
                            && function.eq_ignore_ascii_case(&name)
                            && let Some(k) = f.inputs.iter().position(|p| p.name.eq_ignore_ascii_case(pin))
                        {
                            ends[k] = note_end(&w.from);
                        }
                        if let End::Pin { function, pin } = &w.from
                            && function.eq_ignore_ascii_case(&name)
                            && let Some(k) = f.outputs.iter().position(|p| p.name.eq_ignore_ascii_case(pin))
                        {
                            ends[ins + k] = note_end(&w.to);
                        }
                    }
                }
            }
        }
        let binds: Vec<Option<Bind>> = ends
            .into_iter()
            .map(|end| {
                let (target, sub) = end?;
                let entry = ws.vault.resolve_from(&target, Some(&graph_rel)).entry()?;
                let rel = entry.rel.clone();
                let text = ws.store.get(&rel).ok()?.to_owned();
                Some(Bind {
                    rel: rel.to_string(),
                    text,
                    sub,
                })
            })
            .collect();
        let team = ws.team.clone();
        if let Some(editor) = &mut self.editor {
            editor.wire_label = label;
            editor.refresh(block, &args, binds, team.as_ref());
        }
    }

    /// Changes the text holding the edited function: the graph, or
    /// `_funct.md` (an `Elsewhere` step). Either way Ctrl+Z undoes it.
    fn edit_function(&mut self, change: Box<dyn FnOnce(&str) -> String>) -> Task<Message> {
        let Some(editor) = &self.editor else {
            return Task::none();
        };
        match editor.origin {
            Origin::Local => self.edit_graph(change, true),
            Origin::Saved => {
                let what = format!("Edited {} in {}", editor.name, func::SAVED);
                let Some(text) = self.current_text(func::SAVED) else {
                    self.status = Some("⚠ Couldn't read _funct.md".into());
                    return Task::none();
                };
                let new = change(&text);
                if new == text {
                    return Task::none();
                }
                let file = history::Change {
                    rel: func::SAVED.to_owned(),
                    before: text,
                    after: new,
                };
                // Quiet like a graph edit; the step's words are for undo.
                let status = self.status.take();
                let task = self.edit_files(vec![file], what, false);
                self.status = status;
                task
            }
        }
    }

    /// Where a function's view is remembered: by the file that holds it.
    fn function_view_key(&self, editor: &Editor) -> Option<String> {
        let file = match editor.origin {
            Origin::Saved => func::SAVED.to_owned(),
            Origin::Local => self.workspace.as_ref()?.note.as_ref()?.rel.to_string(),
        };
        Some(views::function_key(&file, &editor.name))
    }

    fn editor_message(&mut self, message: func_editor::Message) -> Task<Message> {
        use func_editor::Outcome;
        if let func_editor::Message::Viewed(view) = message {
            let key = self.editor.as_ref().and_then(|e| self.function_view_key(e));
            if let (Some(key), Some(ws)) = (key, &mut self.workspace) {
                ws.views.set(&key, view);
            }
            if let Some(editor) = &mut self.editor {
                editor.saved_view = Some(view);
            }
            return Task::none();
        }
        let Some(editor) = &mut self.editor else {
            return Task::none();
        };
        let context = editor.context.clone();
        let name = editor.name.clone();
        match editor.update(message) {
            Outcome::None => Task::none(),
            Outcome::Edit(change) => self.edit_function(change),
            Outcome::Status(s) => {
                self.status = Some(s);
                Task::none()
            }
            Outcome::Back => {
                self.editor = None;
                Task::none()
            }
            Outcome::Wire(call) => match context {
                Context::Wire { line } => self.edit_graph(|text| nodes::set_wire_function(text, line, Some(&call)), true),
                Context::Node => Task::none(),
            },
            Outcome::Change => {
                if let Context::Wire { line } = context
                    && let Some(w) = self.graph_wire(line)
                {
                    self.open_picker(
                        PickTarget::Wire {
                            line: Some(line),
                            from: w.from,
                            to: w.to,
                        },
                        Some(name),
                    );
                }
                Task::none()
            }
            Outcome::TakeOff => match context {
                Context::Wire { line } => {
                    self.editor = None;
                    self.edit_graph(|text| nodes::set_wire_function(text, line, None), true)
                }
                Context::Node => Task::none(),
            },
            Outcome::Duplicate => self.duplicate_function(&name, context),
            Outcome::Rename(new) => self.rename_function(&new),
        }
    }

    /// Why `new` can't be the name of the function `old` (from `origin`):
    /// another function already has it where this one is used.
    fn name_taken(&mut self, old: &str, new: &str, origin: Origin) -> Option<String> {
        if new.eq_ignore_ascii_case(old) {
            return None;
        }
        let has = |blocks: &[func::Block]| blocks.iter().any(|b| b.function.name.eq_ignore_ascii_case(new));
        match origin {
            Origin::Local => {
                if has(&self.functions.local) {
                    return Some(format!("This graph already has a function called {new}"));
                }
                if has(&self.functions.saved) {
                    return Some(format!("_funct.md already has a function called {new}"));
                }
                None
            }
            Origin::Saved => {
                if has(&self.functions.saved) {
                    return Some(format!("_funct.md already has a function called {new}"));
                }
                // A graph's own function by that name would take its wires.
                for rel in self.graph_rels() {
                    if let Some(text) = self.current_text(&rel)
                        && has(&func::local_blocks(&text))
                    {
                        return Some(format!("{} has its own function called {new}", rel.trim_end_matches(".md")));
                    }
                }
                None
            }
        }
    }

    /// Every graph file in the vault.
    fn graph_rels(&self) -> Vec<String> {
        self.workspace.as_ref().map_or_else(Vec::new, |ws| {
            ws.vault
                .entries
                .iter()
                .filter(|e| e.kind == Kind::Note && nodes::is_graph(&e.rel))
                .map(|e| e.rel.to_string())
                .collect()
        })
    }

    /// Renames the function open in the editor, and every wire, node and
    /// layout line that uses it: in this graph for a local function, in
    /// every graph (except those with their own function by that name) for
    /// a saved one. Undoable in one step.
    fn rename_function(&mut self, new: &str) -> Task<Message> {
        let Some(editor) = &self.editor else {
            return Task::none();
        };
        let Some(block) = editor.block.clone() else {
            return Task::none();
        };
        let origin = editor.origin;
        let old = block.function.name.clone();
        if let Some(why) = self.name_taken(&old, new, origin) {
            self.status = Some(format!("⚠ {why}"));
            return Task::none();
        }
        let Some(open) = self.workspace.as_ref().and_then(|ws| ws.note.as_ref()).map(|n| (n.rel.to_string(), n.text())) else {
            return Task::none();
        };
        let (open_rel, open_text) = open;
        match origin {
            Origin::Local => {
                let task = self.edit_graph(
                    |text| nodes::rename_function(&func::edit::rename(text, &block, new, false), &old, new),
                    true,
                );
                self.status = Some(format!("Renamed {old} to {new} in this graph"));
                task
            }
            Origin::Saved => {
                let Some(saved) = self.current_text(func::SAVED) else {
                    self.status = Some("⚠ Couldn't read _funct.md".into());
                    return Task::none();
                };
                let mut files = vec![history::Change {
                    rel: func::SAVED.to_owned(),
                    after: func::edit::rename(&saved, &block, new, true),
                    before: saved,
                }];
                let own = |text: &str| func::local_blocks(text).iter().any(|b| b.function.name.eq_ignore_ascii_case(&old));
                for rel in self.graph_rels() {
                    if rel == open_rel {
                        continue;
                    }
                    let Some(text) = self.current_text(&rel) else { continue };
                    if own(&text) {
                        continue;
                    }
                    let after = nodes::rename_function(&text, &old, new);
                    if after != text {
                        files.push(history::Change { rel, before: text, after });
                    }
                }
                let open_after = if own(&open_text) { open_text.clone() } else { nodes::rename_function(&open_text, &old, new) };
                let joined = open_after != open_text;
                let graphs = files.len() - 1 + usize::from(joined);
                let what = format!(
                    "Renamed {old} to {new} in _funct.md{}",
                    match graphs {
                        0 => String::new(),
                        1 => " and 1 graph".to_owned(),
                        n => format!(" and {n} graphs"),
                    }
                );
                let task = self.edit_files(files, what, joined);
                if !joined {
                    return task;
                }
                let graph = self.edit_graph(|_| open_after, true);
                Task::batch([task, graph])
            }
        }
    }

    /// The wire at `line` of the open graph.
    fn graph_wire(&self, line: usize) -> Option<nodes::Wire> {
        let note = self.workspace.as_ref()?.note.as_ref()?;
        nodes::parse(&note.text()).wires.into_iter().find(|w| w.line == line)
    }

    /// Copies a saved function into the graph as `Name copy`, and points
    /// the wire being edited at the copy.
    fn duplicate_function(&mut self, name: &str, context: Context) -> Task<Message> {
        let Some((block, _, _)) = self.functions.find(name) else {
            return Task::none();
        };
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let Ok(saved) = ws.store.get(func::SAVED).map(str::to_owned) else {
            return Task::none();
        };
        let taken = |n: &str| self.functions.find(n).is_some();
        let copy = (1..)
            .map(|n| if n == 1 { format!("{name} copy") } else { format!("{name} copy {n}") })
            .find(|n| !taken(n))
            .unwrap_or_default();
        let mut lines: Vec<String> = saved
            .lines()
            .skip(block.open)
            .take(block.close + 1 - block.open)
            .map(str::to_owned)
            .collect();
        if let Some(fn_line) = lines.iter_mut().find(|l| l.trim_start().starts_with("fn ")) {
            *fn_line = format!("fn {copy}");
        }
        let wire = match context {
            Context::Wire { line } => Some(line),
            Context::Node => None,
        };
        let call = wire.and_then(|line| self.graph_wire(line)).and_then(|w| w.function);
        let task = self.edit_graph(
            |text| {
                let text = match (wire, call) {
                    (Some(line), Some(call)) => {
                        let rest = call.find('(').map_or("", |i| &call[i..]).to_owned();
                        nodes::set_wire_function(text, line, Some(&format!("{copy}{rest}")))
                    }
                    _ => text.to_owned(),
                };
                nodes::add_block(&text, Section::Functions, &lines)
            },
            true,
        );
        self.status = Some(match wire {
            Some(_) => format!("Copied to {copy} in this graph; this wire uses the copy"),
            None => format!("Copied to {copy} in this graph"),
        });
        let context = match wire {
            Some(line) => Context::Wire { line },
            None => Context::Node,
        };
        self.open_editor(&copy.clone(), context);
        task
    }

    /// Creates `_funct.md` with the two starter functions, the first time
    /// the function library is opened.
    fn ensure_saved_functions(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        if ws.vault.note(func::SAVED).is_some() {
            return;
        }
        let path = ws.vault.path(func::SAVED);
        let created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, func::SAVED_DEFAULT.as_bytes()));
        if let Err(error) = created {
            self.status = Some(format!("⚠ Couldn't create _funct.md: {error}"));
            return;
        }
        let _ = ws.store.get(func::SAVED);
        if let Ok(vault) = Vault::scan(&ws.vault.root) {
            ws.vault = vault;
            ws.tree.refresh(&ws.vault);
        }
        self.status = Some("Created _funct.md with Wait For Done and Copy Status".into());
        self.rebuild_graph();
    }

    fn open_picker(&mut self, target: PickTarget, current: Option<String>) {
        self.ensure_saved_functions();
        let wants = match &target {
            PickTarget::Wire { from, to, .. } => Some((end_kind(from), end_kind(to))),
            PickTarget::Node => None,
        };
        let choices = self
            .functions
            .all()
            .into_iter()
            .map(|(b, origin)| {
                let f = &b.function;
                let fits = match wants {
                    Some(w) => f.wire_shape() == Some(w),
                    None => true,
                };
                (f.name.clone(), origin, fits, func_editor::shape(f))
            })
            .collect();
        let missing = current.as_ref().filter(|c| self.functions.find(c).is_none()).cloned();
        let picker = Picker {
            choices,
            name: missing.unwrap_or_default(),
            for_wire: matches!(target, PickTarget::Wire { .. }),
            current,
        };
        self.picker = Some((picker, target));
    }

    fn picker_message(&mut self, message: func_editor::PickerMessage) -> Task<Message> {
        use func_editor::PickerMessage;
        let Some((picker, target)) = &mut self.picker else {
            return Task::none();
        };
        match message {
            PickerMessage::Name(name) => {
                picker.name = name;
                Task::none()
            }
            PickerMessage::Close => {
                self.picker = None;
                Task::none()
            }
            PickerMessage::Remove => {
                let target = target.clone();
                self.picker = None;
                self.editor = None;
                match target {
                    PickTarget::Wire { line: Some(line), .. } => {
                        self.edit_graph(|text| nodes::set_wire_function(text, line, None), true)
                    }
                    _ => Task::none(),
                }
            }
            PickerMessage::Pick(name) => {
                let target = target.clone();
                self.picker = None;
                self.attach(&name, target, None)
            }
            PickerMessage::Create => {
                let name = picker.name.trim().to_owned();
                if name.is_empty() {
                    self.status = Some("⚠ Give the new function a name".into());
                    return Task::none();
                }
                if name.contains(['(', ')', '.', ':', '[', ']']) {
                    self.status = Some("⚠ A function's name can't contain ( ) . : [ or ]".into());
                    return Task::none();
                }
                let target = target.clone();
                self.picker = None;
                if self.functions.find(&name).is_some() {
                    return self.attach(&name, target, None);
                }
                let (kin, kout) = match &target {
                    PickTarget::Wire { from, to, .. } => (end_kind(from), end_kind(to)),
                    PickTarget::Node => (func::PortKind::File, func::PortKind::File),
                };
                let block = vec![
                    "```obsession".to_owned(),
                    format!("fn {name}"),
                    func::edit::port_line(true, "src", kin),
                    func::edit::port_line(false, "dst", kout),
                    "```".to_owned(),
                ];
                self.attach(&name, target, Some(block))
            }
        }
    }

    /// Puts a function on a wire (or adds it as a node), creating its block
    /// in `## Functions` first when `block` is given, then opens it.
    fn attach(&mut self, name: &str, target: PickTarget, block: Option<Vec<String>>) -> Task<Message> {
        let add = |text: &str| match &block {
            Some(lines) => nodes::add_block(text, Section::Functions, lines),
            None => text.to_owned(),
        };
        match target {
            PickTarget::Node => {
                let item = format!("fn {name}");
                let task = self.edit_graph(|text| add(&nodes::add_item(text, Section::Nodes, &item)), true);
                self.editor = None;
                task
            }
            PickTarget::Wire { line, from, to } => {
                let task = self.edit_graph(
                    |text| match line {
                        Some(line) => add(&nodes::set_wire_function(text, line, Some(name))),
                        None => add(&nodes::add_item(
                            text,
                            Section::Wires,
                            &format!("{} → {} : {name}", end_text(&from), end_text(&to)),
                        )),
                    },
                    true,
                );
                // The wire's line now (a link wire just got one).
                let line = self.workspace.as_ref().and_then(|ws| ws.note.as_ref()).and_then(|note| {
                    nodes::parse(&note.text())
                        .wires
                        .into_iter()
                        .find(|w| {
                            w.from == from
                                && w.to == to
                                && w.function.as_deref().is_some_and(|f| f.split('(').next().unwrap_or(f).trim().eq_ignore_ascii_case(name))
                        })
                        .map(|w| w.line)
                });
                if let Some(line) = line {
                    self.open_editor(name, Context::Wire { line });
                }
                task
            }
        }
    }

    /// Changes the open graph's text (undoable, autosaved), then redraws
    /// the graph if `rebuild`.
    fn edit_graph(&mut self, change: impl FnOnce(&str) -> String, rebuild: bool) -> Task<Message> {
        let Some(note) = self.note_mut() else {
            return Task::none();
        };
        let text = note.text();
        let new = change(&text);
        if new == text {
            return Task::none();
        }
        let id = note.id;
        let timers = note.apply_text(&new);
        if rebuild {
            self.rebuild_graph();
        }
        start_timers(id, timers)
    }

    fn graph_message(&mut self, message: graph::Message) -> Task<Message> {
        let Some(graph) = &mut self.graph else {
            return Task::none();
        };
        match message {
            graph::Message::Viewed(view) => {
                // Kept for coming back from the function editor, and written
                // to `.obsessed/views.txt` when you leave the graph.
                graph.saved_view = Some(view);
                if let Some(ws) = &mut self.workspace
                    && let Some(note) = &ws.note
                {
                    let key = views::graph_key(&note.rel);
                    ws.views.set(&key, view);
                }
                Task::none()
            }
            graph::Message::Moved(card, at) => {
                if let Some(c) = graph.cards.get_mut(card) {
                    c.pos = at;
                }
                graph.touch();
                let layout: Vec<(String, f32, f32)> =
                    graph.layout().into_iter().map(|(k, x, y)| (k.to_owned(), x, y)).collect();
                self.edit_graph(
                    |text| {
                        let refs: Vec<(&str, f32, f32)> = layout.iter().map(|(k, x, y)| (k.as_str(), *x, *y)).collect();
                        nodes::set_layout(text, &refs)
                    },
                    false,
                )
            }
            graph::Message::Open(card) => {
                let Some(c) = graph.cards.get(card) else {
                    return Task::none();
                };
                if c.kind == graph::CardKind::Function {
                    let name = c.title.clone();
                    self.open_editor(&name, Context::Node);
                    return Task::none();
                }
                let key = c.key.clone();
                self.follow_note(&key, None, true)
            }
            graph::Message::Connect(from, to) => {
                let (Some(a), Some(b)) = (graph.cards.get(from.card), graph.cards.get(to.card)) else {
                    return Task::none();
                };
                let fa = a.kind == graph::CardKind::Function;
                let fb = b.kind == graph::CardKind::Function;
                if fa && fb {
                    self.status = Some("⚠ Wire a function's pins to notes, not to another function".into());
                    return Task::none();
                }
                let start = if fa {
                    match from.pin.and_then(|k| a.pins_out.get(k)) {
                        Some((pin, _)) => format!("{}.{pin}", a.title),
                        None => {
                            self.status = Some(format!("⚠ {} has no outputs", a.title));
                            return Task::none();
                        }
                    }
                } else {
                    format!("[[{}]]", a.key)
                };
                let end = if fb {
                    match to.pin.and_then(|k| b.pins_in.get(k)) {
                        Some((pin, _)) => format!("{}.{pin}", b.title),
                        None => {
                            self.status = Some(format!("⚠ {} has no inputs", b.title));
                            return Task::none();
                        }
                    }
                } else {
                    format!("[[{}]]", b.key)
                };
                let known = graph.wires.iter().any(|w| {
                    !w.link
                        && w.from == from.card
                        && w.to == to.card
                        && (!fa || w.from_pin == from.pin)
                        && (!fb || w.to_pin == to.pin)
                });
                if known {
                    return Task::none();
                }
                let item = format!("{start} → {end}");
                self.edit_graph(|text| nodes::add_item(text, Section::Wires, &item), true)
            }
            graph::Message::OpenCrew(m) => {
                let Some(key) = graph.hand.get(m).map(|h| h.key.clone()) else {
                    return Task::none();
                };
                self.open_crew(&key)
            }
            graph::Message::Assign { member, card, task } => {
                let (Some(h), Some(c)) = (graph.hand.get(member), graph.cards.get(card)) else {
                    return Task::none();
                };
                let key = h.key.clone();
                if c.rel.is_none() {
                    self.status = Some(format!("⚠ {} isn't a note yet: double-click it to create it first", c.title));
                    return Task::none();
                }
                match task {
                    Some(row) => self.assign(&key, card, row),
                    None if c.tasks.is_empty() => {
                        self.status = Some(format!("{} has no tasks to give @{key}", c.title));
                        Task::none()
                    }
                    None => {
                        self.assigning = Some(Assigning {
                            member: key.clone(),
                            card,
                            tasks: c
                                .tasks
                                .iter()
                                .enumerate()
                                .map(|(i, t)| (i, t.text.clone(), t.people.contains(&key)))
                                .collect(),
                            title: c.title.clone(),
                        });
                        Task::none()
                    }
                }
            }
            graph::Message::Function(wire) => {
                let Some(w) = graph.wires.get(wire).cloned() else {
                    return Task::none();
                };
                let (a, b) = (&graph.cards[w.from], &graph.cards[w.to]);
                if a.kind == graph::CardKind::Function || b.kind == graph::CardKind::Function {
                    self.status = Some("A wire to a function's pin carries a file; the function itself is the node".into());
                    return Task::none();
                }
                let (a_key, b_key) = (a.key.clone(), b.key.clone());
                let name = w.label.as_ref().map(|l| l.split('(').next().unwrap_or(l).trim().to_owned());
                if let (Some(name), Some(line)) = (&name, w.line)
                    && self.functions.find(name).is_some()
                {
                    self.open_editor(name, Context::Wire { line });
                    return Task::none();
                }
                let (from, to) = match w.line.and_then(|line| self.graph_wire(line)) {
                    Some(parsed) => (parsed.from, parsed.to),
                    None => (
                        End::Note { target: a_key, sub: None },
                        End::Note { target: b_key, sub: None },
                    ),
                };
                self.open_picker(PickTarget::Wire { line: w.line, from, to }, name);
                Task::none()
            }
            graph::Message::Delete(graph::Selected::Card(card)) => {
                let Some(c) = graph.cards.get(card) else {
                    return Task::none();
                };
                let key = c.key.clone();
                let wires: Vec<usize> = graph
                    .wires
                    .iter()
                    .filter(|w| (w.from == card || w.to == card) && !w.link)
                    .filter_map(|w| w.line)
                    .collect();
                let layout: Vec<(String, f32, f32)> = graph
                    .layout()
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| *i != card)
                    .map(|(_, (k, x, y))| (k.to_owned(), x, y))
                    .collect();
                self.status = Some(format!("Removed {key} from the graph (the note itself is untouched)"));
                self.edit_graph(
                    |text| {
                        let parsed = nodes::parse(text);
                        let mut lines = wires;
                        if let Some(node) = parsed.nodes.iter().find(|n| n.key == key) {
                            lines.push(node.line);
                        }
                        let without = nodes::remove_lines(text, &lines);
                        let refs: Vec<(&str, f32, f32)> = layout.iter().map(|(k, x, y)| (k.as_str(), *x, *y)).collect();
                        nodes::set_layout(&without, &refs)
                    },
                    true,
                )
            }
            graph::Message::Delete(graph::Selected::Wire(wire)) => {
                let Some(w) = graph.wires.get(wire) else {
                    return Task::none();
                };
                match w.line {
                    Some(line) => self.edit_graph(|text| nodes::remove_lines(text, &[line]), true),
                    None => {
                        // A link wire: from the linked note to the note
                        // with the link.
                        let (Some(source), Some(target)) =
                            (graph.cards[w.to].rel.clone(), graph.cards[w.from].rel.clone())
                        else {
                            return Task::none();
                        };
                        let count = self.links_to(&source, &target);
                        if count > 0 {
                            self.unlinking = Some(Unlinking { source, target, count });
                        }
                        Task::none()
                    }
                }
            }
        }
    }

    fn open_index(&mut self, index: u32) -> Task<Message> {
        let Some(ws) = &self.workspace else {
            return Task::none();
        };
        match ws.vault.entries.get(index as usize) {
            Some(entry) if entry.kind == Kind::Note => {
                let rel = entry.rel.clone();
                self.open(rel)
            }
            _ => Task::none(),
        }
    }

    /// Opens a note: saves the current one first, then reads the new one
    /// through the store (from RAM if it was read before).
    fn open(&mut self, rel: Box<str>) -> Task<Message> {
        if self
            .workspace
            .as_ref()
            .and_then(|ws| ws.note.as_ref())
            .is_some_and(|n| n.rel == rel)
        {
            return Task::none();
        }
        let save = self.save_now();
        // Leaving a graph: remember where it was looked at.
        self.save_views();
        let mode = self.mode;
        self.next_note_id += 1;
        self.editor = None;
        self.picker = None;
        // Suggestions belong to the note they were typed in.
        self.completion = None;
        let id = self.next_note_id;

        let Some(ws) = &mut self.workspace else {
            return save;
        };
        if let Some(old) = ws.note.take() {
            ws.store.unpin(&old.rel);
        }
        // Its undo steps belong to the note that was open.
        ws.elsewhere.clear();
        match ws.store.pin(&rel).and_then(|()| ws.store.get(&rel).map(str::to_owned)) {
            Ok(text) => {
                ws.tree.reveal(&ws.vault, &rel);
                let note = Note::new(id, rel, &text, mode, ws.team.as_ref());
                self.status = note.warnings.first().cloned();
                ws.note = Some(note);
                self.menu = None;
            }
            Err(error) => {
                self.status = Some(format!("Couldn't open {rel}: {error}"));
            }
        }
        self.refresh_links();
        if self.note_mut().is_some_and(|n| nodes::is_graph(&n.rel)) {
            self.show_graph = true;
            self.graph_fit += 1;
        }
        self.rebuild_graph();
        save
    }

    /// Opens a note and shows a line or a `#heading`/`^block` in it.
    fn open_at(&mut self, rel: Box<str>, jump: Option<Jump>) -> Task<Message> {
        let task = self.open(rel.clone());
        let Some(jump) = jump else {
            return task;
        };
        let Some(note) = self.note_mut().filter(|n| n.rel == rel) else {
            return task;
        };
        let line = match &jump {
            Jump::Line(line) => Some(*line),
            Jump::Section(sub) => links::outline(&note.text()).find(sub),
        };
        match line {
            Some(line) => note.jump_to(line),
            None => {
                if let Jump::Section(sub) = jump {
                    self.status = Some(format!("⚠ No “{sub}” in {}", rel.trim_end_matches(".md")));
                }
            }
        }
        task
    }

    /// Re-checks the open note's links and its backlinks.
    fn refresh_links(&mut self) {
        let Some(ws) = &mut self.workspace else {
            return;
        };
        let Some(note) = &mut ws.note else {
            ws.backlinks.clear();
            return;
        };
        let text = note.text();
        let mut states = links::link_states(&text, &note.rel, &ws.vault, &ws.links);
        for line in text.lines() {
            for (_, name) in crew::mentions(line) {
                let known = ws.crew.iter().any(|m| m.key == name);
                states.insert(format!("@{name}"), if known { links::LinkState::Ok } else { links::LinkState::Missing });
            }
        }
        note.set_link_states(LinkStates::new(states));
        if ws.indexed {
            ws.mentions = ws.links.mention_counts(ws.team.as_ref(), None);
        }
        ws.backlinks = if ws.indexed {
            ws.links.backlinks(&note.rel, &ws.vault)
        } else {
            Vec::new()
        };
    }

    /// Renames (or moves) a note and rewrites every link to it, the way
    /// Obsidian does. Runs on this thread: the open note is saved first,
    /// then the file is moved, then each linking note is rewritten and
    /// saved (each write is atomic).
    fn rename(&mut self, old: &str, input: &str) -> Result<Task<Message>, String> {
        let new_rel = vault::note_rel_from_input(input).map_err(str::to_owned)?;
        if new_rel == old {
            return Ok(Task::none());
        }
        self.save_blocking();
        let Some(ws) = &mut self.workspace else {
            return Ok(Task::none());
        };
        if !ws.indexed {
            return Err("Still reading the vault's links; try again in a moment".into());
        }
        if ws.vault.note(&new_rel).is_some() && !new_rel.eq_ignore_ascii_case(old) {
            return Err("A note with that name is already there".into());
        }
        let sources = ws.links.linking_notes(old, &ws.vault);
        let from = ws.vault.path(old);
        let to = ws.vault.path(&new_rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't make the folder: {e}"))?;
        }
        std::fs::rename(&from, &to).map_err(|e| format!("Couldn't rename it: {e}"))?;

        // The store and the open note follow the file.
        let was_open = ws.note.as_ref().is_some_and(|n| &*n.rel == old);
        ws.store.forget(old);
        if was_open && let Some(note) = &mut ws.note {
            note.rel = new_rel.clone().into();
            let _ = ws.store.pin(&new_rel);
        }
        ws.links.remove(old);
        ws.views.rename(old, &new_rel);

        let mut notes = 0;
        let mut count = 0;
        let mut failed = Vec::new();
        for source in &sources {
            // The renamed note may link to itself.
            let now: &str = if &**source == old { &new_rel } else { source };
            let Ok(text) = ws.store.get(now).map(str::to_owned) else {
                failed.push(now.to_owned());
                continue;
            };
            let Some((new_text, n)) = links::rewrite_links(&text, source, &ws.vault, old, &new_rel) else {
                continue;
            };
            ws.store.set(now, new_text.clone());
            if let Some(job) = ws.store.take_save(now) {
                let result = job.run();
                ws.store.finish_save(&result);
                if result.error().is_some() {
                    failed.push(now.to_owned());
                    continue;
                }
            }
            ws.links.update(now, &new_text);
            if let Some(search) = &mut ws.search { search.update(now, &new_text); }
            if let Some(note) = &mut ws.note
                && &*note.rel == now
            {
                note.replace_text(&new_text);
            }
            notes += 1;
            count += n;
        }
        if ws.links.outline(&new_rel).is_none()
            && let Ok(text) = ws.store.get(&new_rel).map(str::to_owned)
        {
            ws.links.update(&new_rel, &text);
            if let Some(search) = &mut ws.search { search.update(&new_rel, &text); }
        }

        match Vault::scan(&ws.vault.root) {
            Ok(vault) => {
                ws.vault = vault;
                ws.tree.refresh(&ws.vault);
                ws.tree.reveal(&ws.vault, &new_rel);
            }
            Err(error) => self.status = Some(format!("Couldn't re-read the folder: {error}")),
        }
        let name = new_rel.trim_end_matches(".md");
        self.status = Some(if !failed.is_empty() {
            format!("⚠ Renamed to {name}, but couldn't update links in: {}", failed.join(", "))
        } else if count == 0 {
            format!("Renamed to {name}")
        } else {
            format!(
                "Renamed to {name} · updated {count} link{} in {notes} note{}",
                if count == 1 { "" } else { "s" },
                if notes == 1 { "" } else { "s" }
            )
        });
        self.refresh_links();
        Ok(Task::none())
    }

    /// Hands unsaved edits to the store and starts a background write if the
    /// text differs from the disk.
    fn save_now(&mut self) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let Some(note) = &mut ws.note else {
            return Task::none();
        };
        let mut changed = None;
        if note.unsynced {
            let text = note.text();
            note.analyze(&text);
            ws.links.update(&note.rel, &text);
            if let Some(search) = &mut ws.search { search.update(&note.rel, &text); }
            if ws.store.get(&note.rel).ok() != Some(text.as_str()) {
                changed = Some(note.rel.to_string());
            }
            ws.store.set(&note.rel, text);
            note.unsynced = false;
            if &*note.rel == SETTINGS_NOTE {
                ws.load_team();
                if let Some(note) = &mut ws.note {
                    note.refresh_format(ws.team.as_ref());
                }
            }
        }
        let Some(note) = &ws.note else {
            return Task::none();
        };
        let save = match ws.store.take_save(&note.rel) {
            Some(job) => Task::perform(async move { job.run() }, Message::Saved),
            None => Task::none(),
        };
        match changed {
            Some(rel) => Task::batch([save, self.note_changed(&rel)]),
            None => save,
        }
    }

    /// A note's text changed (here or in another app): functions that read
    /// it run, and a changed graph or `_funct.md` changes the functions.
    fn note_changed(&mut self, rel: &str) -> Task<Message> {
        let card = self.workspace.as_ref().is_some_and(|ws| crew::is_card(rel, &ws.crew_folder));
        let settings = rel.eq_ignore_ascii_case(SETTINGS_NOTE);
        if (card || settings)
            && let Some(ws) = &mut self.workspace
        {
            if settings {
                ws.load_team();
            }
            ws.load_crew();
            self.refresh_links();
            if !settings {
                self.rebuild_graph();
                return Task::none();
            }
        }
        if nodes::is_graph(rel) || rel.eq_ignore_ascii_case(func::SAVED) || rel.eq_ignore_ascii_case(SETTINGS_NOTE) {
            self.rebuild_index();
            return Task::none();
        }
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        if !ws.runs.index.reads(rel) {
            // Not an input, but maybe an output: what's out of date moves.
            if ws.runs.pinned.iter().any(|p| p == rel) && !ws.runs.running {
                self.check_out_of_date();
                self.rebuild_graph();
            }
            return Task::none();
        }
        ws.runs.pending.insert(rel.to_owned());
        self.schedule_run()
    }

    /// Re-reads every graph and `_funct.md` into the run index, keeps the
    /// notes it touches in RAM, then checks what's out of date.
    fn rebuild_index(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let rels: Vec<String> = ws
            .vault
            .notes()
            .filter(|e| nodes::is_graph(&e.rel))
            .map(|e| e.rel.to_string())
            .collect();
        let graphs: Vec<(String, String)> = rels
            .into_iter()
            .filter_map(|rel| {
                let text = ws.store.get(&rel).ok()?.to_owned();
                Some((rel, text))
            })
            .collect();
        let saved = if ws.vault.note(func::SAVED).is_some() {
            ws.store.get(func::SAVED).map(func::blocks).unwrap_or_default()
        } else {
            Vec::new()
        };
        let index = Index::build(&ws.vault, &graphs, &saved);
        let files = index.files();
        for rel in &files {
            let _ = ws.store.pin(rel);
        }
        for rel in std::mem::replace(&mut ws.runs.pinned, files) {
            ws.store.unpin(&rel);
        }
        ws.runs.index = Arc::new(index);
        self.check_out_of_date();
    }

    /// Which functions would change something if they ran now. Opening a
    /// vault never changes files: those wires show "out of date" instead.
    fn check_out_of_date(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let files = run_files(ws);
        let index = ws.runs.index.clone();
        ws.runs.out_of_date = runner::pending(&index, &files, ws.team.as_ref())
            .into_iter()
            .map(|(j, summary)| ((index.jobs[j].graph.clone(), index.jobs[j].line), summary))
            .collect();
    }

    /// Starts the pending run when its time comes (at most one a second).
    fn schedule_run(&mut self) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let runs = &mut ws.runs;
        if runs.running || runs.scheduled || runs.pending.is_empty() {
            return Task::none();
        }
        runs.scheduled = true;
        Task::perform(timer::after(runs.wait()), |()| Message::RunNow)
    }

    /// Runs `start` (and their chains) off the UI thread. The open note's
    /// typing is saved into the run first.
    fn start_run(&mut self, start: Vec<usize>) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        ws.runs.running = true;
        let files = run_files(ws);
        let index = ws.runs.index.clone();
        let team = ws.team.clone();
        Task::perform(
            async move {
                let outcome = runner::run(&index, &files, &start, team.as_ref());
                let summary = outcome.summary(&index);
                let names = outcome.blocked.keys().map(|&j| (j, index.jobs[j].name().to_owned())).collect();
                let ran = outcome.ran.iter().map(|&j| (index.jobs[j].graph.clone(), index.jobs[j].line)).collect();
                Arc::new(RunResult { outcome, summary, names, ran })
            },
            Message::RunDone,
        )
    }

    /// Puts new text in a note: the open note (undoable there) or the
    /// store, then saves it once.
    fn put_text(&mut self, rel: &str, text: &str) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let mut timers = Task::none();
        if let Some(note) = &mut ws.note
            && &*note.rel == rel
        {
            let id = note.id;
            let t = note.apply_text(text);
            note.unsynced = false;
            note.analyze(text);
            timers = start_timers(id, t);
        }
        ws.links.update(rel, text);
        if let Some(search) = &mut ws.search { search.update(rel, text); }
        ws.store.set(rel, text.to_owned());
        let save = match ws.store.take_save(rel) {
            Some(job) => Task::perform(async move { job.run() }, Message::Saved),
            None => Task::none(),
        };
        Task::batch([timers, save])
    }

    /// The current text of a note (the open note's editor, or the store).
    fn current_text(&mut self, rel: &str) -> Option<String> {
        let ws = self.workspace.as_mut()?;
        match &ws.note {
            Some(note) if &*note.rel == rel => Some(note.text()),
            _ => ws.store.get(rel).ok().map(str::to_owned),
        }
    }

    /// A run finished: apply its changes to what the notes hold now, save
    /// each changed note once, and log it for undo.
    fn run_done(&mut self, result: &RunResult) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        ws.runs.running = false;
        let now = std::time::Instant::now();
        ws.runs.last = Some(now);
        // Only runs that changed something glow.
        if !result.outcome.changes.is_empty() {
            ws.runs.glow = result.ran.iter().map(|k| (k.clone(), now)).collect();
        }
        let outcome = &result.outcome;
        let mut rels: Vec<String> = Vec::new();
        for c in &outcome.changes {
            if !rels.contains(&c.rel) {
                rels.push(c.rel.clone());
            }
        }
        let mut tasks = Vec::new();
        let mut applied: Vec<Change> = Vec::new();
        let mut skipped = 0;
        for rel in &rels {
            let mine: Vec<&Change> = outcome.changes.iter().filter(|c| &c.rel == rel).collect();
            let Some(current) = self.current_text(rel) else {
                skipped += mine.len();
                continue;
            };
            let (new, done) = runner::apply(&current, &mine);
            skipped += mine.len() - done.len();
            applied.extend(done.into_iter().cloned());
            if new != current {
                tasks.push(self.put_text(rel, &new));
            }
        }
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let root = ws.vault.root.clone();
        let mut notes: Vec<String> = Vec::new();
        if !applied.is_empty() {
            match ws.runs.record(&root, obsession_core::log::Kind::Run, result.summary.clone(), applied) {
                Ok(_) => notes.push(format!("{} · Undo in Runs", result.summary)),
                Err(e) => notes.push(format!("{} · ⚠ {e}", result.summary)),
            }
        }
        if skipped > 0 {
            notes.push(format!("⚠ {skipped} change{} skipped: the note changed meanwhile", if skipped == 1 { "" } else { "s" }));
        }
        notes.extend(outcome.warnings.iter().map(|w| format!("⚠ {w}")));
        for (j, why) in &outcome.blocked {
            let name = result.names.get(j).map_or("A function", String::as_str);
            notes.push(format!("⚠ {name} didn't change anything: {why}"));
        }
        if !notes.is_empty() {
            self.status = Some(notes.join(" · "));
        }
        self.check_out_of_date();
        self.rebuild_graph();
        tasks.push(self.schedule_run());
        Task::batch(tasks)
    }

    /// Undoes a run: each change whose lines still hold what it wrote.
    fn undo_run(&mut self, id: u64) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let Some(rec) = ws.runs.log.iter().find(|r| r.id == id).cloned() else {
            return Task::none();
        };
        let mut rels: Vec<String> = Vec::new();
        for c in &rec.changes {
            if !rels.contains(&c.rel) {
                rels.push(c.rel.clone());
            }
        }
        let mut tasks = Vec::new();
        let mut skipped = 0;
        for rel in &rels {
            let mine: Vec<&Change> = rec.changes.iter().filter(|c| &c.rel == rel).collect();
            let Some(current) = self.current_text(rel) else {
                skipped += mine.len();
                continue;
            };
            let (old, missed) = runner::revert(&current, &mine);
            skipped += missed;
            if old != current {
                tasks.push(self.put_text(rel, &old));
            }
        }
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let _ = obsession_core::log::mark_undone(&ws.vault.root, id);
        if let Some(r) = ws.runs.log.iter_mut().find(|r| r.id == id) {
            r.undone = true;
        }
        self.status = Some(match skipped {
            0 => format!("Undid: {}", rec.summary),
            n => format!(
                "Undid: {} · ⚠ {n} line{} left alone: changed since the run",
                rec.summary,
                if n == 1 { "" } else { "s" }
            ),
        });
        self.check_out_of_date();
        self.rebuild_graph();
        Task::batch(tasks)
    }

    /// Files changed on disk: tell other apps' edits from our own writes.
    /// The watcher saw changes. Notes go in a queue that `drain_outside`
    /// reads a slice at a time, so a burst (a git pull, a sync) doesn't read
    /// every note at once on the UI thread. The file list and pictures are
    /// dealt with now.
    fn files_changed(&mut self, events: Vec<runs::FsEvent>) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let root = ws.vault.root.clone();
        let mut structural = false;
        for event in events {
            structural |= event.structural;
            if let Ok(rel) = event.path.strip_prefix(&root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if obsession_core::live::is_picture(&rel) {
                    self.pictures.forget(&rel);
                }
            }
            if runs::rel_of(&root, &event.path).is_some() && ws.outside.queued.insert(event.path.clone()) {
                ws.outside.queue.push_back(event.path);
                ws.outside.quiet_since = None;
            }
        }
        let drain = !ws.outside.queue.is_empty() && !ws.outside.draining;
        let mut tasks = Vec::new();
        if structural {
            tasks.push(self.rescan());
        }
        if drain {
            if let Some(ws) = &mut self.workspace {
                ws.outside.draining = true;
                ws.outside.started = Some(std::time::Instant::now());
            }
            tasks.push(self.drain_outside());
        }
        Task::batch(tasks)
    }

    /// Takes the next slice of queued outside changes. Notes the store holds
    /// (the open one, pinned ones) are checked here, for up to
    /// `OUTSIDE_SLICE`; up to `OUTSIDE_BATCH` others are read one after
    /// another off the UI thread (a read can wait on the disk or a virus
    /// scan) and come back as `Message::OutsideRead`. Then a pause of
    /// `OUTSIDE_PAUSE` before the next slice, so a big burst stays a
    /// fraction of one core and the window keeps drawing. When the queue has
    /// been empty for `OUTSIDE_QUIET`, logs the burst and refreshes what
    /// depends on the notes once.
    fn drain_outside(&mut self) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let root = ws.vault.root.clone();
        let started = std::time::Instant::now();
        let mut rescan = false;
        let mut to_read: Vec<(String, PathBuf)> = Vec::new();
        ws.outside.slices += 1;
        while let Some(path) = ws.outside.queue.pop_front() {
            ws.outside.read += 1;
            ws.outside.queued.remove(&path);
            let Some(rel) = runs::rel_of(&root, &path) else { continue };
            if !ws.store.is_loaded(&rel) {
                to_read.push((rel, path));
                if to_read.len() >= OUTSIDE_BATCH {
                    break;
                }
                continue;
            }
            match ws.store.check_outside(&rel) {
                Outside::Same => {}
                Outside::Changed => {
                    let text = ws.store.get(&rel).map(str::to_owned).unwrap_or_default();
                    ws.links.update(&rel, &text);
                    if let Some(search) = &mut ws.search { search.update(&rel, &text); }
                    if let Some(note) = &mut ws.note
                        && *note.rel == *rel
                    {
                        if note.unsynced {
                            ws.outside.warnings.push(format!(
                                "⚠ {} changed in another app; your unsaved edits here will be saved over it",
                                rel.trim_end_matches(".md")
                            ));
                        } else {
                            note.replace_text(&text);
                            note.analyze(&text);
                        }
                    }
                    ws.outside.changed.push(rel);
                }
                Outside::Conflict => ws.outside.warnings.push(format!(
                    "⚠ {} changed in another app while it had unsaved changes here; keeping these",
                    rel.trim_end_matches(".md")
                )),
                Outside::Deleted => {
                    ws.links.remove(&rel);
                    if let Some(search) = &mut ws.search {
                        search.remove(&rel);
                    }
                    rescan = true;
                }
                Outside::NotLoaded => {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        ws.links.update(&rel, &text);
                        if let Some(search) = &mut ws.search { search.update(&rel, &text); }
                        ws.outside.changed.push(rel);
                    }
                }
            }
            if started.elapsed() >= OUTSIDE_SLICE {
                break;
            }
        }
        let mut tasks = Vec::new();
        if rescan {
            tasks.push(Task::done(Message::Rescan));
        }
        if !to_read.is_empty() {
            // One after another, off the UI thread.
            tasks.push(Task::perform(
                async move {
                    to_read
                        .into_iter()
                        .map(|(rel, path)| {
                            let text = std::fs::read_to_string(&path).map_err(|e| e.kind() == std::io::ErrorKind::NotFound);
                            (rel, text)
                        })
                        .collect()
                },
                Message::OutsideRead,
            ));
            return Task::batch(tasks);
        }
        // The burst ends once nothing new has come for `OUTSIDE_QUIET`: the
        // watcher passes changes on as they happen, so a burst arrives as
        // many small batches.
        let quiet = match (ws.outside.queue.is_empty(), ws.outside.quiet_since) {
            (false, _) => None,
            (true, None) => {
                ws.outside.quiet_since = Some(std::time::Instant::now());
                Some(OUTSIDE_QUIET)
            }
            (true, Some(since)) => OUTSIDE_QUIET.checked_sub(since.elapsed()).filter(|d| !d.is_zero()),
        };
        let wait = if ws.outside.queue.is_empty() { quiet } else { Some(OUTSIDE_PAUSE) };
        if let Some(wait) = wait {
            tasks.push(Task::perform(timer::after(wait), |()| Message::DrainOutside));
            return Task::batch(tasks);
        }
        // The burst is read: once for all of it.
        ws.outside.quiet_since = None;
        ws.outside.draining = false;
        if std::env::var_os("OBSESSION_DEV_TIMING").is_some() {
            eprintln!(
                "TIMING outside burst: {} notes in {} slices, {} ms",
                ws.outside.read,
                ws.outside.slices,
                ws.outside.started.map_or(0, |t| t.elapsed().as_millis())
            );
        }
        ws.outside.read = 0;
        ws.outside.slices = 0;
        let changed = std::mem::take(&mut ws.outside.changed);
        let warnings = std::mem::take(&mut ws.outside.warnings);
        if !changed.is_empty() {
            let names: Vec<&str> = changed.iter().take(3).map(|r| r.trim_end_matches(".md")).collect();
            let more = if changed.len() > 3 { format!(" and {} more", changed.len() - 3) } else { String::new() };
            let summary = format!("Changed in another app: {}{more}", names.join(", "));
            let _ = ws.runs.record(&root, obsession_core::log::Kind::Outside, summary, Vec::new());
            for rel in changed {
                tasks.push(self.note_changed(&rel));
            }
            self.refresh_links();
            self.rebuild_graph();
        }
        if !warnings.is_empty() {
            self.status = Some(warnings.join(" · "));
        }
        Task::batch(tasks)
    }

    /// Notes read off the UI thread for a burst: index them, then on to the
    /// next slice after a pause. (`Err(true)`: the note is gone.)
    fn outside_read(&mut self, texts: Vec<(String, Result<String, bool>)>) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let mut rescan = false;
        for (rel, text) in texts {
            match text {
                Ok(text) => {
                    ws.links.update(&rel, &text);
                    if let Some(search) = &mut ws.search {
                        search.update(&rel, &text);
                    }
                    ws.outside.changed.push(rel);
                }
                Err(true) => {
                    ws.links.remove(&rel);
                    if let Some(search) = &mut ws.search {
                        search.remove(&rel);
                    }
                    rescan = true;
                }
                Err(false) => {}
            }
        }
        let next = Task::perform(timer::after(OUTSIDE_PAUSE), |()| Message::DrainOutside);
        if rescan { Task::batch([next, self.rescan()]) } else { next }
    }

    /// Reads the vault folder again, off the UI thread, at most once every
    /// `RESCAN_GAP`: a request within the gap (or while one runs) schedules
    /// one rescan for when it's over, and further requests join that one.
    /// So a burst of new or deleted files rescans (and refreshes everything
    /// after it) a few times a second, not once per batch the watcher sends.
    fn rescan(&mut self) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let r = &mut ws.rescans;
        if r.running {
            r.again = true;
            return Task::none();
        }
        if r.scheduled {
            return Task::none();
        }
        if let Some(wait) = r.last.and_then(|last| RESCAN_GAP.checked_sub(last.elapsed())) {
            r.scheduled = true;
            return Task::perform(timer::after(wait), |()| Message::Rescan);
        }
        r.running = true;
        r.last = Some(std::time::Instant::now());
        let root = ws.vault.root.clone();
        Task::perform(async move { Vault::scan(&root).map_err(|e| e.to_string()) }, Message::VaultRefreshed)
    }

    /// A rescan landed: one asked for meanwhile follows, after the gap.
    fn rescan_landed(&mut self) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        ws.rescans.running = false;
        if std::mem::take(&mut ws.rescans.again) { self.rescan() } else { Task::none() }
    }

    /// Used when closing: write on this thread so nothing is lost.
    fn save_blocking(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        if let Some(note) = &mut ws.note {
            if note.unsynced {
                ws.store.set(&note.rel, note.text());
                note.unsynced = false;
            }
            if let Some(job) = ws.store.take_save(&note.rel) {
                let result = job.run();
                ws.store.finish_save(&result);
            }
        }
        // The index cache, if notes changed since it was written.
        if ws.indexed && ws.links_dirty {
            let _ = links::save_cache(&ws.vault.root, &ws.links);
        }
        self.save_views();
    }

    /// Writes `.obsessed/views.txt` if a view changed (forgetting graphs and
    /// function files that are gone). Never runs while idle: only on leaving
    /// a graph, switching vaults and closing.
    fn save_views(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        if !ws.views.is_dirty() {
            return;
        }
        ws.views.prune(|file| ws.vault.note(file).is_some());
        let _ = ws.views.save(&ws.vault.root);
    }

    /// A message from Live Preview.
    fn live_message(&mut self, message: live::Message) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let Some(note) = &mut ws.note else {
            return Task::none();
        };
        let outcome = note.live_message(message, ws.team.as_ref());
        if self.mode.shows_preview() {
            note.refresh_preview_if_stale();
        }
        let id = note.id;
        if outcome.performed.message.is_some() {
            self.status = outcome.performed.message;
        }
        let mut tasks = vec![start_timers(id, outcome.performed.timers)];
        if outcome.focus {
            tasks.push(operation::focus(live::editor_id()));
        }
        if outcome.settle {
            tasks.push(Task::perform(
                timer::after(std::time::Duration::from_millis(20)),
                |()| Message::Settle,
            ));
        }
        if let Some(dy) = outcome.scroll {
            tasks.push(operation::scroll_by(
                live::scroll_id(),
                iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: dy },
            ));
        }
        if outcome.edge_step {
            tasks.push(Task::perform(timer::after(live::EDGE_STEP), |()| {
                Message::Live(live::Message::EdgeScroll)
            }));
        }
        if let Some((task, at)) = outcome.menu {
            self.menu = Some(StatusMenu { task, at });
        }
        if let Some(uri) = outcome.open {
            tasks.push(self.follow_link(&uri));
        }
        Task::batch(tasks)
    }

    fn settings_message(&mut self, message: SettingsMessage) -> Task<Message> {
        match message {
            SettingsMessage::Open => {
                let Some(ws) = &self.workspace else {
                    return Task::none();
                };
                self.settings = Some(SettingsForm {
                    crew: ws.crew_folder.clone(),
                    compact: theme::compact(),
                    error: None,
                });
                Task::none()
            }
            SettingsMessage::Close => {
                self.settings = None;
                theme::set_compact(settings::prefs().compact);
                Task::none()
            }
            SettingsMessage::Crew(value) => {
                if let Some(form) = &mut self.settings {
                    form.crew = value;
                    form.error = None;
                }
                Task::none()
            }
            SettingsMessage::Browse => {
                let Some(ws) = &self.workspace else {
                    return Task::none();
                };
                // Start in the folder already set, if it's there.
                let current = self.settings.as_ref().map(|f| f.crew.trim().trim_matches('/').to_owned()).unwrap_or_default();
                let start = Some(ws.vault.path(&current)).filter(|p| !current.is_empty() && p.is_dir()).unwrap_or_else(|| ws.vault.root.clone());
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title("Choose the crew folder (inside the vault)")
                            .set_directory(start)
                            .pick_folder()
                            .await
                            .map(|handle| handle.path().to_path_buf())
                    },
                    |picked| Message::Settings(SettingsMessage::Picked(picked)),
                )
            }
            SettingsMessage::Compact(on) => {
                // Applies at once, so you can see it; Cancel puts it back.
                if let Some(form) = &mut self.settings {
                    form.compact = on;
                }
                theme::set_compact(on);
                Task::none()
            }
            SettingsMessage::Picked(None) => Task::none(),
            SettingsMessage::Picked(Some(path)) => {
                let Some(ws) = &self.workspace else {
                    return Task::none();
                };
                let Some(form) = &mut self.settings else {
                    return Task::none();
                };
                let root = std::fs::canonicalize(&ws.vault.root).unwrap_or_else(|_| ws.vault.root.clone());
                let picked = std::fs::canonicalize(&path).unwrap_or(path);
                match picked.strip_prefix(&root) {
                    Ok(rel) if rel.as_os_str().is_empty() => {
                        form.error = Some("That's the vault itself; pick a folder inside it".into());
                    }
                    Ok(rel) => {
                        form.crew = rel.to_string_lossy().replace('\\', "/");
                        form.error = None;
                    }
                    Err(_) => {
                        form.error = Some(format!("That folder isn't inside this vault ({})", ws.vault.root.display()));
                    }
                }
                Task::none()
            }
            SettingsMessage::Save => {
                let Some(form) = self.settings.take() else {
                    return Task::none();
                };
                let folder = form.crew.replace('\\', "/").trim().trim_matches('/').to_owned();
                let folder = if folder.is_empty() { crew::FOLDER.to_owned() } else { folder };
                // This computer's preferences.
                let prefs = settings::Prefs { compact: form.compact };
                if prefs != settings::prefs() {
                    settings::save_prefs(prefs);
                }
                theme::set_compact(form.compact);
                let Some(ws) = &mut self.workspace else {
                    return Task::none();
                };
                if folder == ws.crew_folder {
                    return Task::none();
                }
                let exists = ws.vault.note(SETTINGS_NOTE).is_some();
                let task = if exists {
                    let Some(text) = self.current_text(SETTINGS_NOTE) else {
                        self.status = Some("⚠ Couldn't read _settings.md".into());
                        return Task::none();
                    };
                    let new = crew::set_frontmatter(&text, crew::SETTING, &folder);
                    let save = self.put_text(SETTINGS_NOTE, &new);
                    Task::batch([save, self.note_changed(SETTINGS_NOTE)])
                } else {
                    let path = ws.vault.path(SETTINGS_NOTE);
                    let text = crew::set_frontmatter("", crew::SETTING, &folder);
                    if let Err(error) = obsession_core::write::atomic_write(&path, text.as_bytes()) {
                        self.status = Some(format!("⚠ Couldn't create _settings.md: {error}"));
                        return Task::none();
                    }
                    let root = ws.vault.root.clone();
                    Task::perform(async move { Vault::scan(&root).map_err(|e| e.to_string()) }, Message::VaultRefreshed)
                };
                let Some(ws) = &mut self.workspace else {
                    return task;
                };
                ws.crew_folder = folder.clone();
                ws.load_crew();
                let n = ws.crew.len();
                self.status = Some(format!(
                    "Crew folder: {folder} · {n} {} · saved in _settings.md",
                    if n == 1 { "person" } else { "people" }
                ));
                self.refresh_links();
                self.rebuild_graph();
                task
            }
        }
    }

    fn view_settings<'a>(&'a self, form: &'a SettingsForm, ws: &'a Workspace) -> Element<'a, Message> {
        let wanted = form.crew.replace('\\', "/").trim().trim_matches('/').to_lowercase();
        // Folders to pick from: those whose path contains what's typed.
        let folders: Vec<&str> = ws
            .vault
            .entries
            .iter()
            .filter(|e| e.kind == Kind::Folder)
            .map(|e| &*e.rel)
            .filter(|rel| wanted.is_empty() || rel.to_lowercase().contains(&wanted))
            .take(8)
            .collect();
        let folder = if wanted.is_empty() { crew::FOLDER.to_owned() } else { form.crew.trim().trim_matches('/').to_owned() };
        let seats = crew::seats(&folder, ws.vault.entries.iter().map(|e| (&*e.rel, e.kind == Kind::Folder)));
        let found = ws.vault.entries.iter().any(|e| e.kind == Kind::Folder && e.rel.eq_ignore_ascii_case(&folder));
        let preview = match (found, seats.len()) {
            (false, _) => format!("No folder {folder} yet: crew cards will be created there"),
            (true, 0) => "No one in it yet".to_owned(),
            (true, n) => {
                let names: Vec<String> = seats.iter().take(6).map(|s| format!("@{}", s.key)).collect();
                let more = if n > 6 { format!(" and {} more", n - 6) } else { String::new() };
                format!("{n} {}: {}{more}", if n == 1 { "person" } else { "people" }, names.join(", "))
            }
        };
        let mut picks = column![].spacing(2);
        if !folders.is_empty() {
            picks = picks.push(text("Or pick one of these:").size(11).color(theme::FAINT));
        }
        for rel in folders {
            picks = picks.push(
                button(text(rel).size(theme::SMALL).color(theme::TEXT))
                    .on_press(Message::Settings(SettingsMessage::Crew(rel.to_owned())))
                    .padding([4, 10])
                    .width(Fill)
                    .style(theme::row(rel.eq_ignore_ascii_case(&folder))),
            );
        }
        let panel = column![
            text("Settings").size(18).color(theme::HEADING),
            text("Saved in _settings.md at the vault root, so everyone using this vault shares them.")
                .size(theme::SMALL)
                .color(theme::MUTED),
            rule::horizontal(1),
            text("Crew folder").size(theme::TEXT_SIZE).color(theme::HEADING),
            text("Each note in it is a person (Members/ika.md), and so is each folder in it (Members/Adam/, whose card is Members/Adam/Adam.md).")
                .size(theme::SMALL)
                .color(theme::MUTED),
            row![
                iced::widget::text_input("_crew", &form.crew)
                    .on_input(|v| Message::Settings(SettingsMessage::Crew(v)))
                    .on_submit(Message::Settings(SettingsMessage::Save))
                    .padding(8)
                    .size(theme::SMALL)
                    .style(theme::search),
                button(text("Browse…").size(theme::SMALL))
                    .on_press(Message::Settings(SettingsMessage::Browse))
                    .padding([7, 14])
                    .style(theme::tool(false)),
            ]
            .spacing(8)
            .align_y(iced::Center),
            match &form.error {
                Some(e) => text(format!("⚠ {e}")).size(theme::SMALL).color(theme::WARNING),
                None => text(preview).size(theme::SMALL).color(theme::MENTION),
            },
            picks,
            rule::horizontal(1),
            text("This computer").size(theme::TEXT_SIZE).color(theme::HEADING),
            iced::widget::checkbox(form.compact)
                .label("Compact layout: tighter toolbar, file tree and panels")
                .on_toggle(|on| Message::Settings(SettingsMessage::Compact(on)))
                .text_size(theme::SMALL)
                .size(16),
            row![
                Space::new().width(Fill),
                button(text("Cancel").size(theme::SMALL))
                    .on_press(Message::Settings(SettingsMessage::Close))
                    .padding([5, 14])
                    .style(theme::tool(false)),
                button(text("Save").size(theme::SMALL))
                    .on_press(Message::Settings(SettingsMessage::Save))
                    .padding([5, 14])
                    .style(theme::tool(true)),
            ]
            .spacing(8),
        ]
        .spacing(10);
        let popup = container(panel).padding(18).width(iced::Length::Fixed(560.0)).style(theme::popup);
        iced::widget::opaque(
            iced::widget::mouse_area(
                container(iced::widget::opaque(popup))
                    .padding(iced::Padding::new(0.0).top(80.0))
                    .center_x(Fill)
                    .height(Fill)
                    .style(theme::backdrop),
            )
            .on_press(Message::Settings(SettingsMessage::Close)),
        )
    }

    /// Adds `@member` to the end of task `row` on node card `card`.
    fn assign(&mut self, member: &str, card: usize, row: usize) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        let Some(c) = graph.cards.get(card) else {
            return Task::none();
        };
        let (Some(rel), Some(t)) = (c.rel.clone(), c.tasks.get(row)) else {
            return Task::none();
        };
        let (line, title) = (t.line, t.text.clone());
        let Some(text) = self.current_text(&rel) else {
            return Task::none();
        };
        let mut lines: Vec<&str> = text.split('\n').collect();
        let Some(old) = lines.get(line).copied() else {
            return Task::none();
        };
        let Some(new) = crew::add_mention(old, member) else {
            self.status = Some(format!("@{member} is already on “{title}”"));
            return Task::none();
        };
        lines[line] = &new;
        let changed = lines.join("\n");
        let what = format!("Added @{member} to “{title}” in {}", rel.trim_end_matches(".md"));
        self.edit_elsewhere(&rel, &text, &changed, what)
    }

    /// Changes a note other than the open one from the open note's view
    /// (the graph): saved at once, and undoable from here with Ctrl+Z.
    fn edit_elsewhere(&mut self, rel: &str, before: &str, after: &str, what: String) -> Task<Message> {
        let change = history::Change {
            rel: rel.to_owned(),
            before: before.to_owned(),
            after: after.to_owned(),
        };
        self.edit_files(vec![change], what, false)
    }

    /// Changes notes other than the open one as one undo step. `joined`:
    /// an edit of the open note follows, and undoes with it.
    fn edit_files(&mut self, files: Vec<history::Change>, what: String, joined: bool) -> Task<Message> {
        let open = self.open_fingerprint();
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let open_rel = ws.note.as_ref().map(|n| n.rel.to_string());
        let files: Vec<history::Change> = files.into_iter().filter(|f| Some(&f.rel) != open_rel.as_ref()).collect();
        let puts: Vec<(String, String)> = files.iter().map(|f| (f.rel.clone(), f.after.clone())).collect();
        if let Some(open) = open {
            ws.elsewhere.push(history::Step {
                files,
                open,
                what: what.clone(),
                joined,
            });
        }
        self.status = Some(what);
        self.put_files(&puts)
    }

    /// Puts text in other notes and brings what depends on them up to date.
    fn put_files(&mut self, files: &[(String, String)]) -> Task<Message> {
        let mut tasks = Vec::new();
        for (rel, text) in files {
            tasks.push(self.put_text(rel, text));
            tasks.push(self.note_changed(rel));
        }
        self.refresh_links();
        self.rebuild_graph();
        Task::batch(tasks)
    }

    /// How many links in `source` lead to the note `target`.
    fn links_to(&mut self, source: &str, target: &str) -> usize {
        let Some(text) = self.current_text(source) else {
            return 0;
        };
        let Some(ws) = &self.workspace else {
            return 0;
        };
        links::links(&text)
            .iter()
            .filter(|l| leads_to(&ws.vault, source, l, target))
            .count()
    }

    /// Removes the links in `u.source` that lead to `u.target`, keeping
    /// their words.
    fn unlink(&mut self, u: &Unlinking) -> Task<Message> {
        let Some(text) = self.current_text(&u.source) else {
            return Task::none();
        };
        let Some(ws) = &self.workspace else {
            return Task::none();
        };
        let Some((new, count)) = links::remove_links(&text, |l| leads_to(&ws.vault, &u.source, l, &u.target)) else {
            self.status = Some("That link is gone already".into());
            return Task::none();
        };
        let what = format!(
            "Removed {} to [[{}]] from {}",
            if count == 1 { "the link".to_owned() } else { format!("{count} links") },
            u.target.trim_end_matches(".md"),
            u.source.trim_end_matches(".md"),
        );
        self.edit_elsewhere(&u.source, &text, &new, what)
    }

    /// A click on `@name`: open their crew card, or create it.
    fn open_crew(&mut self, name: &str) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let key = crew::key_of(name);
        let known = ws.crew.iter().find(|m| m.key == key);
        if let Some(m) = known.filter(|m| m.exists) {
            let rel: Box<str> = m.rel.clone().into();
            return self.open(rel);
        }
        // A person's folder without its card yet gets one named like the
        // folder; someone new gets one in the crew folder's style.
        let (rel, name) = match known {
            Some(m) => (m.rel.clone(), m.name.clone()),
            None => (crew::new_card_rel(&ws.crew_folder, name, ws.crew_as_folders()), name.to_owned()),
        };
        let name = name.as_str();
        let path = ws.vault.path(&rel);
        let created = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::OpenOptions::new().write(true).create_new(true).open(&path))
            .and_then(|mut f| std::io::Write::write_all(&mut f, crew::template(name).as_bytes()));
        if let Err(error) = created {
            self.status = Some(format!("Couldn't create a card for @{name}: {error}"));
            return Task::none();
        }
        // Known to the store at once, so the watcher's event for it isn't
        // taken for another app's edit.
        let _ = ws.store.get(&rel);
        self.status = Some(format!("Created a crew card for @{name}"));
        let root = ws.vault.root.clone();
        let rel: Box<str> = rel.into();
        let save = self.save_now();
        Task::batch([
            save,
            Task::perform(
                async move { Vault::scan(root).map_err(|e| e.to_string()) },
                move |result| Message::Rescanned(result, rel.clone()),
            ),
        ])
    }

    /// Shows `@name` suggestions when the caret is right after `@` or a
    /// partly typed name.
    fn update_completion(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let Some(note) = &mut ws.note else { return };
        let matches: Vec<(String, String, String)> = match note.mention_prefix() {
            Some(typed) => {
                let t = typed.to_lowercase();
                let mut found: Vec<(String, String, String)> = ws
                    .crew
                    .iter()
                    .filter(|m| m.key.starts_with(&t) || m.name.to_lowercase().split_whitespace().any(|w| w.starts_with(&t)))
                    .map(|m| (m.key.clone(), m.name.clone(), m.role.clone()))
                    .collect();
                // An exact, finished name needs no suggestion.
                if found.len() == 1 && found[0].0 == t {
                    found.clear();
                }
                found.truncate(6);
                self.completion = Some(Completion { typed, matches: Vec::new() });
                found
            }
            None => Vec::new(),
        };
        let on = !matches.is_empty();
        note.set_completing(on);
        match (&mut self.completion, on) {
            (Some(c), true) => c.matches = matches,
            _ => self.completion = None,
        }
    }

    fn follow_link(&mut self, uri: &str) -> Task<Message> {
        if let Some(name) = uri.strip_prefix('@') {
            return self.open_crew(name);
        }
        if let Some(dest) = uri.strip_prefix(obsession_core::preview::WIKI_SCHEME) {
            let dest = dest.split('|').next().unwrap_or(dest);
            let (target, sub) = match dest.split_once('#') {
                Some((target, sub)) => (target.trim(), Some(sub.trim().to_owned())),
                None => (dest.trim(), None),
            };
            return self.follow_note(target, sub, true);
        }
        if uri.starts_with("https://") || uri.starts_with("http://") || uri.starts_with("mailto:") {
            let _ = std::process::Command::new("rundll32")
                .args(["url.dll,FileProtocolHandler", uri])
                .spawn();
            return Task::none();
        }
        match links::note_url(uri) {
            Some((target, sub)) => self.follow_note(&target, sub, false),
            None => {
                self.status = Some(format!("Can't open {uri}"));
                Task::none()
            }
        }
    }

    /// Opens what a link to `target` (`#sub`) leads to, from the open note.
    /// A `[[wiki link]]` to a note that doesn't exist creates it, as in
    /// Obsidian (at the path the link names, from the vault root).
    fn follow_note(&mut self, target: &str, sub: Option<String>, create: bool) -> Task<Message> {
        let Some(ws) = &mut self.workspace else {
            return Task::none();
        };
        let from = ws.note.as_ref().map(|n| n.rel.clone());
        let jump = sub.map(Jump::Section);
        if target.is_empty() {
            // `[[#heading]]`: this note.
            return match from {
                Some(rel) => self.open_at(rel, jump),
                None => Task::none(),
            };
        }
        match ws.vault.resolve_from(target, from.as_deref()) {
            Resolved::One(entry) => {
                let rel = entry.rel.clone();
                self.open_at(rel, jump)
            }
            Resolved::Ambiguous { picked, count } => {
                let rel = picked.rel.clone();
                let task = self.open_at(rel.clone(), jump);
                self.status = Some(format!(
                    "⚠ [[{target}]] matches {count} notes; opened {}. Add part of the folder path to pick one.",
                    rel.trim_end_matches(".md")
                ));
                task
            }
            Resolved::Attachment(found) => {
                // Opens in the app Windows uses for that kind of file.
                let path = found.map(|rel| ws.vault.path(rel));
                match path {
                    Some(path) if path.is_file() => {
                        let _ = std::process::Command::new("rundll32")
                            .arg("url.dll,FileProtocolHandler")
                            .arg(&path)
                            .spawn();
                    }
                    _ => self.status = Some(format!("Can't find {target} in the vault")),
                }
                Task::none()
            }
            Resolved::NotFound if create => {
                let rel = match vault::note_rel_from_input(target) {
                    Ok(rel) => rel,
                    Err(error) => {
                        self.status = Some(format!("Can't create [[{target}]]: {error}"));
                        return Task::none();
                    }
                };
                match vault::create_note(&ws.vault.root, &rel) {
                    Ok(()) => {
                        let _ = ws.store.get(&rel);
                        self.status = Some(format!("Created {rel}"));
                        let root = ws.vault.root.clone();
                        let rel: Box<str> = rel.into();
                        let save = self.save_now();
                        Task::batch([
                            save,
                            Task::perform(
                                async move { Vault::scan(root).map_err(|e| e.to_string()) },
                                move |result| Message::Rescanned(result, rel.clone()),
                            ),
                        ])
                    }
                    Err(error) => {
                        self.status = Some(format!("Couldn't create {rel}: {error}"));
                        Task::none()
                    }
                }
            }
            Resolved::NotFound => {
                self.status = Some(format!("Can't find {target}"));
                Task::none()
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let watch = self
            .workspace
            .as_ref()
            .map_or_else(Subscription::none, |ws| watch::watch(ws.vault.root.clone()).map(Message::Fs));
        // Only while a selection is dragged: the pointer anywhere in the
        // window, for scrolling at the page's edges.
        let dragging = self.workspace.as_ref().and_then(|ws| ws.note.as_ref()).is_some_and(Note::live_dragging);
        let pointer = if dragging {
            event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::Live(live::Message::Pointer(position)))
                }
                _ => None,
            })
        } else {
            Subscription::none()
        };
        Subscription::batch([
            watch,
            pointer,
            event::listen_with(shortcut_from_event).map(Message::Shortcut),
            window::close_requests().map(Message::CloseRequested),
            event::listen_with(|event, _, _| match event {
                iced::Event::Window(window::Event::Rescaled(scale)) => Some(Message::ScaleFactor(scale)),
                _ => None,
            }),
            window::resize_events().map(|(_, size)| Message::Resized(size)),
            event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Message::Live(live::Message::MouseUp))
                }
                _ => None,
            }),
        ])
    }

    fn view(&self) -> Element<'_, Message> {
        let main: Element<'_, Message> = match &self.workspace {
            None => self.view_welcome(),
            Some(ws) => {
                let sidebar = container(
                    ws.tree
                        .view(&ws.vault, ws.note.as_ref().map(|n| &*n.rel))
                        .map(Message::Sidebar),
                )
                .style(theme::panel)
                .height(Fill);

                let body: Element<'_, Message> = match &ws.note {
                    None => container(
                        text("Pick a note on the left, or press Ctrl+P.")
                            .color(theme::MUTED),
                    )
                    .center(Fill)
                    .into(),
                    Some(note) => {
                        let editor = || {
                            note.view_editor().map(|editor| -> Element<'_, Message> {
                                right_click::right_click(
                                    editor.map(|event| match event {
                                        note::SourceEvent::Action(action) => Message::Edit(action),
                                        note::SourceEvent::Complete => Message::CompleteMention(None),
                                    }),
                                    Message::EditorRightClick,
                                )
                                .into()
                            })
                        };
                        let graph = self.graph.as_ref().filter(|_| self.show_graph).map(|g| {
                            let canvas = match &self.editor {
                                Some(editor) => editor.view().map(Message::Editor),
                                None => g.view().map(Message::Graph),
                            };
                            // Always two layers, so the canvas keeps its pan
                            // and zoom whether or not the bar has buttons.
                            stack![canvas, self.view_graph_bar(ws, note)].into()
                        });
                        let body = match self.mode {
                            _ if graph.is_some() => graph,
                            Mode::Live | Mode::Reading => {
                                note.view_live(&self.pictures).map(|view| view.map(Message::Live))
                            }
                            Mode::Source => editor(),
                            Mode::Split => editor().map(|editor| {
                                let mirror: Element<'_, Message> = match note.view_mirror(&self.pictures) {
                                    Some(view) => view.map(Message::Live),
                                    None => Space::new().into(),
                                };
                                row![
                                    container(editor).width(Fill),
                                    rule::vertical(1),
                                    container(mirror).width(Fill),
                                ]
                                .into()
                            }),
                        };
                        // set_mode keeps the note's body in step with the
                        // mode, so this is only a fallback.
                        body.unwrap_or_else(|| Space::new().into())
                    }
                };

                let page = container(body).style(theme::page).width(Fill).height(Fill);
                // `@name` suggestions, at the bottom of the page.
                let page: Element<'_, Message> = match &self.completion {
                    Some(c) if !c.matches.is_empty() => {
                        let mut list = column![
                            text(format!("@{}…  Tab picks the first, or click", c.typed))
                                .size(11)
                                .color(theme::FAINT)
                        ]
                        .spacing(2);
                        for (k, (key, name, role)) in c.matches.iter().enumerate() {
                            list = list.push(
                                button(
                                    row![
                                        text(format!("@{key}")).size(theme::SMALL).color(theme::MENTION),
                                        text(name.clone()).size(theme::SMALL).color(theme::TEXT),
                                        text(role.clone()).size(theme::SMALL).color(theme::MUTED),
                                    ]
                                    .spacing(10),
                                )
                                .on_press(Message::CompleteMention(Some(key.clone())))
                                .padding([3, 8])
                                .width(Fill)
                                .style(theme::row(k == 0)),
                            );
                        }
                        let popup = container(list).padding(8).width(iced::Length::Fixed(380.0)).style(theme::popup);
                        stack![
                            page,
                            container(popup).align_bottom(Fill).padding(iced::Padding::new(16.0).left(24.0)),
                        ]
                        .into()
                    }
                    // Always a stack, so the editor keeps its place in the
                    // widget tree (and its focus) when suggestions appear.
                    _ => stack![page].into(),
                };
                let mut middle = row![sidebar, page].height(Fill);
                if ws.runs.show {
                    middle = middle.push(rule::vertical(1)).push(ws.runs.view().map(Message::Runs));
                } else if self.show_backlinks && ws.note.is_some() && !(self.editor.is_some() && self.show_graph) {
                    middle = middle.push(rule::vertical(1)).push(
                        backlinks::view(&ws.backlinks, ws.indexed).map(Message::Backlinks),
                    );
                }
                column![self.view_toolbar(ws), middle, self.view_status(ws)].into()
            }
        };

        if let (Some(menu), Some(note)) = (
            &self.menu,
            self.workspace.as_ref().and_then(|ws| ws.note.as_ref()),
        ) {
            return stack![main, menu.view(&note.format, self.window).map(Message::StatusMenu)]
                .into();
        }
        if let Some(form) = &self.new_note {
            return stack![main, form.view().map(Message::NewNote)].into();
        }
        if let Some((picker, _)) = &self.picker {
            return stack![main, picker.view().map(Message::Picker)].into();
        }
        if let (Some(form), Some(ws)) = (&self.settings, &self.workspace) {
            return stack![main, self.view_settings(form, ws)].into();
        }
        if let (Some(find), Some(ws)) = (&self.find, &self.workspace) {
            return stack![main, find.view(ws.search.is_some()).map(Message::Find)].into();
        }
        if let Some(u) = &self.unlinking {
            let links = if u.count == 1 { "the link".to_owned() } else { format!("the {} links", u.count) };
            let question = format!(
                "Remove {links} to [[{}]] from {}?",
                u.target.trim_end_matches(".md"),
                u.source.trim_end_matches(".md")
            );
            let buttons = row![
                Space::new().width(Fill),
                button(text("Cancel").size(theme::SMALL)).padding([5, 12]).style(theme::tool(false)).on_press(Message::Unlink(false)),
                button(text("Remove").size(theme::SMALL)).padding([5, 12]).style(theme::tool(true)).on_press(Message::Unlink(true)),
            ]
            .spacing(8);
            let body = column![
                text(question).size(theme::TEXT_SIZE).color(theme::HEADING),
                text("The words stay; only the link goes. Ctrl+Z puts it back.").size(theme::SMALL).color(theme::FAINT),
                buttons,
            ]
            .spacing(10);
            let popup = container(body).padding(16).width(iced::Length::Fixed(460.0)).style(theme::popup);
            let backdrop = iced::widget::mouse_area(
                container(iced::widget::opaque(popup))
                    .padding(iced::Padding::new(0.0).top(120.0))
                    .center_x(Fill)
                    .height(Fill)
                    .style(theme::backdrop),
            )
            .on_press(Message::Unlink(false));
            return stack![main, iced::widget::opaque(backdrop)].into();
        }
        if let Some(a) = &self.assigning {
            let mut list = column![
                text(format!("Give @{} which task in {}?", a.member, a.title)).size(theme::TEXT_SIZE).color(theme::HEADING)
            ]
            .spacing(4);
            for (row, task, has) in &a.tasks {
                let label = if *has { format!("{task}  · already theirs") } else { task.clone() };
                let b = button(text(label).size(theme::SMALL).color(if *has { theme::FAINT } else { theme::TEXT }))
                    .padding([5, 10])
                    .width(Fill)
                    .style(theme::row(false));
                list = list.push(if *has { b } else { b.on_press(Message::AssignTo(Some((a.card, *row)))) });
            }
            let popup = container(list).padding(14).width(iced::Length::Fixed(480.0)).style(theme::popup);
            let backdrop = iced::widget::mouse_area(
                container(iced::widget::opaque(popup))
                    .padding(iced::Padding::new(0.0).top(90.0))
                    .center_x(Fill)
                    .height(Fill)
                    .style(theme::backdrop),
            )
            .on_press(Message::AssignTo(None));
            return stack![main, iced::widget::opaque(backdrop)].into();
        }
        match (&self.switcher, &self.workspace) {
            (Some(switcher), Some(ws)) => {
                stack![main, switcher.view(&ws.vault).map(Message::Switcher)].into()
            }
            // Always a stack, so the window's widgets keep their state (focus,
            // scroll) when a popup opens or closes.
            _ => stack![main].into(),
        }
    }

    /// The graph's own tools, in a small bar in the graph's top-left corner
    /// (the top toolbar is the same for notes and graphs). In the function
    /// editor only the run button shows.
    fn view_graph_bar<'a>(&'a self, ws: &'a Workspace, note: &'a Note) -> Element<'a, Message> {
        let nowrap = iced::widget::text::Wrapping::None;
        let tool = |label: &'static str, message: Message, active: bool| {
            button(text(label).size(theme::SMALL).wrapping(nowrap))
                .on_press(message)
                .padding(theme::tool_pad())
                .style(theme::tool(active))
        };
        let mut bar = row![].spacing(theme::tool_gap()).align_y(iced::Center);
        if self.editor.is_none() {
            bar = bar
                .push(tool("+ Node", Message::AddNode, false))
                .push(tool("+ Function", Message::AddFunctionNode, false))
                .push((!ws.crew.is_empty()).then(|| tool("Crew", Message::ToggleHand, self.show_hand)))
                .push(tool("Link wires", Message::ToggleLinkWires, self.show_link_wires));
        }
        let stale = ws.runs.stale_in(&note.rel);
        if stale > 0 {
            bar = bar.push(
                button(text(format!("⚠ Run {stale} out of date")).size(theme::SMALL).color(theme::WARNING).wrapping(nowrap))
                    .on_press(Message::RunGraph)
                    .padding(theme::tool_pad())
                    .style(theme::tool(false)),
            );
        }
        let empty = self.editor.is_some() && stale == 0;
        // Opaque: a click between the buttons doesn't reach the cards under
        // it. Only the bar itself; the rest of the canvas stays live.
        let bar: Element<'_, Message> = if empty {
            Space::new().into()
        } else {
            // A narrow graph wraps the buttons onto a second line.
            iced::widget::opaque(container(bar.wrap().vertical_spacing(theme::tool_gap())).padding(4).style(theme::popup))
        };
        container(bar).padding(8).into()
    }

    fn view_toolbar<'a>(&'a self, ws: &'a Workspace) -> Element<'a, Message> {
        let mode_button = |label: &'static str, mode: Mode| {
            button(text(label).size(theme::SMALL))
                .on_press(Message::SetMode(mode))
                .padding(theme::tool_pad())
                .style(theme::tool(self.mode == mode))
        };
        let path = ws
            .note
            .as_ref()
            .map_or(String::new(), |n| n.rel.trim_end_matches(".md").to_owned());

        container(
            row![
                button(text("Open folder").size(theme::SMALL))
                    .on_press(Message::PickFolder)
                    .padding(theme::tool_pad())
                    .style(theme::tool(false)),
                button(text("Settings").size(theme::SMALL))
                    .on_press(Message::Settings(SettingsMessage::Open))
                    .padding(theme::tool_pad())
                    .style(theme::tool(self.settings.is_some())),
                button(text("Search").size(theme::SMALL))
                    .on_press(Message::Shortcut(Shortcut::FindInVault))
                    .padding(theme::tool_pad())
                    .style(theme::tool(self.find.is_some())),
                button(text("+ New note").size(theme::SMALL))
                    .on_press(Message::StartNewNote)
                    .padding(theme::tool_pad())
                    .style(theme::tool(false)),
                // The path takes what's left and is cut off, never the buttons.
                container(text(path).size(theme::SMALL).color(theme::MUTED).wrapping(iced::widget::text::Wrapping::None))
                    .width(Fill)
                    .clip(true),
                row![]
                    .push(self.graph.is_some().then(|| {
                        button(text("Graph").size(theme::SMALL))
                            .on_press(Message::ToggleGraph)
                            .padding(theme::tool_pad())
                            .style(theme::tool(self.show_graph))
                    }))
                    .spacing(theme::tool_gap()),
                button(text(if ws.runs.running { "Runs ●" } else { "Runs" }).size(theme::SMALL))
                    .on_press(Message::ToggleRuns)
                    .padding(theme::tool_pad())
                    .style(theme::tool(ws.runs.show)),
                button(text("Backlinks").size(theme::SMALL))
                    .on_press(Message::ToggleBacklinks)
                    .padding(theme::tool_pad())
                    .style(theme::tool(self.show_backlinks && !ws.runs.show)),
                Space::new().width(8),
                mode_button("Live", Mode::Live),
                mode_button("Source", Mode::Source),
                mode_button("Split", Mode::Split),
                mode_button("Reading", Mode::Reading),
            ]
            .spacing(theme::tool_gap())
            .align_y(iced::Center),
        )
        .padding(theme::bar_pad())
        .style(theme::status_bar)
        .into()
    }

    fn view_status<'a>(&'a self, ws: &'a Workspace) -> Element<'a, Message> {
        let saved = match &ws.note {
            Some(note) if note.unsynced || ws.store.is_dirty(&note.rel) => "Editing…",
            Some(_) => "Saved",
            None => "",
        };
        let statuses = match &ws.note {
            Some(note) => {
                let source = match note.source {
                    Source::Note => "this note",
                    Source::Settings => "_settings.md",
                    Source::Default => "default",
                };
                match note.unknown {
                    0 => format!("Statuses: {source}"),
                    n => format!("Statuses: {source} · ⚠ {n} unknown"),
                }
            }
            None => String::new(),
        };
        let stats = ws.store.stats();
        let info = format!(
            "{} KB in RAM · {} reads · {} writes · {} skipped",
            ws.store.bytes_in_ram().div_ceil(1024),
            stats.reads,
            stats.writes,
            stats.skipped_writes,
        );
        let message = self.status.as_deref().unwrap_or("Ctrl+N new · Ctrl+P find · Ctrl+Shift+F search · Ctrl+E view · Ctrl+O open");
        // On a crew card: their open mentions across the vault.
        let person = ws.note.as_ref().filter(|n| crew::is_card(&n.rel, &ws.crew_folder)).and_then(|n| {
            let i = ws.crew.iter().position(|m| *m.rel == *n.rel)?;
            let m = &ws.crew[i];
            let count = ws.mentions.get(&m.key).copied().unwrap_or(0);
            let badge = ws.vault_badges()[i].map_or(String::new(), |b| format!(" · {}", b.label()));
            Some(format!("@{} · {count} open task{} in the vault{badge}", m.key, if count == 1 { "" } else { "s" }))
        });

        container(
            row![
                text(message).size(theme::SMALL),
                Space::new().width(Fill),
                text(person.unwrap_or_default()).size(theme::SMALL).color(theme::MENTION),
                text(statuses).size(theme::SMALL).color(theme::MUTED),
                text(info).size(theme::SMALL).color(theme::FAINT),
                text(saved).size(theme::SMALL),
            ]
            .spacing(16),
        )
        .padding(theme::tool_pad())
        .style(theme::status_bar)
        .into()
    }

    fn view_welcome(&self) -> Element<'_, Message> {
        let content = column![
            text("Obsession").size(32),
            text("Open a folder of markdown notes to start.").color(theme::MUTED),
            button(text("Open folder…"))
                .on_press(Message::PickFolder)
                .padding([8, 18])
                .style(theme::tool(true)),
            text(self.status.as_deref().unwrap_or(if self.scanning {
                "Reading the folder…"
            } else {
                ""
            }))
            .size(theme::SMALL)
            .color(theme::MUTED),
        ]
        .spacing(14)
        .align_x(iced::Center);

        container(content)
            .center(Fill)
            .style(theme::page)
            .into()
    }
}

/// Where to show a note once it's open.
#[derive(Debug, Clone)]
enum Jump {
    Line(usize),
    /// A `#heading` (or `#a#b`) or `^block`, as written after `#`.
    Section(String),
}

/// The open graph's own functions and the saved ones in `_funct.md`
/// (through the store, so it's read once).
fn load_library(ws: &mut Workspace, graph_text: &str) -> Library {
    let saved = if ws.vault.note(func::SAVED).is_some() {
        ws.store.get(func::SAVED).map(func::blocks).unwrap_or_default()
    } else {
        Vec::new()
    };
    Library {
        local: func::local_blocks(graph_text),
        saved,
    }
}

fn start_timers(id: u64, timers: Vec<(Timer, std::time::Duration)>) -> Task<Message> {
    Task::batch(timers.into_iter().map(|(timer, delay)| {
        Task::perform(timer::after(delay), move |()| Message::Timer(id, timer))
    }))
}

fn window_size() -> (f32, f32) {
    #[cfg(debug_assertions)]
    if let Some((w, h)) = std::env::var("OBSESSION_DEV_SIZE")
        .ok()
        .and_then(|s| s.split_once(',').map(|(w, h)| (w.parse().ok(), h.parse().ok())))
        .and_then(|(w, h)| w.zip(h))
    {
        return (w, h);
    }
    (1280.0, 800.0)
}

/// By default iced asks wgpu for every backend, and on Windows that loads both
/// the Vulkan and the DirectX 12 driver, which costs about 100 MB. Pick DX12
/// alone, on the low-power GPU. Setting `WGPU_BACKEND` or `WGPU_POWER_PREF`
/// yourself still overrides this.
/// Dev: `OBSESSION_DEV_MSAA=1` starts with 4× MSAA (to measure it).
fn dev_msaa() -> bool {
    cfg!(debug_assertions) && std::env::var_os("OBSESSION_DEV_MSAA").is_some()
}

fn graphics_defaults() {
    let defaults: &[(&str, &str)] = if cfg!(windows) {
        &[("WGPU_BACKEND", "dx12"), ("WGPU_POWER_PREF", "low")]
    } else {
        &[("WGPU_POWER_PREF", "low")]
    };
    for (key, value) in defaults {
        if std::env::var_os(key).is_none() {
            // SAFETY: called first thing in `main`, before any other thread
            // exists, so nothing can read the environment concurrently.
            unsafe { std::env::set_var(key, value) };
        }
    }
}

/// Maps key presses to shortcuts. Ctrl combinations count even when a text
/// field has the focus; plain keys only when nothing else used them.
fn shortcut_from_event(
    event: iced::Event,
    status: event::Status,
    _window: window::Id,
) -> Option<Shortcut> {
    let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return None;
    };
    #[cfg(debug_assertions)]
    if std::env::var_os("OBSESSION_DEV_KEYS").is_some() {
        eprintln!("key {key:?} {status:?}");
    }
    if key == Key::Named(Named::F2) {
        return Some(Shortcut::Rename);
    }
    if modifiers.command() {
        return match key.as_ref() {
            Key::Character("o") => Some(Shortcut::OpenFolder),
            Key::Character("n") => Some(Shortcut::NewNote),
            Key::Character("p") => Some(Shortcut::QuickSwitch),
            Key::Character("f" | "F") if modifiers.shift() => Some(Shortcut::FindInVault),
            Key::Character("e") => Some(Shortcut::CycleMode),
            Key::Character("s") => Some(Shortcut::Save),
            Key::Character("z") if modifiers.shift() => Some(Shortcut::Redo),
            Key::Character("z") => Some(Shortcut::Undo),
            Key::Character("y") => Some(Shortcut::Redo),
            // Only when no text box copied first (Reading has none).
            Key::Character("c") if status == event::Status::Ignored => Some(Shortcut::Copy),
            _ => None,
        };
    }
    // Escape always reaches the app: a focused text box (search, the quick
    // switcher, the new-note box) captures it just to drop its own focus,
    // which would otherwise take a second press to close the popup.
    if key == Key::Named(Named::Escape) {
        return Some(Shortcut::Escape);
    }
    if status == event::Status::Captured {
        return None;
    }
    match key {
        Key::Named(Named::ArrowUp) => Some(Shortcut::Up),
        Key::Named(Named::ArrowDown) => Some(Shortcut::Down),
        Key::Named(Named::Tab) if !modifiers.shift() && !modifiers.alt() => Some(Shortcut::Tab),
        _ => None,
    }
}

/// Whether `link`, written in `source`, leads to the note `target`.
fn leads_to(vault: &Vault, source: &str, link: &links::Link, target: &str) -> bool {
    !link.target.is_empty() && vault.resolve_from(&link.target, Some(source)).entry().is_some_and(|e| &*e.rel == target)
}

/// A crew member's portrait file in the vault: `portrait:` as the card
/// gives it (relative to the card), else found by its name anywhere, like
/// a link.
fn portrait_of(vault: &Vault, member: &Member) -> Option<Box<str>> {
    let written = member.portrait.as_deref()?;
    let name = written.rsplit('/').next().unwrap_or(written);
    vault
        .attachment(written, Some(&member.rel))
        .or_else(|| vault.attachment(name, Some(&member.rel)))
        .filter(|rel| obsession_core::live::is_picture(rel))
        .map(Box::from)
}
