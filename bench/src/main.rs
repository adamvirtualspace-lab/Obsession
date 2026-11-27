//! obsession-bench: generate test vaults and time the core.
//!
//! ```text
//! obsession-bench gen  <dir> <notes>   write a test vault (refuses a non-empty dir)
//! obsession-bench scan <dir>           time a vault scan
//! obsession-bench read <dir>           time reading every note through the store, twice
//! obsession-bench links <dir>          time building the link index and backlink lookups
//! obsession-bench graphs <dir> <wires> add graphs with that many function wires to a `gen` vault
//! obsession-bench runs <dir>           time the function index and the out-of-date check
//! obsession-bench search <dir>         time building search and its memory
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use obsession_core::{Store, Vault};

/// Counts heap bytes, so `search` can say what the index holds.
struct Counting;
static HELD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        use std::sync::atomic::Ordering::Relaxed;
        let now = HELD.fetch_add(layout.size(), Relaxed) + layout.size();
        PEAK.fetch_max(now, Relaxed);
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        HELD.fetch_sub(layout.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn held() -> usize {
    HELD.load(std::sync::atomic::Ordering::Relaxed)
}

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
        [cmd, dir] if cmd == "links" => links(Path::new(dir)),
        [cmd, dir, n] if cmd == "graphs" => match n.parse() {
            Ok(n) => graphs(Path::new(dir), n),
            Err(_) => Err(format!("not a number: {n}")),
        },
        [cmd, dir] if cmd == "runs" => runs(Path::new(dir)),
        [cmd, dir] if cmd == "search" => search(Path::new(dir)),
        _ => Err("usage: obsession-bench gen <dir> <notes> | scan <dir> | read <dir> | links <dir> | graphs <dir> <wires> | runs <dir> | search <dir>".into()),
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

fn links(dir: &Path) -> Result<(), String> {
    let vault = Vault::scan(dir).map_err(|e| e.to_string())?;
    let rels: Vec<&str> = vault.notes().map(|e| &*e.rel).collect();
    use obsession_core::links::LinkIndex;
    // Where the time goes: reading, then each part of indexing a note.
    let start = Instant::now();
    let texts: Vec<(&str, String)> = rels
        .iter()
        .filter_map(|r| Some((*r, fs::read_to_string(dir.join(r)).ok()?)))
        .collect();
    println!("read {} notes in {:.0?}", texts.len(), start.elapsed());
    let time = |label: &str, f: &dyn Fn(&str)| {
        let start = Instant::now();
        for (_, t) in &texts {
            f(t);
        }
        println!("  {label}: {:.0?}", start.elapsed());
    };
    time("links", &|t| drop(obsession_core::links::links(t)));
    time("outline", &|t| drop(obsession_core::links::outline(t)));
    time("task mentions", &|t| drop(obsession_core::crew::task_mentions(t)));
    time("frontmatter", &|t| drop(obsession_core::status::from_frontmatter(t.lines())));
    let start = Instant::now();
    let mut idx = LinkIndex::default();
    for (rel, t) in &texts {
        idx.update(rel, t);
    }
    println!("  update(): {:.0?}", start.elapsed());
    let start = Instant::now();
    let (index, stats) = LinkIndex::build_cached(dir, rels.iter().copied(), None);
    println!("indexed {} notes in {:.0?} ({} read)", index.len(), start.elapsed(), stats.parsed);
    let start = Instant::now();
    let cache = index.to_cache();
    println!("cache: {} KB, written in {:.0?}", cache.len() / 1024, start.elapsed());
    let start = Instant::now();
    let (again, stats) = LinkIndex::build_cached(dir, rels.iter().copied(), Some(&cache));
    println!(
        "from the cache: {} notes in {:.0?} ({} read, {} cached)",
        again.len(),
        start.elapsed(),
        stats.parsed,
        stats.cached
    );
    let start = Instant::now();
    let mut total = 0;
    for rel in rels.iter().take(1000) {
        total += index.backlinks(rel, &vault).len();
    }
    let n = rels.len().min(1000);
    println!("backlinks for {n} notes in {:.0?} ({total} found)", start.elapsed());
    // Unlinked mentions: what opening a note costs once they're wanted.
    let mut search = obsession_core::search::SearchIndex::default();
    for (rel, t) in &texts {
        search.update(rel, t);
    }
    let start = Instant::now();
    let mut lines = 0;
    for rel in rels.iter().take(100) {
        lines += search.unlinked(rel, 200).total;
    }
    let n = rels.len().min(100);
    println!(
        "unlinked mentions for {n} notes in {:.0?} ({:.1} ms each, {lines} lines found)",
        start.elapsed(),
        start.elapsed().as_secs_f64() * 1000.0 / n as f64
    );
    Ok(())
}

/// Graphs of 10 wires each, every wire running Wait For Done from one
/// department's shot to the next department's, plus `_funct.md`.
fn graphs(dir: &Path, wires: usize) -> Result<(), String> {
    let vault = Vault::scan(dir).map_err(|e| e.to_string())?;
    if vault.note(obsession_core::func::SAVED).is_none() {
        fs::write(dir.join(obsession_core::func::SAVED), obsession_core::func::SAVED_DEFAULT).map_err(|e| e.to_string())?;
    }
    let folder = dir.join("graphs");
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let shots: Vec<&str> = vault.notes().map(|e| &*e.rel).filter(|r| r.starts_with("ep")).collect();
    let mut written = 0;
    for g in 0..wires.div_ceil(10) {
        let mut text = String::from("# Pipeline

## Wires
");
        for w in 0..10.min(wires - g * 10) {
            let i = (g * 10 + w) * 7 % shots.len().max(1);
            let (Some(a), Some(b)) = (shots.get(i), shots.get(i + 1)) else { break };
            let a = a.strip_suffix(".md").unwrap_or(a);
            let b = b.strip_suffix(".md").unwrap_or(b);
            text.push_str(&format!("- [[{a}]] → [[{b}]] : Wait For Done(task: \"Task {}\", target: \"Task {}\")
", w % 8, (w + 1) % 8));
            written += 1;
        }
        fs::write(folder.join(format!("g{g:03}_nodes.md")), text).map_err(|e| e.to_string())?;
    }
    println!("wrote {} graphs, {written} wires", wires.div_ceil(10));
    Ok(())
}

/// What `rebuild_index` and `check_out_of_date` cost in the app.
fn runs(dir: &Path) -> Result<(), String> {
    use obsession_core::{func, nodes, runner};
    let vault = Vault::scan(dir).map_err(|e| e.to_string())?;
    let mut store = Store::new(dir);
    let start = Instant::now();
    let rels: Vec<String> = vault.notes().filter(|e| nodes::is_graph(&e.rel)).map(|e| e.rel.to_string()).collect();
    let graphs: Vec<(String, String)> = rels
        .into_iter()
        .filter_map(|rel| Some((rel.clone(), store.get(&rel).ok()?.to_owned())))
        .collect();
    let saved = store.get(func::SAVED).map(func::blocks).unwrap_or_default();
    println!("read {} graphs and _funct.md in {:.0?}", graphs.len(), start.elapsed());
    let start = Instant::now();
    let index = runner::Index::build(&vault, &graphs, &saved);
    println!("index: {} jobs in {:.0?}", index.jobs.len(), start.elapsed());
    let start = Instant::now();
    let files = index.files();
    for rel in &files {
        store.pin(rel).map_err(|e| format!("{rel}: {e}"))?;
    }
    println!("pinned {} notes in {:.0?}", files.len(), start.elapsed());
    let start = Instant::now();
    let texts: std::collections::HashMap<String, String> =
        files.iter().filter_map(|r| Some((r.clone(), store.get(r).ok()?.to_owned()))).collect();
    println!("gathered their texts in {:.0?}", start.elapsed());
    let start = Instant::now();
    let pending = runner::pending(&index, &texts, None);
    println!("out of date: {} of {} in {:.0?}", pending.len(), index.jobs.len(), start.elapsed());
    Ok(())
}

/// Building search, and what it holds.
fn search(dir: &Path) -> Result<(), String> {
    let vault = Vault::scan(dir).map_err(|e| e.to_string())?;
    let rels: Vec<&str> = vault.notes().map(|e| &*e.rel).collect();
    let before = held();
    PEAK.store(before, std::sync::atomic::Ordering::Relaxed);
    let start = Instant::now();
    let index = obsession_core::search::SearchIndex::build(dir, rels.iter().copied());
    let peak = PEAK.load(std::sync::atomic::Ordering::Relaxed) - before;
    println!(
        "search: {} notes in {:.0?}, {} KB of text; heap {} KB held, {} KB at the peak",
        index.len(),
        start.elapsed(),
        index.bytes() / 1024,
        (held() - before) / 1024,
        peak / 1024
    );
    let start = Instant::now();
    let r = index.query("task review", 300);
    println!("query: {} lines in {} notes in {:.0?}", r.total, r.notes, start.elapsed());
    Ok(())
}
