//! Type checking (Syntax.md, "Solid or dashed"): every node exists, every
//! required argument is filled, every type matches, there are no loops and
//! there's at least one action. A function that passes draws its wires
//! solid; one that doesn't draws them dashed with ⚠ and the first problem.

use std::collections::HashMap;

use super::library::{self, Output, Spec};
use super::types::{Type, Want};
use super::{Arg, Function, Literal, PortKind, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    /// The line in the file, when there is one.
    pub line: Option<usize>,
    /// The step it's about.
    pub step: Option<usize>,
    pub message: String,
}

/// What a name in the function refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    In(usize),
    Out(usize),
    Param(usize),
    Step(usize),
}

#[derive(Debug, Clone, Default)]
pub struct Checked {
    /// Each step's result type (`None` for actions and broken steps).
    pub types: Vec<Option<Type>>,
    /// Each step's arguments, by the node's input: the value given, or
    /// `None` for a default.
    pub bound: Vec<Vec<Option<Value>>>,
    /// Steps in an order where every step comes after what it reads.
    pub order: Vec<usize>,
    pub problems: Vec<Problem>,
    names: HashMap<String, Source>,
}

impl Checked {
    /// Solid: no problems at all.
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn first_problem(&self) -> Option<&str> {
        self.problems.first().map(|p| p.message.as_str())
    }

    pub fn source(&self, name: &str) -> Option<Source> {
        self.names.get(name).copied()
    }

    /// Problems about one step.
    pub fn step_problems(&self, step: usize) -> impl Iterator<Item = &Problem> {
        self.problems.iter().filter(move |p| p.step == Some(step))
    }
}

/// Binds a step's arguments to the node's inputs (positional first, then
/// by name).
pub fn bind(spec: &Spec, args: &[Arg]) -> (Vec<Option<Value>>, Vec<String>) {
    let mut bound: Vec<Option<Value>> = vec![None; spec.inputs.len()];
    let mut problems = Vec::new();
    let mut next = 0;
    for arg in args {
        let index = match &arg.name {
            None => {
                let i = next;
                next += 1;
                if i >= spec.inputs.len() {
                    problems.push(format!("{} takes only {}", spec.name, values(spec.inputs.len())));
                    continue;
                }
                i
            }
            Some(name) => match spec.input(name) {
                Some(i) => i,
                None => {
                    let names: Vec<&str> = spec.inputs.iter().map(|i| i.name).collect();
                    problems.push(format!("{} has no `{name}` (it has {})", spec.name, names.join(", ")));
                    continue;
                }
            },
        };
        if bound[index].is_some() {
            problems.push(format!("`{}` is given twice", spec.inputs[index].name));
            continue;
        }
        bound[index] = Some(arg.value.clone());
    }
    for (i, input) in spec.inputs.iter().enumerate() {
        if bound[i].is_none() && input.default == library::Default::Required {
            problems.push(format!("{} needs `{}`", spec.name, input.name));
        }
    }
    (bound, problems)
}

