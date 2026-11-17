//! Running functions (Syntax.md, section 10).
//!
//! [`Index`] finds every function the vault uses: each wire with a function
//! in every graph file, and each `fn Name` node with all its pins wired. A
//! [`Job`] is one of those, ready to run: compiled and checked once, its
//! ports resolved to notes.
//!
//! [`run`] evaluates jobs in memory against a set of texts. A job's changes
//! may change a file another job reads; that job runs too, in the same run
//! (a chain). Each line can change at most once per run: a second change is
//! skipped and reported as a loop. Nothing here touches the disk; the caller
//! writes each changed file once.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::func::check::{self, Checked};
use crate::func::eval::{self, Bind, Patch};
use crate::func::{self, Arg, Block, Function, Library, Literal, PortKind, Value};
use crate::nodes::{self, End, NodeKind};
use crate::status::Format;
use crate::vault::Vault;

/// At most this many job evaluations per run, chains included.
pub const MAX_STEPS: usize = 500;

/// One function, wired and ready to run.
#[derive(Debug, Clone)]
pub struct Job {
    /// The graph file it's in.
    pub graph: String,
    /// The wire's line, or the `fn Name` node's line.
    pub line: usize,
    /// On a wire (`true`) or a function node.
    pub on_wire: bool,
    pub function: Function,
    pub checked: Checked,
    /// The wire's values for the params (`None`: the default).
    pub params: Vec<Option<Literal>>,
    /// Each port (ins, then outs): the note's path and `#sub`.
    pub ports: Vec<(String, Option<String>)>,
}

impl Job {
    pub fn name(&self) -> &str {
        &self.function.name
    }

    /// The notes this job reads (its `in`s).
    pub fn inputs(&self) -> impl Iterator<Item = &str> {
        self.ports[..self.function.inputs.len()].iter().map(|(rel, _)| rel.as_str())
    }

    /// Every note it touches.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.ports.iter().map(|(rel, _)| rel.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Index {
    pub jobs: Vec<Job>,
    /// note → jobs with an `in` in it.
    readers: HashMap<String, Vec<usize>>,
}

/// The values a call gives each param, by name or position.
pub fn params_from(f: &Function, args: &[Arg]) -> Vec<Option<Literal>> {
    let mut out = vec![None; f.params.len()];
    for (k, arg) in args.iter().enumerate() {
        let index = match &arg.name {
            Some(n) => f.params.iter().position(|p| p.name.eq_ignore_ascii_case(n)),
            None => (k < f.params.len()).then_some(k),
        };
        if let (Some(i), Value::Lit(l)) = (index, &arg.value) {
            out[i] = Some(l.clone());
        }
    }
    out
}

/// What kind of port a wire end fits: a note is a File, `#^id` a Task,
/// `#Heading` a Section.
pub fn end_kind(end: &End) -> Option<PortKind> {
    match end {
        End::Note { sub: None, .. } => Some(PortKind::File),
        End::Note { sub: Some(s), .. } if s.starts_with('^') => Some(PortKind::Task),
        End::Note { .. } => Some(PortKind::Section),
        End::Pin { .. } => None,
    }
}

