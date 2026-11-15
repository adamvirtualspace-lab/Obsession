// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backlinks;
#[cfg(debug_assertions)]
mod dev;
mod graph;
mod source_scroll;
mod highlight;
mod live;
mod live_text;
mod new_note;
mod note;
mod right_click;
mod settings;
mod sidebar;
mod status_menu;
mod switcher;
mod theme;
mod timer;

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
use obsession_core::nodes::{self, Section};

use crate::highlight::LinkStates;
use obsession_core::vault::{self, Kind, Resolved, Vault};

use new_note::NewNote;
use note::{Mode, Note, Timer};
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
        .antialiasing(false)
        .window_size(window_size())
        .exit_on_close_request(false)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
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
    /// Pick a note to add to the open graph.
    AddNode,
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
    Up,
    Down,
    Escape,
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
}

impl App {
    fn boot() -> (Self, Task<Message>) {
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
        };
        let start = std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(settings::last_vault);
        match start {
            Some(path) => {
                let mut app = app;
                let task = app.scan(path);
                (app, task)
            }
            None => (app, Task::none()),
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

    fn scan(&mut self, path: PathBuf) -> Task<Message> {
        self.scanning = true;
        Task::perform(
            async move { Vault::scan(path).map_err(|e| e.to_string()) },
            Message::Scanned,
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
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
                settings::remember_vault(&vault.root);
                let store = Store::new(&vault.root);
                let tree = Tree::new(&vault);
                self.status = Some(format!("{} notes", vault.notes().count()));
                // Read every note's links on a background thread.
                let root = vault.root.clone();
                let rels: Vec<Box<str>> = vault.notes().map(|e| e.rel.clone()).collect();
                let index = Task::perform(
                    async move {
                        let index = LinkIndex::build(&root, rels.iter().map(|r| &**r));
                        (root, Arc::new(index))
                    },
                    |(root, index)| Message::Indexed(root, index),
                );
                let mut ws = Workspace {
                    vault,
                    store,
                    tree,
                    note: None,
                    team: None,
                    links: LinkIndex::default(),
                    indexed: false,
                    backlinks: Vec::new(),
                };
                if let Some(warning) = ws.load_team().into_iter().next() {
                    self.status = Some(format!("_settings.md: {warning}"));
                }
                self.workspace = Some(ws);
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
                if self.show_graph {
                    self.rebuild_graph();
                }
                Task::none()
            }
            Message::ToggleLinkWires => {
                self.show_link_wires = !self.show_link_wires;
                if let Some(graph) = &mut self.graph {
                    graph.show_links = self.show_link_wires;
                    graph.touch();
                }
                Task::none()
            }
            Message::AddNode => {
                let Some(ws) = &self.workspace else {
                    return Task::none();
                };
                self.adding_node = true;
                self.switcher = Some(Switcher::new(&ws.vault));
                operation::focus(switcher::input_id())
            }
            Message::ToggleBacklinks => {
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
                        // A new note: empty, so nothing to read for its links.
                        if ws.links.outline(&rel).is_none() {
                            ws.links.update(&rel, "");
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
                start_timers(id, performed.timers)
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
            Message::Saved(result) => {
                if let Some(ws) = &mut self.workspace {
                    ws.store.finish_save(&result);
                }
                if let Some(error) = result.error() {
                    self.status = Some(format!("Couldn't save {}: {error}", result.rel()));
                }
                Task::none()
            }
            Message::SetMode(mode) => {
                self.show_graph = false;
                self.set_mode(mode)
            }
            Message::Live(message) => {
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
                if let Some((task, at)) = outcome.menu {
                    self.menu = Some(StatusMenu { task, at });
                }
                if let Some(uri) = outcome.open {
                    tasks.push(self.follow_link(&uri));
                }
                Task::batch(tasks)
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

    fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
        if self.menu.take().is_some() && matches!(shortcut, Shortcut::Escape) {
            return Task::none();
        }
        if self.new_note.is_some() {
            if matches!(shortcut, Shortcut::Escape | Shortcut::NewNote) {
                self.new_note = None;
            }
            return Task::none();
        }
        if let Some(switcher) = &mut self.switcher {
            match shortcut {
                Shortcut::Up => switcher.move_selection(false),
                Shortcut::Down => switcher.move_selection(true),
                Shortcut::Escape | Shortcut::QuickSwitch => self.switcher = None,
                _ => {}
            }
            return Task::none();
        }
        match shortcut {
            Shortcut::QuickSwitch => {
                let Some(ws) = &self.workspace else {
                    return Task::none();
                };
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
            Shortcut::Up | Shortcut::Down | Shortcut::Escape => Task::none(),
        }
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
        let (Some(form), Some(ws)) = (&mut self.new_note, &self.workspace) else {
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
        let mut graph = graph::Graph::build(
            &text,
            &rel,
            &ws.vault,
            &mut ws.store,
            ws.team.as_ref(),
            &ws.links,
            self.show_link_wires,
        );
        graph.fit = self.graph_fit;
        self.graph = Some(graph);
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
                    self.status = Some("Function nodes open in milestone 5".into());
                    return Task::none();
                }
                let key = c.key.clone();
                self.follow_note(&key, None, true)
            }
            graph::Message::Connect(from, to) => {
                let (Some(a), Some(b)) = (graph.cards.get(from), graph.cards.get(to)) else {
                    return Task::none();
                };
                if a.kind == graph::CardKind::Function || b.kind == graph::CardKind::Function {
                    self.status = Some("Wiring function pins comes in milestone 5".into());
                    return Task::none();
                }
                if graph.wires.iter().any(|w| w.from == from && w.to == to && !w.link) {
                    return Task::none();
                }
                let item = format!("[[{}]] → [[{}]]", a.key, b.key);
                self.edit_graph(|text| nodes::add_item(text, Section::Wires, &item), true)
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
                        let note = graph.cards[w.to].title.clone();
                        self.status = Some(format!(
                            "That wire is a [[link]] in {note}; remove the link there, or hide link wires"
                        ));
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
        let mode = self.mode;
        self.next_note_id += 1;
        let id = self.next_note_id;

        let Some(ws) = &mut self.workspace else {
            return save;
        };
        if let Some(old) = ws.note.take() {
            ws.store.unpin(&old.rel);
        }
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
        let states = links::link_states(&text, &note.rel, &ws.vault, &ws.links);
        note.set_link_states(LinkStates::new(states));
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
        if note.unsynced {
            let text = note.text();
            note.analyze(&text);
            ws.links.update(&note.rel, &text);
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
        match ws.store.take_save(&note.rel) {
            Some(job) => Task::perform(async move { job.run() }, Message::Saved),
            None => Task::none(),
        }
    }

    /// Used when closing: write on this thread so nothing is lost.
    fn save_blocking(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let Some(note) = &mut ws.note else { return };
        if note.unsynced {
            ws.store.set(&note.rel, note.text());
            note.unsynced = false;
        }
        if let Some(job) = ws.store.take_save(&note.rel) {
            let result = job.run();
            ws.store.finish_save(&result);
        }
    }

    fn follow_link(&mut self, uri: &str) -> Task<Message> {
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
        let Some(ws) = &self.workspace else {
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
            Resolved::Attachment => {
                let path = ws.vault.root.join(target);
                if path.is_file() {
                    let _ = std::process::Command::new("rundll32")
                        .arg("url.dll,FileProtocolHandler")
                        .arg(&path)
                        .spawn();
                } else {
                    self.status = Some(format!("Can't find {target} (attachments open from the vault root for now)"));
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
        Subscription::batch([
            event::listen_with(shortcut_from_event).map(Message::Shortcut),
            window::close_requests().map(Message::CloseRequested),
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
                                    editor.map(Message::Edit),
                                    Message::EditorRightClick,
                                )
                                .into()
                            })
                        };
                        let graph = self
                            .graph
                            .as_ref()
                            .filter(|_| self.show_graph)
                            .map(|g| g.view().map(Message::Graph));
                        let body = match self.mode {
                            _ if graph.is_some() => graph,
                            Mode::Live | Mode::Reading => {
                                note.view_live().map(|view| view.map(Message::Live))
                            }
                            Mode::Source => editor(),
                            Mode::Split => editor().map(|editor| {
                                let mirror: Element<'_, Message> = match note.view_mirror() {
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
                let mut middle = row![sidebar, page].height(Fill);
                if self.show_backlinks && ws.note.is_some() {
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
        match (&self.switcher, &self.workspace) {
            (Some(switcher), Some(ws)) => {
                stack![main, switcher.view(&ws.vault).map(Message::Switcher)].into()
            }
            _ => main,
        }
    }

    fn view_toolbar<'a>(&'a self, ws: &'a Workspace) -> Element<'a, Message> {
        let mode_button = |label: &'static str, mode: Mode| {
            button(text(label).size(theme::SMALL))
                .on_press(Message::SetMode(mode))
                .padding([4, 12])
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
                    .padding([4, 12])
                    .style(theme::tool(false)),
                button(text("+ New note").size(theme::SMALL))
                    .on_press(Message::StartNewNote)
                    .padding([4, 12])
                    .style(theme::tool(false)),
                text(path).size(theme::SMALL).color(theme::MUTED),
                Space::new().width(Fill),
                row![]
                    .push(self.graph.is_some().then(|| {
                        button(text("Graph").size(theme::SMALL))
                            .on_press(Message::ToggleGraph)
                            .padding([4, 12])
                            .style(theme::tool(self.show_graph))
                    }))
                    .push((self.graph.is_some() && self.show_graph).then(|| {
                        button(text("+ Node").size(theme::SMALL))
                            .on_press(Message::AddNode)
                            .padding([4, 12])
                            .style(theme::tool(false))
                    }))
                    .push((self.graph.is_some() && self.show_graph).then(|| {
                        button(text("Link wires").size(theme::SMALL))
                            .on_press(Message::ToggleLinkWires)
                            .padding([4, 12])
                            .style(theme::tool(self.show_link_wires))
                    }))
                    .spacing(10),
                button(text("Backlinks").size(theme::SMALL))
                    .on_press(Message::ToggleBacklinks)
                    .padding([4, 12])
                    .style(theme::tool(self.show_backlinks)),
                Space::new().width(8),
                mode_button("Live", Mode::Live),
                mode_button("Source", Mode::Source),
                mode_button("Split", Mode::Split),
                mode_button("Reading", Mode::Reading),
            ]
            .spacing(10)
            .align_y(iced::Center),
        )
        .padding([8, 12])
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
        let message = self.status.as_deref().unwrap_or("Ctrl+N new · Ctrl+P find · Ctrl+E view · Ctrl+O open");

        container(
            row![
                text(message).size(theme::SMALL),
                Space::new().width(Fill),
                text(statuses).size(theme::SMALL).color(theme::MUTED),
                text(info).size(theme::SMALL).color(theme::FAINT),
                text(saved).size(theme::SMALL),
            ]
            .spacing(16),
        )
        .padding([4, 12])
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
    if key == Key::Named(Named::F2) {
        return Some(Shortcut::Rename);
    }
    if modifiers.command() {
        return match key.as_ref() {
            Key::Character("o") => Some(Shortcut::OpenFolder),
            Key::Character("n") => Some(Shortcut::NewNote),
            Key::Character("p") => Some(Shortcut::QuickSwitch),
            Key::Character("e") => Some(Shortcut::CycleMode),
            Key::Character("s") => Some(Shortcut::Save),
            Key::Character("z") if modifiers.shift() => Some(Shortcut::Redo),
            Key::Character("z") => Some(Shortcut::Undo),
            Key::Character("y") => Some(Shortcut::Redo),
            _ => None,
        };
    }
    if status == event::Status::Captured {
        return None;
    }
    match key {
        Key::Named(Named::ArrowUp) => Some(Shortcut::Up),
        Key::Named(Named::ArrowDown) => Some(Shortcut::Down),
        Key::Named(Named::Escape) => Some(Shortcut::Escape),
        _ => None,
    }
}