pub fn check(f: &Function) -> Checked {
    let mut c = Checked::default();
    fn problem(c: &mut Checked, line: Option<usize>, step: Option<usize>, message: String) {
        c.problems.push(Problem { line, step, message });
    }
    for (line, e) in &f.errors {
        problem(&mut c, Some(*line), None, e.clone());
    }

    // Names.
    fn declare(c: &mut Checked, name: &str, source: Source, line: usize, step: Option<usize>) {
        if c.names.contains_key(name) {
            problem(c, Some(line), step, format!("`{name}` is used twice"));
        } else {
            c.names.insert(name.to_owned(), source);
        }
    }
    for (i, p) in f.inputs.iter().enumerate() {
        declare(&mut c, &p.name, Source::In(i), p.line, None);
    }
    for (i, p) in f.outputs.iter().enumerate() {
        declare(&mut c, &p.name, Source::Out(i), p.line, None);
    }
    for (i, p) in f.params.iter().enumerate() {
        declare(&mut c, &p.name, Source::Param(i), p.line, None);
    }
    for (i, s) in f.steps.iter().enumerate() {
        if let Some(id) = &s.id {
            declare(&mut c, id, Source::Step(i), s.line, Some(i));
        }
    }

    // Each step's node and arguments.
    let specs: Vec<Option<&'static Spec>> = f.steps.iter().map(|s| library::find(&s.op)).collect();
    c.types = vec![None; f.steps.len()];
    c.bound = vec![Vec::new(); f.steps.len()];
    let mut broken = vec![false; f.steps.len()];
    let mut reads: Vec<Vec<usize>> = vec![Vec::new(); f.steps.len()];
    for (i, s) in f.steps.iter().enumerate() {
        let Some(spec) = specs[i] else {
            problem(&mut c, Some(s.line), Some(i), format!("There's no node called `{}`", s.op));
            broken[i] = true;
            continue;
        };
        if spec.is_action() && s.id.is_some() {
            problem(&mut c, Some(s.line), Some(i), format!("{} is an action: it has no result to name", spec.name));
        }
        let (bound, bind_problems) = bind(spec, &s.args);
        for p in bind_problems {
            problem(&mut c, Some(s.line), Some(i), p);
            broken[i] = true;
        }
        for v in bound.iter().flatten() {
            if let Value::Ref(name) = v {
                match c.names.get(name) {
                    Some(Source::Step(j)) => reads[i].push(*j),
                    Some(_) => {}
                    None => {
                        problem(&mut c, Some(s.line), Some(i), format!("Nothing is called `{name}`"));
                        broken[i] = true;
                    }
                }
            }
        }
        c.bound[i] = bound;
    }

    // Order (and loops).
    let n = f.steps.len();
    let mut state = vec![0u8; n]; // 0 new, 1 visiting, 2 done
    fn visit(i: usize, reads: &[Vec<usize>], state: &mut [u8], order: &mut Vec<usize>, looped: &mut Vec<usize>) {
        match state[i] {
            2 => return,
            1 => {
                looped.push(i);
                return;
            }
            _ => {}
        }
        state[i] = 1;
        for &j in &reads[i] {
            visit(j, reads, state, order, looped);
        }
        state[i] = 2;
        order.push(i);
    }
    let mut looped = Vec::new();
    for i in 0..n {
        visit(i, &reads, &mut state, &mut c.order, &mut looped);
    }
    looped.sort_unstable();
    looped.dedup();
    for &i in &looped {
        let name = f.steps[i].id.clone().unwrap_or_else(|| f.steps[i].op.clone());
        problem(&mut c, Some(f.steps[i].line), Some(i), format!("`{name}` feeds into itself (a loop)"));
        broken[i] = true;
    }

    // Types, in order. `from_out`: a value found in an `out` (so a task in
    // it can be changed).
    let mut from_out = vec![false; n];
    let order = c.order.clone();
    for &i in &order {
        let Some(spec) = specs[i] else { continue };
        let s = &f.steps[i];
        let mut arg_types: Vec<Option<Type>> = Vec::with_capacity(spec.inputs.len());
        let mut ok = !broken[i];
        for (k, input) in spec.inputs.iter().enumerate() {
            let value = c.bound[i].get(k).cloned().flatten();
            let (ty, origin_out) = match &value {
                None => (input.default.literal().map(|l| l.ty()), false),
                Some(Value::Lit(l)) => (Some(l.ty()), false),
                Some(Value::Ref(name)) => match c.names.get(name) {
                    Some(Source::In(p)) => (Some(Type::from(f.inputs[*p].kind)), false),
                    Some(Source::Out(p)) => (Some(Type::from(f.outputs[*p].kind)), true),
                    Some(Source::Param(p)) => (Some(f.params[*p].default.ty()), false),
                    Some(Source::Step(j)) => (c.types[*j], from_out[*j]),
                    None => (None, false),
                },
            };
            from_out[i] |= origin_out;
            arg_types.push(ty);
            let Some(ty) = ty else {
                ok = false;
                continue;
            };
            if !input.want.accepts(ty) {
                problem(
                    &mut c,
                    Some(s.line),
                    Some(i),
                    format!("{}'s `{}` wants {}, but gets {}", spec.name, input.name, input.want.describe(), ty.name()),
                );
                ok = false;
                continue;
            }
            if let Want::Target(_) = input.want {
                let writable = match &value {
                    Some(Value::Ref(name)) => match c.names.get(name) {
                        Some(Source::Out(_)) => true,
                        Some(Source::Step(j)) => from_out[*j] && ty == Type::Task,
                        _ => false,
                    },
                    _ => false,
                };
                if !writable {
                    let what = match &value {
                        Some(Value::Ref(name)) => match c.names.get(name) {
                            Some(Source::In(_)) => format!("`{name}` is an in; actions only change an out"),
                            _ => format!("`{name}` isn't an out (or a task found in one)"),
                        },
                        _ => "the target must be an out".to_owned(),
                    };
                    problem(&mut c, Some(s.line), Some(i), format!("{}: {what}", spec.name));
                    ok = false;
                }
            }
        }
        if !ok {
            continue;
        }
        c.types[i] = match spec.output {
            Output::Action => None,
            Output::Fixed(t) => Some(t),
            Output::SameAs(k) => {
                let t = arg_types[k];
                if spec.name == "Select"
                    && let (Some(a), Some(b)) = (arg_types[1], arg_types[2])
                    && !compatible(a, b)
                {
                    problem(
                        &mut c,
                        Some(s.line),
                        Some(i),
                        format!("Select's two choices must be the same type ({} and {})", a.name(), b.name()),
                    );
                    continue;
                }
                // A file or section picked by Select is still read as text.
                t.map(|t| if t.is_textual() && arg_types[2].is_some_and(|b| b != t) { Type::Text } else { t })
            }
            Output::ElementOf(k) => arg_types[k].and_then(Type::element),
        };
    }

    if !f.steps.iter().any(|s| library::find(&s.op).is_some_and(Spec::is_action)) {
        problem(&mut c, None, None, "No action yet: add Set Status, Replace Text, Ensure Line or Write Section".into());
    }
    c
}