impl Index {
    /// Reads every graph (`(rel, text)`) with the saved functions.
    /// Functions that don't check out, or aren't fully wired, are left out:
    /// the graph view shows why.
    pub fn build(vault: &Vault, graphs: &[(String, String)], saved: &[Block]) -> Self {
        let mut index = Index::default();
        for (graph, text) in graphs {
            let library = Library {
                local: func::local_blocks(text),
                saved: saved.to_vec(),
            };
            let parsed = nodes::parse(text);
            let resolve = |end: &End| -> Option<(String, Option<String>)> {
                let End::Note { target, sub } = end else { return None };
                let entry = vault.resolve_from(target, Some(graph)).entry()?;
                Some((entry.rel.to_string(), sub.clone()))
            };
            let ready = |f: &Function| -> Option<Checked> {
                let checked = check::check(f);
                checked.ok().then_some(checked)
            };

            // Functions on wires.
            for wire in &parsed.wires {
                let Some(call) = &wire.function else { continue };
                let Ok((name, args)) = func::parse_call(call) else { continue };
                let Some((block, _, _)) = library.find(&name) else { continue };
                let f = &block.function;
                let Some((kin, kout)) = f.wire_shape() else { continue };
                if end_kind(&wire.from) != Some(kin) || end_kind(&wire.to) != Some(kout) {
                    continue;
                }
                if !check::check_call(f, &args).is_empty() {
                    continue;
                }
                let (Some(checked), Some(from), Some(to)) = (ready(f), resolve(&wire.from), resolve(&wire.to)) else {
                    continue;
                };
                index.jobs.push(Job {
                    graph: graph.clone(),
                    line: wire.line,
                    on_wire: true,
                    params: params_from(f, &args),
                    function: f.clone(),
                    checked,
                    ports: vec![from, to],
                });
            }

            // Function nodes, wired by their pins.
            for node in &parsed.nodes {
                let NodeKind::Function { name } = &node.kind else { continue };
                let Some((block, _, _)) = library.find(name) else { continue };
                let f = &block.function;
                let Some(checked) = ready(f) else { continue };
                let ins = f.inputs.len();
                let mut ports: Vec<Option<(String, Option<String>)>> = vec![None; ins + f.outputs.len()];
                for wire in &parsed.wires {
                    if let End::Pin { function, pin } = &wire.to
                        && function.eq_ignore_ascii_case(name)
                        && let Some(k) = f.inputs.iter().position(|p| p.name.eq_ignore_ascii_case(pin))
                        && end_kind(&wire.from) == Some(f.inputs[k].kind)
                    {
                        ports[k] = resolve(&wire.from);
                    }
                    if let End::Pin { function, pin } = &wire.from
                        && function.eq_ignore_ascii_case(name)
                        && let Some(k) = f.outputs.iter().position(|p| p.name.eq_ignore_ascii_case(pin))
                        && end_kind(&wire.to) == Some(f.outputs[k].kind)
                    {
                        ports[ins + k] = resolve(&wire.to);
                    }
                }
                let Some(ports) = ports.into_iter().collect::<Option<Vec<_>>>() else {
                    continue;
                };
                index.jobs.push(Job {
                    graph: graph.clone(),
                    line: node.line,
                    on_wire: false,
                    params: vec![None; f.params.len()],
                    function: f.clone(),
                    checked,
                    ports,
                });
            }
        }
        for (i, job) in index.jobs.iter().enumerate() {
            for rel in job.inputs() {
                let readers = index.readers.entry(rel.to_owned()).or_default();
                if !readers.contains(&i) {
                    readers.push(i);
                }
            }
        }
        index
    }

    /// Every note a job touches (to keep in RAM).
    pub fn files(&self) -> Vec<String> {
        let mut all: Vec<String> = self.jobs.iter().flat_map(|j| j.files().map(str::to_owned)).collect();
        all.sort();
        all.dedup();
        all
    }

    /// Jobs with an input in any of these notes.
    pub fn affected<'a>(&self, changed: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
        let mut out: Vec<usize> = changed
            .into_iter()
            .filter_map(|rel| self.readers.get(rel))
            .flatten()
            .copied()
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn reads(&self, rel: &str) -> bool {
        self.readers.contains_key(rel)
    }

    /// The job for a wire (or node) line of a graph.
    pub fn find(&self, graph: &str, line: usize) -> Option<usize> {
        self.jobs.iter().position(|j| j.graph == graph && j.line == line)
    }
}

/// One change a run made: lines `line..line + old.len()` of `rel` became
/// `new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub rel: String,
    pub line: usize,
    pub old: Vec<String>,
    pub new: Vec<String>,
    /// The function that made it.
    pub by: String,
}

#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// The final text of each changed note.
    pub texts: HashMap<String, String>,
    /// Every change, in the order it was made.
    pub changes: Vec<Change>,
    /// Jobs that ran (each once, in first-run order).
    pub ran: Vec<usize>,
    /// Why a job's action didn't fire (`Find Task` matched nothing, …).
    pub blocked: HashMap<usize, String>,
    /// Loops and other things the run skipped.
    pub warnings: Vec<String>,
}

impl Outcome {
    /// "Wait For Done: 1 change in comp/sh040"
    pub fn summary(&self, index: &Index) -> String {
        let mut names: Vec<&str> = self.ran.iter().map(|&j| index.jobs[j].name()).collect();
        names.dedup();
        let names = match names.len() {
            0 => "Nothing".to_owned(),
            1..=3 => names.join(", "),
            n => format!("{} and {} more", names[..2].join(", "), n - 2),
        };
        let files: HashSet<&str> = self.changes.iter().map(|c| c.rel.as_str()).collect();
        let mut files: Vec<&str> = files.into_iter().collect();
        files.sort_unstable();
        match (self.changes.len(), files.as_slice()) {
            (0, _) => format!("{names}: nothing to change"),
            (1, [f]) => format!("{names}: 1 change in {}", f.trim_end_matches(".md")),
            (n, [f]) => format!("{names}: {n} changes in {}", f.trim_end_matches(".md")),
            (n, fs) => format!("{names}: {n} changes in {} notes", fs.len()),
        }
    }
}

