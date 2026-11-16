//! Every node in the node library (Syntax.md, "Node library").

use super::Literal;
use super::types::{Type, Want};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Text,
    Lists,
    Logic,
    Markdown,
    Actions,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Text,
        Category::Lists,
        Category::Logic,
        Category::Markdown,
        Category::Actions,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Text => "Text",
            Category::Lists => "Lists",
            Category::Logic => "Logic",
            Category::Markdown => "Markdown and tasks",
            Category::Actions => "Actions",
        }
    }
}

/// An argument's default when it's left out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Default {
    Required,
    Text(&'static str),
    Bool(bool),
}

impl Default {
    pub fn literal(self) -> Option<Literal> {
        match self {
            Default::Required => None,
            Default::Text(t) => Some(Literal::Text(t.to_owned())),
            Default::Bool(b) => Some(Literal::Bool(b)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Input {
    pub name: &'static str,
    pub want: Want,
    pub default: Default,
}

/// What a node gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// An action: nothing; it changes its target.
    Action,
    Fixed(Type),
    /// The type of this argument (`Select`, `Filter Contains`).
    SameAs(usize),
    /// One item of the list in this argument (`Get`, `First`).
    ElementOf(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub name: &'static str,
    pub category: Category,
    pub inputs: &'static [Input],
    pub output: Output,
    pub help: &'static str,
}

impl Spec {
    pub fn is_action(&self) -> bool {
        self.output == Output::Action
    }

    /// The input called `name` (case-insensitive, spaces collapsed).
    pub fn input(&self, name: &str) -> Option<usize> {
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        self.inputs.iter().position(|i| i.name.eq_ignore_ascii_case(&name))
    }
}

const fn req(name: &'static str, want: Want) -> Input {
    Input {
        name,
        want,
        default: Default::Required,
    }
}

const fn text(name: &'static str, default: &'static str) -> Input {
    Input {
        name,
        want: Want::Text,
        default: Default::Text(default),
    }
}

const fn flag(name: &'static str, default: bool) -> Input {
    Input {
        name,
        want: Want::Bool,
        default: Default::Bool(default),
    }
}

const IGNORE_CASE: Input = flag("ignore case", false);
const WHEN: Input = flag("when", true);
const T: Want = Want::Text;

use Category::*;
use Output::*;

const fn node(name: &'static str, category: Category, inputs: &'static [Input], output: Output, help: &'static str) -> Spec {
    Spec {
        name,
        category,
        inputs,
        output,
        help,
    }
}

pub static NODES: &[Spec] = &[
    // Text
    node("Contains", Text, &[req("text", T), req("find", T), IGNORE_CASE], Fixed(Type::Bool), "Whether the text contains `find`"),
    node("Starts With", Text, &[req("text", T), req("find", T), IGNORE_CASE], Fixed(Type::Bool), "Whether the text starts with `find`"),
    node("Ends With", Text, &[req("text", T), req("find", T), IGNORE_CASE], Fixed(Type::Bool), "Whether the text ends with `find`"),
    node("Equal", Text, &[req("a", T), req("b", T), IGNORE_CASE], Fixed(Type::Bool), "Whether two texts are the same"),
    node("Is Empty", Text, &[req("text", T)], Fixed(Type::Bool), "Whether the text is empty (spaces count as empty)"),
    node("Len", Text, &[req("text", T)], Fixed(Type::Number), "How many characters"),
    node("Left", Text, &[req("text", T), req("count", Want::Number)], Fixed(Type::Text), "The first `count` characters"),
    node("Right", Text, &[req("text", T), req("count", Want::Number)], Fixed(Type::Text), "The last `count` characters"),
    node("Mid", Text, &[req("text", T), req("start", Want::Number), req("count", Want::Number)], Fixed(Type::Text), "`count` characters from `start` (counting from 0)"),
    node("Find", Text, &[req("text", T), req("find", T), IGNORE_CASE], Fixed(Type::Number), "Where `find` first appears, or -1"),
    node("Replace", Text, &[req("text", T), req("from", T), req("to", T), IGNORE_CASE], Fixed(Type::Text), "The text with every `from` replaced by `to`"),
    node("Trim", Text, &[req("text", T)], Fixed(Type::Text), "Without spaces at either end"),
    node("Lower", Text, &[req("text", T)], Fixed(Type::Text), "In lower case"),
    node("Upper", Text, &[req("text", T)], Fixed(Type::Text), "In upper case"),
    node("Matches Wildcard", Text, &[req("text", T), req("pattern", T)], Fixed(Type::Bool), "Whether the text matches a pattern with `*` and `?`"),
    node("Join Text", Text, &[req("a", T), req("b", T), text("separator", "")], Fixed(Type::Text), "Two texts joined"),
    node("Split", Text, &[req("text", T), req("by", T)], Fixed(Type::TextList), "The text cut at every `by`"),
    node("Lines", Text, &[req("text", T)], Fixed(Type::TextList), "The text's lines"),
    // Lists
    node("Count", Lists, &[req("list", Want::List)], Fixed(Type::Number), "How many items"),
    node("Get", Lists, &[req("list", Want::List), req("index", Want::Number)], ElementOf(0), "The item at `index` (counting from 0)"),
    node("First", Lists, &[req("list", Want::List)], ElementOf(0), "The first item"),
    node("Last", Lists, &[req("list", Want::List)], ElementOf(0), "The last item"),
    node("Filter Contains", Lists, &[req("list", Want::List), req("find", T), IGNORE_CASE], SameAs(0), "The items whose text contains `find`"),
    node("Any Contains", Lists, &[req("list", Want::List), req("find", T), IGNORE_CASE], Fixed(Type::Bool), "Whether any item's text contains `find`"),
    node("All Contains", Lists, &[req("list", Want::List), req("find", T), IGNORE_CASE], Fixed(Type::Bool), "Whether every item's text contains `find`"),
    node("Join", Lists, &[req("list", Want::List), req("separator", T)], Fixed(Type::Text), "The items' text joined"),
    // Logic
    node("Not", Logic, &[req("bool", Want::Bool)], Fixed(Type::Bool), "true when false, false when true"),
    node("And", Logic, &[req("a", Want::Bool), req("b", Want::Bool)], Fixed(Type::Bool), "Whether both are true"),
    node("Or", Logic, &[req("a", Want::Bool), req("b", Want::Bool)], Fixed(Type::Bool), "Whether either is true"),
    node("Select", Logic, &[req("bool", Want::Bool), req("if true", Want::Any), req("if false", Want::Any)], SameAs(1), "One of two values"),
    node("Compare", Logic, &[req("a", T), req("op", T), req("b", T)], Fixed(Type::Bool), "`op` is \"=\", \"<\", \">\", \"<=\" or \">=\"; numbers compare as numbers"),
    // Markdown and tasks
    node("Tasks", Markdown, &[req("text", T)], Fixed(Type::TaskList), "Every task in the text"),
    node("Find Task", Markdown, &[req("text or list", Want::TextOrTasks), req("find", T)], Fixed(Type::Task), "The first task whose text contains `find` (or whose ID is `find`, if it starts with ^)"),
    node("Status", Markdown, &[req("task", Want::Task)], Fixed(Type::Text), "The task's status character"),
    node("Task Text", Markdown, &[req("task", Want::Task)], Fixed(Type::Text), "The task's text, without status, people or ID"),
    node("People", Markdown, &[req("task", Want::Task)], Fixed(Type::TextList), "The @people on the task"),
    node("Count Status", Markdown, &[req("tasks", Want::TaskList), req("status", T)], Fixed(Type::Number), "How many tasks have this status"),
    node("All Status", Markdown, &[req("tasks", Want::TaskList), req("status", T)], Fixed(Type::Bool), "Whether every task has this status"),
    node("Section", Markdown, &[req("text", T), req("heading", T)], Fixed(Type::Text), "The text under a heading"),
    node("Frontmatter", Markdown, &[req("file", Want::File), req("key", T)], Fixed(Type::Text), "A frontmatter property's value"),
    // Actions
    node("Set Status", Actions, &[req("target", Want::Target(&[Type::File, Type::Task])), req("status", T), text("find", ""), text("from", ""), WHEN], Action, "Sets a task's status (in a File target: the first task whose text contains `find`). With `from`, only changes a task whose status is one of those characters"),
    node("Replace Text", Actions, &[req("target", Want::Target(&[Type::File, Type::Section])), req("from", T), req("to", T), WHEN], Action, "Replaces text in a file or section"),
    node("Ensure Line", Actions, &[req("target", Want::Target(&[Type::File, Type::Section])), req("line", T), WHEN], Action, "Adds a line at the end, unless it's already there"),
    node("Write Section", Actions, &[req("target", Want::Target(&[Type::Section])), req("text", T), WHEN], Action, "Replaces everything under a heading"),
];

/// The node called `name` (case-insensitive).
pub fn find(name: &str) -> Option<&'static Spec> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    NODES.iter().find(|s| s.name.eq_ignore_ascii_case(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library() {
        assert_eq!(find("find  task").unwrap().name, "Find Task");
        assert!(find("Set Status").unwrap().is_action());
        assert_eq!(find("Select").unwrap().input("If True"), Some(1));
        // Names are unique.
        for (i, a) in NODES.iter().enumerate() {
            assert!(NODES[i + 1..].iter().all(|b| !b.name.eq_ignore_ascii_case(a.name)), "{}", a.name);
        }
    }
}