fn values(n: usize) -> String {
    if n == 1 { "1 value".into() } else { format!("{n} values") }
}

fn compatible(a: Type, b: Type) -> bool {
    a == b || (a.is_textual() && b.is_textual())
}

/// Checks the values a wire gives a function's params
/// (`Wait For Done(task: "x")`). Returns the problems.
pub fn check_call(f: &Function, args: &[Arg]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = vec![false; f.params.len()];
    for (k, arg) in args.iter().enumerate() {
        let index = match &arg.name {
            Some(name) => f.params.iter().position(|p| p.name.eq_ignore_ascii_case(name)),
            None => (k < f.params.len()).then_some(k),
        };
        let Some(index) = index else {
            match &arg.name {
                Some(name) => problems.push(format!("{} has no param `{name}`", f.name)),
                None => problems.push(format!("{} takes only {}", f.name, values(f.params.len()))),
            }
            continue;
        };
        if std::mem::replace(&mut seen[index], true) {
            problems.push(format!("`{}` is given twice", f.params[index].name));
        }
        let param = &f.params[index];
        match &arg.value {
            Value::Lit(l) => {
                let fits = l.ty() == param.default.ty()
                    || (param.default.ty() == Type::Text && matches!(l, Literal::Number(_)));
                if !fits {
                    problems.push(format!("`{}` wants {}, but gets {}", param.name, param.default.ty().name(), l.ty().name()));
                }
            }
            Value::Ref(r) => problems.push(format!("`{}` needs a value (\"text\", a number or true/false), not `{r}`", param.name)),
        }
    }
    problems
}

