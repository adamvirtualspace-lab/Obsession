//! Debug-build helpers for checking the UI without clicking around.
//! Compiled out of release builds entirely.
//!
//! Environment variables, read after a vault is opened:
//! - `OBSESSION_DEV_OPEN=anim/sh040.md`  open a note
//! - `OBSESSION_DEV_MODE=live|source|split|reading`
//! - `OBSESSION_DEV_FIND=query`  open the quick switcher with a query
//! - `OBSESSION_DEV_VSEARCH=ms;query`  open vault search after `ms`, type `query`
//!   after another `ms`, close it after a third (to watch memory)
//! - `OBSESSION_DEV_NEW=name`  open the new-note box and type `name` after the folder
//! - `OBSESSION_DEV_NEW_SUBMIT=1`  then press Enter in it
//! - `OBSESSION_DEV_CLICK=x,y`  left-click the editor text area at this point
//! - `OBSESSION_DEV_RIGHT=x,y[,wx,wy]`  right-click the editor (widget point, window point)
//! - `OBSESSION_DEV_PICK=c`  pick status `c` in the open status menu
//! - `OBSESSION_DEV_LPRESS=line,offset[,link]`  press a drawn Live Preview line
//! - `OBSESSION_DEV_LDRAG=line,offset`  after LPRESS, drag to here before letting go
//! - `OBSESSION_DEV_LTYPE=text{enter}…`  type into the Live Preview editor
//! - `OBSESSION_DEV_SCROLL=lines[,steps]`  wheel the Source editor
//! - `OBSESSION_DEV_VSCROLL=y[,y…]`  scroll the Live view (Split's right pane)
//!   to `y`, then to each next `y` 400 ms later
//! - `OBSESSION_DEV_RENAME=new/name`  rename the open note
//! - `OBSESSION_DEV_FOLLOW=wiki:target#sub`  follow a link from the open note
//! - `OBSESSION_DEV_GRAPH=move,0,10,20;connect,0,1`  graph actions on the open `_nodes.md`
//! - `OBSESSION_DEV_TYPE=text`  type text at the start of the open note
//! - `OBSESSION_DEV_SHOT=C:\path\shot.bmp`  screenshot after `OBSESSION_DEV_WAIT` ms (default 1500)
//! - `OBSESSION_DEV_EXIT=1`  quit after the screenshot
//! - `OBSESSION_DEV_SIZE=w,h`  start with this window size (read at startup)
//! - `OBSESSION_DEV_TIMING=1`  print how long the link index and search took (stderr)
//! - `OBSESSION_DEV_PULSE_MS=8000`  slow down the run glow to look at it
//! - `OBSESSION_DEV_PAN=600`  pan the open graph by itself for 600 frames,
//!   then print frame intervals and cached-layer build times (stderr); with
//!   `ICED_PRESENT_MODE=immediate` the frames aren't held to the display
//! - `OBSESSION_DEV_MSAA=1`  start with iced's 4× MSAA (read at startup)
//! - `OBSESSION_DEV_KEYS=1`  print each key press, whether a widget used it,
//!   and what the Live and Source editors' key bindings saw (stderr)

use std::time::Duration;

use iced::{Task, window};

use crate::note::Mode;
use crate::{App, Message};

#[derive(Debug, Clone)]
pub enum Step {
    Shot(window::Screenshot),
    /// A message sent after a delay, once the window has been laid out
    /// (editor clicks need real text positions).
    Later(Box<Message>),
    /// Scroll the Live view (Split's right pane) to this offset, as the user
    /// dragging its scrollbar would.
    ScrollView(f32),
    /// Follow a link, as a click on it would.
    Follow(String),
    /// Add the note with this name to the open graph.
    AddNode(String),
    /// Undo the newest run.
    UndoRun,
}

fn later(ms: u64, message: Message) -> Task<Message> {
    Task::perform(crate::timer::after(Duration::from_millis(ms)), move |()| {
        Message::Dev(Step::Later(Box::new(message.clone())))
    })
}