/// Lines a patch touches, for the loop guard.
fn touched(p: &Patch) -> std::ops::Range<usize> {
    p.line..p.line + p.old.len().max(1)
}

/// Runs `start` (and whatever they chain into) against `files`, which must
/// hold every note the index touches. Nothing is written.
pub fn run(index: &Index, files: &HashMap<String, String>, start: &[usize], team: Option<&Format>) -> Outcome {
    let mut out = Outcome::default();
    let mut texts: HashMap<String, String> = HashMap::new();
    // Lines changed so far, per note.
    let mut changed_lines: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut queue: VecDeque<usize> = start.iter().copied().collect();
    let mut queued: HashSet<usize> = queue.iter().copied().collect();
    let mut steps = 0;

    while let Some(j) = queue.pop_front() {
        queued.remove(&j);
        steps += 1;
        if steps > MAX_STEPS {
            out.warnings.push(format!("Stopped after {MAX_STEPS} chained runs; something keeps changing"));
            break;
        }
        let job = &index.jobs[j];
        if !out.ran.contains(&j) {
            out.ran.push(j);
        }
        let text_of = |rel: &str, texts: &HashMap<String, String>| -> Option<String> {
            texts.get(rel).or_else(|| files.get(rel)).cloned()
        };
        let binds: Vec<Option<Bind>> = job
            .ports
            .iter()
            .map(|(rel, sub)| {
                Some(Bind {
                    rel: rel.clone(),
                    text: text_of(rel, &texts)?,
                    sub: sub.clone(),
                })
            })
            .collect();
        let result = eval::eval(&job.function, &job.checked, &binds, &job.params, team);
        out.blocked.remove(&j);
        for (k, step) in job.function.steps.iter().enumerate() {
            let action = func::library::find(&step.op).is_some_and(|s| s.is_action());
            if action && let Some(Err(why)) = result.values.get(k) {
                out.blocked.entry(j).or_insert_with(|| why.clone());
            }
        }

        // Apply per note, bottom up, so earlier lines keep their numbers.
        let mut patches = result.patches;
        patches.sort_by(|a, b| a.rel.cmp(&b.rel).then(b.line.cmp(&a.line)));
        let mut changed_now: Vec<String> = Vec::new();
        for p in patches {
            let Some(current) = text_of(&p.rel, &texts) else { continue };
            let seen = changed_lines.entry(p.rel.clone()).or_default();
            if touched(&p).any(|l| seen.contains(&l)) {
                out.warnings.push(format!(
                    "Loop: {} would change line {} of {} a second time in one run; skipped",
                    job.name(),
                    p.line + 1,
                    p.rel.trim_end_matches(".md")
                ));
                continue;
            }
            let applied = eval::apply_patches(&current, &[&p]);
            if applied == current {
                continue;
            }
            // Lines below move by the difference.
            let delta = p.new.len() as isize - p.old.len() as isize;
            let end = p.line + p.old.len();
            let moved: HashSet<usize> = seen
                .iter()
                .map(|&l| if l >= end { (l as isize + delta).max(0) as usize } else { l })
                .collect();
            *seen = moved;
            seen.extend(p.line..p.line + p.new.len().max(1));
            out.changes.push(Change {
                rel: p.rel.clone(),
                line: p.line,
                old: p.old,
                new: p.new,
                by: job.name().to_owned(),
            });
            texts.insert(p.rel.clone(), applied);
            if !changed_now.contains(&p.rel) {
                changed_now.push(p.rel);
            }
        }
        // Chains: whoever reads what just changed runs next.
        for k in index.affected(changed_now.iter().map(String::as_str)) {
            if k != j && queued.insert(k) {
                queue.push_back(k);
            }
        }
    }
    out.texts = texts;
    out
}

