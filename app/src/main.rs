// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(debug_assertions)]
mod dev;
mod highlight;
mod new_note;
mod note;
mod settings;
mod sidebar;
mod switcher;
mod theme;
mod timer;

use std::path::PathBuf;

use iced::keyboard::{self, Key, key::Named};
use iced::widget::{
    Space, button, column, container, markdown, operation, row, rule, stack, text, text_editor,
};
use iced::{Element, Fill, Subscription, Task, event, window};

use obsession_core::store::{SaveResult, Store};
use obsession_core::vault::{self, Kind, Resolved, Vault};

use new_note::NewNote;
use note::{Mode, Note, Timer};
use sidebar::Tree;
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
    /// The vault was scanned again after a note was created; open `rel`.
    Rescanned(Result<Vault, String>, Box<str>),
    NewNote(new_note::Message),
    StartNewNote,
    Sidebar(sidebar::Message),
    Edit(text_editor::Action),
    Timer(u64, Timer),
    Saved(SaveResult),
    Link(markdown::Uri),
    SetMode(Mode),
    Shortcut(Shortcut),
    Switcher(switcher::Message),
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
    Up,
    Down,
    Escape,
}

struct Workspace {
    vault: Vault,
    store: Store,
    tree: Tree,
    note: Option<Note>,
}

struct App {
    workspace: Option<Workspace>,
    mode: Mode,
    switcher: Option<Switcher>,
    new_note: Option<NewNote>,
    status: Option<String>,
    scanning: bool,
    next_note_id: u64,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let app = Self {
            workspace: None,
            mode: Mode::default(),
            switcher: None,
            new_note: None,
            status: None,
            scanning: false,
            next_note_id: 0,
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
                self.workspace = Some(Workspace {
                    vault,
                    store,
                    tree,
                    note: None,
                });
                self.switcher = None;
                self.new_note = None;
                #[cfg(debug_assertions)]
                return dev::after_scan(self);
                #[cfg(not(debug_assertions))]
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
                        self.open(rel)
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
                }
            }
            Message::Edit(action) => {
                let Some(note) = self.note_mut() else {
                    return Task::none();
                };
                let id = note.id;
                let timers = note.perform(action);
                Task::batch(timers.into_iter().map(|(timer, delay)| {
                    Task::perform(timer::after(delay), move |()| Message::Timer(id, timer))
                }))
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
            Message::Link(uri) => self.follow_link(&uri),
            Message::SetMode(mode) => {
                self.set_mode(mode);
                Task::none()
            }
            Message::Shortcut(shortcut) => self.shortcut(shortcut),
            Message::Switcher(message) => self.switcher_message(message),
            Message::CloseRequested(id) => {
                self.save_blocking();
                window::close(id)
            }
            #[cfg(debug_assertions)]
            Message::Dev(step) => dev::update(step),
        }
    }

    fn note_mut(&mut self) -> Option<&mut Note> {
        self.workspace.as_mut()?.note.as_mut()
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        if mode.shows_preview()
            && let Some(note) = self.note_mut() {
                note.refresh_preview_if_stale();
            }
    }

    fn shortcut(&mut self, shortcut: Shortcut) -> Task<Message> {
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
                self.set_mode(self.mode.next());
                Task::none()
            }
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
                    self.open_index(index)
                }
                None => Task::none(),
            },
            switcher::Message::Pick(index) => {
                self.switcher = None;
                self.open_index(index)
            }
            switcher::Message::Close => {
                self.switcher = None;
                Task::none()
            }
        }
    }

    fn new_note_message(&mut self, message: new_note::Message) -> Task<Message> {
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
                ws.note = Some(Note::new(id, rel, &text, mode));
                self.status = None;
            }
            Err(error) => {
                self.status = Some(format!("Couldn't open {rel}: {error}"));
            }
        }
        save
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
            ws.store.set(&note.rel, note.text());
            note.unsynced = false;
        }
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
        let Some(ws) = &self.workspace else {
            return Task::none();
        };
        if let Some(target) = uri.strip_prefix(obsession_core::preview::WIKI_SCHEME) {
            return match ws.vault.resolve(target) {
                Resolved::One(entry) => {
                    let rel = entry.rel.clone();
                    self.open(rel)
                }
                Resolved::Ambiguous(n) => {
                    self.status = Some(format!(
                        "⚠ [[{target}]] matches {n} notes. Add part of the folder path."
                    ));
                    Task::none()
                }
                Resolved::NotFound => {
                    self.status = Some(format!("No note called [[{target}]]"));
                    Task::none()
                }
            };
        }
        if uri.starts_with("https://") || uri.starts_with("http://") {
            let _ = std::process::Command::new("rundll32")
                .args(["url.dll,FileProtocolHandler", uri])
                .spawn();
            return Task::none();
        }
        // A plain markdown link to a note: relative to the note, then the root.
        let target = uri.trim_start_matches("./");
        let from_note = ws
            .note
            .as_ref()
            .and_then(|n| n.rel.rsplit_once('/').map(|(dir, _)| format!("{dir}/{target}")));
        let candidates = from_note.into_iter().chain([target.to_owned()]);
        let found = candidates
            .into_iter()
            .find_map(|c| ws.vault.notes().find(|e| *e.rel == *c).map(|e| e.rel.clone()));
        match found {
            Some(rel) => self.open(rel),
            None => {
                self.status = Some(format!("Can't open {uri}"));
                Task::none()
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            event::listen_with(shortcut_from_event).map(Message::Shortcut),
            window::close_requests().map(Message::CloseRequested),
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
                        let editor = || note.view_editor().map(Message::Edit);
                        let preview = || note.view_preview().map(Message::Link);
                        match self.mode {
                            Mode::Edit => editor(),
                            Mode::Preview => preview(),
                            Mode::Split => row![editor(), rule::vertical(1), preview()].into(),
                        }
                    }
                };

                column![
                    self.view_toolbar(ws),
                    row![sidebar, container(body).style(theme::page).width(Fill).height(Fill)]
                        .height(Fill),
                    self.view_status(ws),
                ]
                .into()
            }
        };

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
                mode_button("Edit", Mode::Edit),
                mode_button("Split", Mode::Split),
                mode_button("Preview", Mode::Preview),
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
    if modifiers.command() {
        return match key.as_ref() {
            Key::Character("o") => Some(Shortcut::OpenFolder),
            Key::Character("n") => Some(Shortcut::NewNote),
            Key::Character("p") => Some(Shortcut::QuickSwitch),
            Key::Character("e") => Some(Shortcut::CycleMode),
            Key::Character("s") => Some(Shortcut::Save),
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