pub fn after_scan(app: &mut App) -> Task<Message> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let mut tasks = Vec::new();

    if let Some(mode) = var("OBSESSION_DEV_MODE") {
        tasks.push(app.set_mode(match mode.as_str() {
            "source" | "edit" => Mode::Source,
            "split" => Mode::Split,
            "reading" | "preview" => Mode::Reading,
            _ => Mode::Live,
        }));
    }
    if let Some(rel) = var("OBSESSION_DEV_OPEN") {
        tasks.push(app.open(rel.into_boxed_str()));
    }
    if let Some(query) = var("OBSESSION_DEV_FIND") {
        tasks.push(app.shortcut(crate::Shortcut::QuickSwitch));
        tasks.push(app.switcher_message(crate::switcher::Message::Query(query)));
    }
    if let Some(spec) = var("OBSESSION_DEV_VSEARCH")
        && let Some((ms, query)) = spec.split_once(';')
        && let Ok(ms) = ms.parse::<u64>()
    {
        tasks.push(later(ms, Message::Shortcut(crate::Shortcut::FindInVault)));
        tasks.push(later(2 * ms, Message::Find(crate::find::Message::Query(query.to_owned()))));
        tasks.push(later(3 * ms, Message::Find(crate::find::Message::Close)));
    }
    if let Some(name) = var("OBSESSION_DEV_NEW") {
        tasks.push(app.shortcut(crate::Shortcut::NewNote));
        let input = format!("{}{name}", app.new_note.as_ref().map_or("", |f| f.input.as_str()));
        tasks.push(app.new_note_message(crate::new_note::Message::Input(input)));
        if var("OBSESSION_DEV_NEW_SUBMIT").is_some() {
            tasks.push(app.new_note_message(crate::new_note::Message::Submit));
        }
    }
    let point = |name: &str| {
        var(name).and_then(|v| {
            let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (n.len() >= 2).then_some(n)
        })
    };
    if let Some(p) = point("OBSESSION_DEV_CLICK") {
        let action = iced::widget::text_editor::Action::Click(iced::Point::new(p[0], p[1]));
        tasks.push(later(400, Message::Edit(action)));
    }
    if let Some(p) = point("OBSESSION_DEV_RIGHT") {
        let local = iced::Point::new(p[0], p[1]);
        let window = iced::Point::new(p.get(2).copied().unwrap_or(p[0]), p.get(3).copied().unwrap_or(p[1]));
        tasks.push(later(400, Message::EditorRightClick(local, window)));
    }
    if let Some(ch) = var("OBSESSION_DEV_PICK").and_then(|v| v.chars().next()) {
        tasks.push(later(700, Message::StatusMenu(crate::status_menu::Message::Pick(ch))));
    }
    // Live Preview: press a drawn line at a drawn-text byte offset.
    if let Some(p) = point("OBSESSION_DEV_LPRESS") {
        let press = crate::live_text::Press {
            offset: p[1] as usize,
            link: p.get(2).map(|l| *l as usize),
            right: false,
            window: iced::Point::new(300.0, 300.0),
        };
        tasks.push(later(
            400,
            Message::Live(crate::live::Message::Press {
                line: p[0] as usize,
                cell: None,
                press,
            }),
        ));
        // Then a drag to another line, if asked, and the button going up.
        if let Some(d) = point("OBSESSION_DEV_LDRAG") {
            tasks.push(later(
                450,
                Message::Live(crate::live::Message::DragAt {
                    line: d[0] as usize,
                    cell: None,
                    offset: d[1] as usize,
                }),
            ));
        }
        // `OBSESSION_DEV_HOLD=1` keeps the button down (to see a drag).
        if var("OBSESSION_DEV_HOLD").is_none() {
            tasks.push(later(500, Message::Live(crate::live::Message::MouseUp)));
        }
    }
    // Live Preview: type into the active editor. `{enter}`, `{bs}`, `{del}`,
    // `{up}`, `{down}`, `{left}`, `{right}`, `{esc}`, `{undo}`, `{redo}` are keys.
    if let Some(script) = var("OBSESSION_DEV_LTYPE") {
        let mut delay = 600;
        for step in live_script(&script) {
            tasks.push(later(delay, step));
            delay += 30;
        }
    }
    // Rename the open note (as typed in the rename box).
    if let Some(input) = var("OBSESSION_DEV_RENAME") {
        tasks.push(later(500, Message::Shortcut(crate::Shortcut::Rename)));
        tasks.push(later(600, Message::NewNote(crate::new_note::Message::Input(input))));
        tasks.push(later(700, Message::NewNote(crate::new_note::Message::Submit)));
    }
    // Follow a link from the open note: `wiki:target#sub`, a URL or a path.
    if let Some(uri) = var("OBSESSION_DEV_FOLLOW") {
        tasks.push(later(800, Message::Dev(Step::Follow(uri))));
    }
    // Graph actions, `;`-separated: `move,card,x,y` `connect,a,b`
    // `delcard,card` `delwire,wire` `open,card` `add,name`.
    if let Some(script) = var("OBSESSION_DEV_GRAPH") {
        use crate::graph::{Message as G, Selected};
        let mut delay = 900;
        for step in script.split(';') {
            let parts: Vec<&str> = step.split(',').map(str::trim).collect();
            let n = |i: usize| parts.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
            let message = match parts.first().copied() {
                Some("move") => Message::Graph(G::Moved(n(1) as usize, iced::Point::new(n(2), n(3)))),
                Some("connect") => {
                    // `connect,a,b[,from pin,to pin]` (-1: no pin).
                    let pin = |i: usize| parts.get(i).and_then(|v| v.parse::<usize>().ok());
                    Message::Graph(G::Connect(
                        crate::graph::PinRef { card: n(1) as usize, pin: pin(3) },
                        crate::graph::PinRef { card: n(2) as usize, pin: pin(4) },
                    ))
                }
                // The ƒ button on wire `n`.
                Some("fn") => Message::Graph(G::Function(n(1) as usize)),
                // Runs: run the graph's out-of-date functions, show the
                // Runs panel, undo the newest run.
                Some("run") => Message::RunGraph,
                Some("runs") => Message::ToggleRuns,
                Some("undorun") => Message::Dev(Step::UndoRun),
                // Crew: `assign,member,card[,row]` (a crew card dropped on a
                // node card, or one of its task rows), `hand` (show/hide).
                Some("assign") => Message::Graph(G::Assign {
                    member: n(1) as usize,
                    card: n(2) as usize,
                    task: parts.get(3).and_then(|v| v.parse().ok()),
                }),
                Some("assignto") => Message::AssignTo(Some((n(1) as usize, n(2) as usize))),
                Some("hand") => Message::ToggleHand,
                // Settings: `settings` opens the page; `crew,folder` types the
                // crew folder; `save` saves.
                Some("settings") => Message::Settings(crate::SettingsMessage::Open),
                Some("crew") => Message::Settings(crate::SettingsMessage::Crew(parts[1..].join(","))),
                Some("save") => Message::Settings(crate::SettingsMessage::Save),
                Some("compact") => Message::Settings(crate::SettingsMessage::Compact(parts.get(1) != Some(&"off"))),
                // Vault search: `search` opens it, `query,words` types, `pickhit`
                // opens the selected line.
                Some("search") => Message::Shortcut(crate::Shortcut::FindInVault),
                Some("query") => Message::Find(crate::find::Message::Query(parts[1..].join(","))),
                Some("pickhit") => Message::Find(crate::find::Message::Submit),
                // As if the folder picker returned this path.
                Some("picked") => Message::Settings(crate::SettingsMessage::Picked(Some(parts[1..].join(",").into()))),
                // The function picker: `pick,Name`, `create,Name`.
                Some("pick") => Message::Picker(crate::func_editor::PickerMessage::Pick(parts.get(1).unwrap_or(&"").to_string())),
                Some("create") => {
                    tasks.push(later(
                        delay,
                        Message::Picker(crate::func_editor::PickerMessage::Name(parts.get(1).unwrap_or(&"").to_string())),
                    ));
                    delay += 100;
                    Message::Picker(crate::func_editor::PickerMessage::Create)
                }
                // The function editor: `edadd,Op`, `edsel,step`,
                // `edwire,step,input,source`, `edset,step,input,value`,
                // `edparam,k,value`, `edhead,line`, `edback`.
                Some(op) if op.starts_with("ed") => {
                    use crate::func_editor::{Message as E, Sel};
                    let text = |i: usize| parts[i.min(parts.len())..].join(",");
                    let m = match op {
                        "edadd" => E::AddNode(
                            obsession_core::func::library::find(parts.get(1).unwrap_or(&""))
                                .map_or("Len", |s| s.name),
                        ),
                        "edsel" => E::Select(Some(Sel::Step(n(1) as usize))),
                        "edwire" => E::Connect {
                            step: n(1) as usize,
                            input: n(2) as usize,
                            source: parts.get(3).unwrap_or(&"").to_string(),
                            from: None,
                        },
                        "edset" => {
                            tasks.push(later(delay, Message::Editor(E::Select(Some(Sel::Step(n(1) as usize))))));
                            tasks.push(later(delay + 50, Message::Editor(E::Draft(n(2) as usize, text(3)))));
                            delay += 100;
                            E::Submit(n(2) as usize)
                        }
                        "edparam" => {
                            tasks.push(later(delay, Message::Editor(E::ParamDraft(n(1) as usize, text(2)))));
                            delay += 50;
                            E::ParamSubmit
                        }
                        "edhead" => {
                            tasks.push(later(delay, Message::Editor(E::Select(Some(Sel::Entry)))));
                            tasks.push(later(delay + 50, Message::Editor(E::HeaderDraft(text(1)))));
                            delay += 100;
                            E::HeaderSubmit
                        }
                        "edback" => E::Back,
                        _ => continue,
                    };
                    Message::Editor(m)
                }
                Some("delcard") => Message::Graph(G::Delete(Selected::Card(n(1) as usize))),
                Some("delwire") => Message::Graph(G::Delete(Selected::Wire(n(1) as usize))),
                Some("open") => Message::Graph(G::Open(n(1) as usize)),
                Some("add") => Message::Dev(Step::AddNode(parts.get(1).unwrap_or(&"").to_string())),
                _ => continue,
            };
            tasks.push(later(delay, message));
            delay += 150;
        }
    }
    // Wheel the Source editor: `lines` per step, `steps` times.
    if let Some(spec) = var("OBSESSION_DEV_SCROLL") {
        let mut parts = spec.split(',').filter_map(|v| v.trim().parse::<i32>().ok());
        let lines = parts.next().unwrap_or(3);
        let steps = parts.next().unwrap_or(1);
        for i in 0..steps {
            let action = iced::widget::text_editor::Action::Scroll { lines };
            tasks.push(later(700 + 40 * i as u64, Message::Edit(action)));
        }
    }
    // Scroll Split's right pane to an offset.
    if let Some(list) = var("OBSESSION_DEV_VSCROLL") {
        for (i, y) in list.split(',').filter_map(|v| v.trim().parse::<f32>().ok()).enumerate() {
            tasks.push(Task::perform(
                crate::timer::after(Duration::from_millis(900 + 400 * i as u64)),
                move |()| Message::Dev(Step::ScrollView(y)),
            ));
        }
    }
    if let Some(typed) = var("OBSESSION_DEV_TYPE") {
        for ch in typed.chars() {
            let action = iced::widget::text_editor::Action::Edit(
                iced::widget::text_editor::Edit::Insert(ch),
            );
            tasks.push(app.update(Message::Edit(action)));
        }
    }
    if var("OBSESSION_DEV_SHOT").is_some() {
        let wait = var("OBSESSION_DEV_WAIT")
            .and_then(|w| w.parse().ok())
            .unwrap_or(1500);
        tasks.push(
            Task::perform(crate::timer::after(Duration::from_millis(wait)), |()| ())
                .then(|()| window::latest())
                .and_then(window::screenshot)
                .map(|shot| Message::Dev(Step::Shot(shot))),
        );
    }
    Task::batch(tasks)
}

