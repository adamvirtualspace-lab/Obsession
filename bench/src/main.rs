//! obsession-bench: generate test vaults and time the core.
//!
//! ```text
//! obsession-bench gen  <dir> <notes>   write a test vault (refuses a non-empty dir)
//! obsession-bench scan <dir>           time a vault scan
//! obsession-bench read <dir>           time reading every note through the store, twice
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use obsession_core::{Store, Vault};

const DEPARTMENTS: [&str; 6] = ["anim", "lighting", "fx", "comp", "layout", "rig"];
const PEOPLE: [&str; 7] = ["ika", "maya", "noor", "teo", "sun-hee", "bram", "unit7"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, dir, n] if cmd == "gen" => match n.parse() {
            Ok(n) => generate(Path::new(dir), n),
            Err(_) => Err(format!("not a number: {n}")),
        },
        [cmd, dir] if cmd == "scan" => scan(Path::new(dir)),
        [cmd, dir] if cmd == "read" => read(Path::new(dir)),
        _ => Err("usage: obsession-bench gen <dir> <notes> | scan <dir> | read <dir>".into()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn generate(dir: &Path, notes: usize) -> Result<(), String> {
    if dir.exists() && fs::read_dir(dir).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("{} is not empty; pick a new folder", dir.display()));
    }
    let start = Instant::now();
    let mut bytes = 0;
    for i in 0..notes {
        let dept = DEPARTMENTS[i % DEPARTMENTS.len()];
        let episode = i / 400;
        let shot = i % 400;
        let folder: PathBuf = dir.join(format!("ep{episode:02}")).join(dept);
        fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let text = note_text(dept, episode, shot, i);
        bytes += text.len();
        fs::write(folder.join(format!("sh{shot:03}.md")), text).map_err(|e| e.to_string())?;
    }
    println!(
        "wrote {notes} notes ({} KB) in {:.0?}",
        bytes / 1024,
        start.elapsed()
    );
    Ok(())
}

fn note_text(dept: &str, episode: usize, shot: usize, i: usize) -> String {
    let who = PEOPLE[i % PEOPLE.len()];
    let other = PEOPLE[(i / 3) % PEOPLE.len()];
    let mut text = format!(
        "# {dept} – ep{episode:02} sh{shot:03}\n\nNotes for the shot. See [[ep{episode:02}/anim/sh{shot:03}]].\n\n"
    );
    for t in 0..8 {
        let status = [" ", "x", "?", "-"][(i + t) % 4];
        text.push_str(&format!("- [{status}] Task {t} for sh{shot:03} @{who} ^t{t}\n"));
    }
    text.push_str(&format!("\n- [ ] Review with @{other}\n"));
    text
}

fn scan(dir: &Path) -> Result<(), String> {
    let start = Instant::now();
    let vault = Vault::scan(dir).map_err(|e| e.to_string())?;
    let elapsed = start.elapsed();
    let index_bytes: usize = vault
        .entries
        .iter()
        .map(|e| e.rel.len() + std::mem::size_of_val(e))
        .sum();
    println!(
        "scanned {} entries ({} notes) in {elapsed:.0?}; tree index ≈ {} KB",
        vault.entries.len(),
        vault.notes().count(),
        index_bytes / 1024
    );
    Ok(())
}

fn read(dir: &Path) -> Result<(), String> {
    let vault = Vault::scan(dir).map_err(|e| e.to_string())?;
    let rels: Vec<&str> = vault.notes().map(|e| &*e.rel).collect();
    let mut store = Store::new(dir);

    for pass in 1..=2 {
        let start = Instant::now();
        for rel in &rels {
            store.pin(rel).map_err(|e| format!("{rel}: {e}"))?;
        }
        let stats = store.stats();
        println!(
            "pass {pass}: {} notes in {:.0?}; disk reads so far {}, {} KB in RAM",
            rels.len(),
            start.elapsed(),
            stats.reads,
            store.bytes_in_ram() / 1024
        );
    }
    Ok(())
}
