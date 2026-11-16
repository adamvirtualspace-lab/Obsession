//! Value types (Syntax.md, "Types") and what each node argument accepts.

use super::{Literal, PortKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    File,
    Section,
    Task,
    Text,
    Number,
    Bool,
    TextList,
    TaskList,
}

impl Type {
    pub fn name(self) -> &'static str {
        match self {
            Type::File => "File",
            Type::Section => "Section",
            Type::Task => "Task",
            Type::Text => "Text",
            Type::Number => "Number",
            Type::Bool => "Bool",
            Type::TextList => "List of Text",
            Type::TaskList => "List of Task",
        }
    }

    /// What one item of a list is.
    pub fn element(self) -> Option<Type> {
        match self {
            Type::TextList => Some(Type::Text),
            Type::TaskList => Some(Type::Task),
            _ => None,
        }
    }

    /// Whether a value of this type can be read as text.
    pub fn is_textual(self) -> bool {
        matches!(self, Type::Text | Type::File | Type::Section | Type::Number)
    }
}

impl From<PortKind> for Type {
    fn from(kind: PortKind) -> Self {
        match kind {
            PortKind::File => Type::File,
            PortKind::Task => Type::Task,
            PortKind::Section => Type::Section,
        }
    }
}

impl Literal {
    pub fn ty(&self) -> Type {
        match self {
            Literal::Text(_) => Type::Text,
            Literal::Number(_) => Type::Number,
            Literal::Bool(_) => Type::Bool,
        }
    }
}

/// What an argument accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// Text, or anything that reads as text: a file's body, a section, a
    /// number.
    Text,
    Number,
    Bool,
    Task,
    File,
    TaskList,
    /// Any list.
    List,
    /// Text (searched for tasks) or a list of tasks.
    TextOrTasks,
    /// Anything (`Select`'s choices).
    Any,
    /// An action's target: an `out` (or a task found in one) of these
    /// types.
    Target(&'static [Type]),
}

impl Want {
    pub fn accepts(self, t: Type) -> bool {
        match self {
            Want::Text => t.is_textual(),
            Want::Number => t == Type::Number,
            Want::Bool => t == Type::Bool,
            Want::Task => t == Type::Task,
            Want::File => t == Type::File,
            Want::TaskList => t == Type::TaskList,
            Want::List => matches!(t, Type::TextList | Type::TaskList),
            Want::TextOrTasks => t.is_textual() || t == Type::TaskList,
            Want::Any => true,
            Want::Target(types) => types.contains(&t),
        }
    }

    pub fn describe(self) -> String {
        match self {
            Want::Text => "Text".into(),
            Want::Number => "Number".into(),
            Want::Bool => "Bool".into(),
            Want::Task => "Task".into(),
            Want::File => "File".into(),
            Want::TaskList => "List of Task".into(),
            Want::List => "List".into(),
            Want::TextOrTasks => "Text or List of Task".into(),
            Want::Any => "any value".into(),
            Want::Target(types) => {
                let names: Vec<&str> = types.iter().map(|t| t.name()).collect();
                format!("an out ({})", names.join(" or "))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_fits_where() {
        assert!(Want::Text.accepts(Type::File));
        assert!(Want::Text.accepts(Type::Section));
        assert!(!Want::Text.accepts(Type::Task));
        assert!(!Want::Text.accepts(Type::TextList));
        assert!(Want::TextOrTasks.accepts(Type::TaskList));
        assert!(!Want::TextOrTasks.accepts(Type::TextList));
        assert!(Want::List.accepts(Type::TextList));
        assert!(!Want::Bool.accepts(Type::Text));
        assert_eq!(Type::TaskList.element(), Some(Type::Task));
    }
}