fn live_script(script: &str) -> Vec<Message> {
    use crate::live::{Intent, Message as L};
    use iced::widget::text_editor::{Action, Edit};
    let mut out = Vec::new();
    let mut rest = script;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('{')
            && let Some(close) = after.find('}')
        {
            let key = &after[..close];
            rest = &after[close + 1..];
            let message = match key {
                "enter" => L::Key(Intent::Enter),
                "bs" => L::Key(Intent::JoinPrev),
                "del" => L::Key(Intent::JoinNext),
                "up" => L::Key(Intent::Up),
                "down" => L::Key(Intent::Down),
                "left" => L::Key(Intent::Left),
                "right" => L::Key(Intent::Right),
                "esc" => L::Key(Intent::Escape),
                // Tab with `@name` suggestions showing.
                "tab" => L::Key(Intent::Complete),
                "back" => L::Edit(Action::Edit(Edit::Backspace)),
                "undo" => {
                    out.push(Message::Shortcut(crate::Shortcut::Undo));
                    continue;
                }
                "redo" => {
                    out.push(Message::Shortcut(crate::Shortcut::Redo));
                    continue;
                }
                _ => continue,
            };
            out.push(Message::Live(message));
            continue;
        }
        let ch = rest.chars().next().unwrap_or(' ');
        rest = &rest[ch.len_utf8()..];
        out.push(Message::Live(L::Edit(Action::Edit(Edit::Insert(ch)))));
    }
    out
}

pub fn update(step: Step) -> Task<Message> {
    match step {
        Step::Later(_) | Step::Follow(_) | Step::AddNode(_) | Step::UndoRun => Task::none(),
        Step::ScrollView(y) => iced::widget::operation::scroll_to(
            crate::live::scroll_id(),
            iced::widget::scrollable::AbsoluteOffset { x: 0.0, y },
        ),
        Step::Shot(shot) => {
            if let Ok(path) = std::env::var("OBSESSION_DEV_SHOT")
                && let Err(error) = write_bmp(&path, &shot) {
                    eprintln!("screenshot failed: {error}");
                }
            if std::env::var_os("OBSESSION_DEV_EXIT").is_some() {
                return iced::exit();
            }
            Task::none()
        }
    }
}

/// A 32-bit top-down BMP: no image crate needed for a debug helper.
fn write_bmp(path: &str, shot: &window::Screenshot) -> std::io::Result<()> {
    let (w, h) = (shot.size.width, shot.size.height);
    let pixels = w as usize * h as usize * 4;
    let mut out = Vec::with_capacity(54 + pixels);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + pixels as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for px in shot.rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    std::fs::write(path, out)
}