/// Which jobs would change something if they ran now (each on its own, no
/// chains): (job, what it would do). Used to show "out of date".
pub fn pending(index: &Index, files: &HashMap<String, String>, team: Option<&Format>) -> Vec<(usize, String)> {
    (0..index.jobs.len())
        .filter_map(|j| {
            let one = run_single(index, files, j, team);
            (!one.changes.is_empty()).then(|| (j, one.summary(index)))
        })
        .collect()
}

fn run_single(index: &Index, files: &HashMap<String, String>, j: usize, team: Option<&Format>) -> Outcome {
    let mut lone = Index {
        jobs: vec![index.jobs[j].clone()],
        readers: HashMap::new(),
    };
    let mut out = run(&lone, files, &[0], team);
    // Report against the real index.
    out.ran = vec![j];
    out.blocked = out.blocked.into_values().map(|v| (j, v)).collect();
    lone.jobs.clear();
    out
}

/// Makes `changes` (in the order a run made them) on `text`, which may have
/// moved on since the run read it. A change applies only where its old
/// lines are still there. Returns the text and the changes that applied.
pub fn apply<'a>(text: &str, changes: &[&'a Change]) -> (String, Vec<&'a Change>) {
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned()).collect();
    let mut applied = Vec::new();
    for &c in changes {
        let end = c.line + c.old.len();
        if end > lines.len() || lines[c.line..end] != c.old[..] {
            continue;
        }
        lines.splice(c.line..end, c.new.iter().cloned());
        applied.push(c);
    }
    (lines.join(ending), applied)
}