/// What kind of wire end a port takes, for messages.
pub fn port_end(kind: PortKind) -> &'static str {
    match kind {
        PortKind::File => "a note ([[note]])",
        PortKind::Task => "a task ([[note#^id]])",
        PortKind::Section => "a heading ([[note#Heading]])",
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse;
    use super::*;

    const WAIT: &str = "fn Wait For Done\nin src: File\nout dst: File\nparam task = \"\"\nparam target = \"\"\nparam done = \"x\"\n\nt  = Find Task(src, task)\ns  = Status(t)\nok = Equal(s, done)\nst = Select(ok, \" \", \"-\")\nSet Status(dst, st, find: target, from: \"- \")\n";

    #[test]
    fn a_good_function_is_solid() {
        let f = parse(WAIT);
        let c = check(&f);
        assert!(c.ok(), "{:?}", c.problems);
        assert_eq!(c.types[0], Some(Type::Task));
        assert_eq!(c.types[1], Some(Type::Text));
        assert_eq!(c.types[2], Some(Type::Bool));
        assert_eq!(c.types[3], Some(Type::Text));
        assert_eq!(c.types[4], None);
        assert_eq!(c.order.last(), Some(&4));

        let lighting = parse(
            "fn Check Lighting\nin anim: File\nin fx: File\nout light: File\n\nat = Tasks(anim)\nad = All Status(at, \"x\")\nfl = Lines(fx)\nfa = Any Contains(fl, \"approved\", ignore case: true)\nok = And(ad, fa)\nst = Select(ok, \" \", \"-\")\nSet Status(light, st, find: \"Render lighting pass\", from: \"- \")\n",
        );
        assert!(check(&lighting).ok(), "{:?}", check(&lighting).problems);
    }

    fn problems(text: &str) -> Vec<String> {
        check(&parse(text)).problems.into_iter().map(|p| p.message).collect()
    }

    #[test]
    fn what_makes_it_dashed() {
        assert_eq!(problems("fn A\nin a: File\nx = Len(a)\n"), ["No action yet: add Set Status, Replace Text, Ensure Line or Write Section"]);
        let p = problems("fn A\nin a: File\nout b: File\nx = Nope(a)\ny = Len(a, a)\nz = Status(a)\nw = Left(a)\nSet Status(b, q)\n");
        assert!(p.contains(&"There's no node called `Nope`".to_owned()), "{p:?}");
        assert!(p.contains(&"Len takes only 1 value".to_owned()), "{p:?}");
        assert!(p.contains(&"Status's `task` wants Task, but gets File".to_owned()), "{p:?}");
        assert!(p.contains(&"Left needs `count`".to_owned()), "{p:?}");
        assert!(p.contains(&"Nothing is called `q`".to_owned()), "{p:?}");

        let p = problems("fn A\nin a: File\nout b: File\nSet Status(a, \"x\")\n");
        assert_eq!(p, ["Set Status: `a` is an in; actions only change an out"]);
        let p = problems("fn A\nin a: File\nout b: File\nx = Not(y)\ny = Not(x)\nSet Status(b, \"x\", when: x)\n");
        assert!(p.iter().any(|m| m.contains("loop")), "{p:?}");
        let p = problems("fn A\nin a: File\nout b: File\nx = Select(true, a, 3)\ny = Select(true, a, true)\nSet Status(b, x)\n");
        assert_eq!(p, ["Select's two choices must be the same type (File and Bool)"]);
        let p = problems("fn A\nin a: File\nout a: File\nx = Set Status(a, \"x\")\n");
        assert!(p.contains(&"`a` is used twice".to_owned()), "{p:?}");
        assert!(p.contains(&"Set Status is an action: it has no result to name".to_owned()), "{p:?}");
    }

    #[test]
    fn tasks_found_in_an_out_can_be_changed() {
        let ok = problems("fn A\nin a: File\nout b: File\nt = Find Task(b, \"Comp\")\nSet Status(t, \"x\")\n");
        assert!(ok.is_empty(), "{ok:?}");
        let bad = problems("fn A\nin a: File\nout b: File\nt = Find Task(a, \"Comp\")\nSet Status(t, \"x\")\n");
        assert_eq!(bad, ["Set Status: `t` isn't an out (or a task found in one)"]);
    }

    #[test]
    fn wire_values() {
        let f = parse(WAIT);
        let (_, args) = super::super::parse_call("Wait For Done(task: \"Render\", target: \"Comp\")").unwrap();
        assert!(check_call(&f, &args).is_empty());
        let (_, args) = super::super::parse_call("Wait For Done(\"a\", nope: \"b\", done: true, task: \"c\")").unwrap();
        assert_eq!(
            check_call(&f, &args),
            ["Wait For Done has no param `nope`", "`done` wants Text, but gets Bool", "`task` is given twice"]
        );
    }
}
