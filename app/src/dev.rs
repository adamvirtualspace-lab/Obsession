//! Debug-build helpers for checking the UI without clicking around.
//! Compiled out of release builds entirely.
//!
//! Environment variables, read after a vault is opened:
//! - `OBSESSION_DEV_OPEN=anim/sh040.md`  open a note
//! - `OBSESSION_DEV_MODE=edit|split|preview`
//! - `OBSESSION_DEV_FIND=query`  open the quick switcher with a query
//! - `OBSESSION_DEV_NEW=name`  open the new-note box and type `name` after the folder
//! - `OBSESSION_DEV_NEW_SUBMIT=1`  then press Enter in it
//! - `OBSESSION_DEV_TYPE=text`  type text at the start of the open note
//! - `OBSESSION_DEV_SHOT=C:\path\shot.bmp`  screenshot after `OBSESSION_DEV_WAIT` ms (default 1500)
//! - `OBSESSION_DEV_EXIT=1`  quit after the screenshot
//! - `OBSESSION_DEV_SIZE=w,h`  start with this window size (read at startup)

use std::time::Duration;

use iced::{Task, window};

use crate::note::Mode;
use crate::{App, Message};

#[derive(Debug, Clone)]
pub enum Step {
    Shot(window::Screenshot),
}

pub fn after_scan(app: &mut App) -> Task<Message> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let mut tasks = Vec::new();

    if let Some(mode) = var("OBSESSION_DEV_MODE") {
        app.set_mode(match mode.as_str() {
            "edit" => Mode::Edit,
            "preview" => Mode::Preview,
            _ => Mode::Split,
        });
    }
    if let Some(rel) = var("OBSESSION_DEV_OPEN") {
        tasks.push(app.open(rel.into_boxed_str()));
    }
    if let Some(query) = var("OBSESSION_DEV_FIND") {
        tasks.push(app.shortcut(crate::Shortcut::QuickSwitch));
        tasks.push(app.switcher_message(crate::switcher::Message::Query(query)));
    }
    if let Some(name) = var("OBSESSION_DEV_NEW") {
        tasks.push(app.shortcut(crate::Shortcut::NewNote));
        let input = format!("{}{name}", app.new_note.as_ref().map_or("", |f| f.input.as_str()));
        tasks.push(app.new_note_message(crate::new_note::Message::Input(input)));
        if var("OBSESSION_DEV_NEW_SUBMIT").is_some() {
            tasks.push(app.new_note_message(crate::new_note::Message::Submit));
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

pub fn update(step: Step) -> Task<Message> {
    match step {
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