/// Undoes `changes` (one run's, in the order they were made) on `text`,
/// newest first. A change is reverted only if its lines still hold what it
/// wrote. Returns the text and how many changes were skipped.
pub fn revert(text: &str, changes: &[&Change]) -> (String, usize) {
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned()).collect();
    let mut skipped = 0;
    for c in changes.iter().rev() {
        let end = c.line + c.new.len();
        if end > lines.len() || lines[c.line..end] != c.new[..] {
            skipped += 1;
            continue;
        }
        lines.splice(c.line..end, c.old.iter().cloned());
    }
    (lines.join(ending), skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::{Entry, Kind};
    use std::path::PathBuf;

    fn vault(rels: &[&str]) -> Vault {
        let entries = rels
            .iter()
            .map(|r| Entry {
                rel: (*r).into(),
                kind: Kind::Note,
                depth: r.matches('/').count() as u16,
            })
            .collect();
        Vault::from_entries(PathBuf::from("v"), entries)
    }

    const FORMAT: &str = "---\nstatuses:\n  - \"[-] ⚪ cannot start\"\n  - \"[ ] 🔳 ready\"\n  - \"[x] ✅ done\"\n---\n";

    fn files(list: &[(&str, &str)]) -> HashMap<String, String> {
        list.iter().map(|(r, t)| ((*r).to_owned(), (*t).to_owned())).collect()
    }

    fn saved() -> Vec<Block> {
        func::blocks(func::SAVED_DEFAULT)
    }

    #[test]
    fn indexes_wires_and_nodes() {
        let v = vault(&["a.md", "b.md", "c.md", "g_nodes.md"]);
        let graph = "## Nodes\n- [[a]]\n- [[b]]\n- fn Two\n\n## Wires\n- [[a]] → [[b]] : Wait For Done(task: \"Render\", target: \"Comp\")\n- [[b]] → [[c]] : Missing\n- [[a]] → Two.x\n- [[b]] → Two.y\n- Two.z → [[c]]\n- [[a]] → [[c]]\n\n## Functions\n```obsession\nfn Two\nin x: File\nin y: File\nout z: File\nEnsure Line(z, \"- [ ] Both\")\n```\n";
        let index = Index::build(&v, &[("g_nodes.md".into(), graph.into())], &saved());
        assert_eq!(index.jobs.len(), 2);
        assert_eq!(index.jobs[0].name(), "Wait For Done");
        assert_eq!(index.jobs[0].line, 6);
        assert_eq!(index.jobs[0].params[0], Some(Literal::Text("Render".into())));
        assert_eq!(index.jobs[1].ports.len(), 3);
        assert!(!index.jobs[1].on_wire);
        assert_eq!(index.affected(["a.md"]), [0, 1]);
        assert_eq!(index.affected(["c.md"]), Vec::<usize>::new());
        assert_eq!(index.files(), ["a.md", "b.md", "c.md"]);
        assert_eq!(index.find("g_nodes.md", 6), Some(0));
    }

    #[test]
    fn chains_and_loops() {
        let v = vault(&["a.md", "b.md", "c.md", "g_nodes.md"]);
        // a done → b ready; b's task ready → c gets a line; c → a would loop.
        let graph = "## Wires\n- [[a]] → [[b]] : Wait For Done(task: \"Light\", target: \"Comp\")\n- [[b]] → [[c]] : Note It\n- [[c]] → [[a]] : Flip\n\n## Functions\n```obsession\nfn Note It\nin src: File\nout dst: File\nt = Find Task(src, \"Comp\")\ns = Status(t)\nok = Equal(s, \" \")\nEnsure Line(dst, \"- [ ] Comp is ready\", when: ok)\n```\n```obsession\nfn Flip\nin src: File\nout dst: File\nSet Status(dst, \"-\", find: \"Light\")\n```\n";
        let index = Index::build(&v, &[("g_nodes.md".into(), graph.into())], &saved());
        assert_eq!(index.jobs.len(), 3);
        let a = format!("{FORMAT}- [x] Light\n");
        let b = format!("{FORMAT}- [-] Comp\n");
        let c = format!("{FORMAT}# C\n");
        let f = files(&[("a.md", &a), ("b.md", &b), ("c.md", &c)]);
        let out = run(&index, &f, &index.affected(["a.md"]), None);
        assert_eq!(out.ran, [0, 1, 2]);
        assert_eq!(out.texts["b.md"], format!("{FORMAT}- [ ] Comp\n"));
        assert_eq!(out.texts["c.md"], format!("{FORMAT}# C\n- [ ] Comp is ready\n"));
        // Flip set a's task back to [-]; that changes a, which Wait For Done
        // reads, so it runs again and would set b's line a second time.
        assert_eq!(out.texts["a.md"], format!("{FORMAT}- [-] Light\n"));
        assert_eq!(out.changes.len(), 3);
        assert!(out.warnings.iter().any(|w| w.starts_with("Loop: Wait For Done would change line 7 of b")), "{:?}", out.warnings);
        assert_eq!(out.summary(&index), "Wait For Done, Note It, Flip: 3 changes in 3 notes");

        // Applying the run's changes to the files it read gives its result.
        for rel in ["a.md", "b.md", "c.md"] {
            let mine: Vec<&Change> = out.changes.iter().filter(|c| c.rel == rel).collect();
            let (text, applied) = apply(&f[rel], &mine);
            assert_eq!(text, out.texts[rel]);
            assert_eq!(applied.len(), mine.len());
        }
        // Undo the whole run, newest first.
        for rel in ["a.md", "b.md", "c.md"] {
            let mine: Vec<&Change> = out.changes.iter().filter(|c| c.rel == rel).collect();
            let (back, skipped) = revert(&out.texts[rel], &mine);
            assert_eq!(back, f[rel], "{rel}");
            assert_eq!(skipped, 0);
        }
        // A line someone changed since is left alone.
        let mine: Vec<&Change> = out.changes.iter().filter(|c| c.rel == "b.md").collect();
        let edited = format!("{FORMAT}- [x] Comp\n");
        assert_eq!(revert(&edited, &mine), (edited.clone(), 1));
    }

    #[test]
    fn out_of_date_and_blocked() {
        let v = vault(&["a.md", "b.md", "g_nodes.md"]);
        let graph = "## Wires\n- [[a]] → [[b]] : Wait For Done(task: \"Light\", target: \"Comp\")\n- [[b]] → [[a]] : Wait For Done(task: \"Nope\", target: \"Light\")\n";
        let index = Index::build(&v, &[("g_nodes.md".into(), graph.into())], &saved());
        let a = format!("{FORMAT}- [x] Light\n");
        let b = format!("{FORMAT}- [-] Comp\n");
        let f = files(&[("a.md", &a), ("b.md", &b)]);
        let p = pending(&index, &f, None);
        assert_eq!(p, [(0, "Wait For Done: 1 change in b".to_owned())]);
        let out = run(&index, &f, &[1], None);
        assert_eq!(out.blocked.get(&1).map(String::as_str), Some("Find Task: no task matches \"Nope\""));
        assert!(out.changes.is_empty());
        // Once run, nothing is pending.
        let done = run(&index, &f, &[0], None);
        let f2 = files(&[("a.md", &a), ("b.md", &done.texts["b.md"])]);
        assert!(pending(&index, &f2, None).is_empty());
    }
}
