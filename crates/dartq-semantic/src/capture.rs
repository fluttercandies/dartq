//! The shared written-type capture: one parse-event sink shape for every
//! consumer that needs the type arguments as written (#610 capture, phase-2
//! feeding #728).
//!
//! [`TypeTree`] records one written type — head name, `?` flag, explicit
//! type arguments, span — and [`TreeSink`] builds trees from the parser's
//! type events (`handle_type`, `begin/end_type_arguments`, the unmodelable
//! shapes' poison). Two passes observe the same event stream per file:
//!
//! * the diagnoser's [`crate::bounds::Bounds`] windows (a `<...>`
//!   type-variables group, a container header, an outermost annotation
//!   site) route their events into a sink for the
//!   `type_argument_not_matching_bounds` face;
//! * the collector ([`crate::collector`]) instantiates its own sink for
//!   declaration annotations and interns the completed trees bottom-up
//!   into the unit's [`crate::model::ArgPool`], so the model's
//!   [`crate::model::Ty::Named`] slots carry the declared arguments.
//!
//! Shapes the model cannot represent (function types, record types, a
//! headless group) poison the enclosing tree; a poisoned tree suppresses
//! its consumer's conversion — the under-report direction, never a
//! fabricated type.

use dartq_source::{SourceRange, StringId};

/// One written type: the head name, the `?` flag, the explicit type
/// arguments, and the source span (head token through the argument
/// group's closer — the official `_typeArgumentErrorNode` anchor).
#[derive(Clone, Debug)]
pub(crate) struct TypeTree {
    /// The head identifier as written (a prefixed `p.C` keeps the head
    /// token the parser completes the type with, the model's bare-name
    /// convention).
    pub head: StringId,
    /// The trailing `?` was present.
    pub nullable: bool,
    /// Explicit type arguments, in source order.
    pub args: Vec<TypeTree>,
    /// Head token through the group closer (leaves: the head token).
    pub range: SourceRange,
    /// An unmodelable shape was seen inside: the tree can never map.
    pub poisoned: bool,
}

impl TypeTree {
    pub(crate) fn leaf(head: StringId, nullable: bool, range: SourceRange) -> Self {
        Self {
            head,
            nullable,
            args: Vec::new(),
            range,
            poisoned: false,
        }
    }
}

/// One open argument group: the head node (absent for headless groups —
/// expression type arguments never carry a head) and the nodes completed
/// at its level so far.
pub(crate) struct Level {
    head: Option<TypeTree>,
    nodes: Vec<TypeTree>,
    poisoned: bool,
}

/// Captures written type trees from the parse events while a capture
/// window is open. Completed roots accumulate in `done`; the consumer
/// claims them per its own window discipline (per type variable, per
/// clause, per annotation site).
#[derive(Default)]
pub(crate) struct TreeSink {
    pub(crate) done: Vec<TypeTree>,
    pub(crate) stack: Vec<Level>,
}

impl TreeSink {
    /// A simple type head completed (`handle_type`).
    ///
    /// The parser re-fires a parameterized head after its own group
    /// (`List<int>?` — the trailing `?` travels on that `handle_type`),
    /// so a leaf whose head token sits exactly on the level's last group
    /// node is that node's completion, never a sibling: it merges the
    /// nullability and any `?` extension into the node. Without the merge
    /// the re-fired head would double-count as an extra argument and
    /// poison the capture.
    pub(crate) fn leaf(&mut self, head: StringId, nullable: bool, range: SourceRange) {
        let siblings = match self.stack.last_mut() {
            Some(level) => &mut level.nodes,
            None => &mut self.done,
        };
        if let Some(last) = siblings.last_mut()
            && last.head == head
            && last.range.offset == range.offset
            && !last.args.is_empty()
        {
            last.nullable |= nullable;
            if range.end() > last.range.end() {
                last.range.length = range.end() - last.range.offset;
            }
            return;
        }
        siblings.push(TypeTree::leaf(head, nullable, range));
    }

    /// A type-argument group opened (`begin_type_arguments`); `head` is
    /// the parameterized type the group applies to.
    pub(crate) fn begin_group(&mut self, head: Option<(StringId, SourceRange)>) {
        let head = head.map(|(name, range)| TypeTree::leaf(name, false, range));
        self.stack.push(Level {
            head,
            nodes: Vec::new(),
            poisoned: false,
        });
    }

    /// The group closed (`end_type_arguments`); the completed head node
    /// joins its parent level (or `done`).
    pub(crate) fn end_group(&mut self, end: SourceRange) {
        let Some(level) = self.stack.pop() else {
            return;
        };
        let Some(mut node) = level.head else {
            // A headless group inside a tree is a shape the consumers do
            // not model; poison the parent instead of guessing.
            if let Some(parent) = self.stack.last_mut() {
                parent.poisoned = true;
            }
            return;
        };
        if end.end() > node.range.end() {
            node.range = SourceRange {
                offset: node.range.offset,
                length: end.end() - node.range.offset,
            };
        }
        node.poisoned |= level.poisoned;
        node.args = level.nodes;
        match self.stack.last_mut() {
            Some(parent) => parent.nodes.push(node),
            None => self.done.push(node),
        }
    }

    /// An unmodelable shape inside the open trees (`begin_function_type`,
    /// `begin_record_type`, ...).
    pub(crate) fn poison(&mut self) {
        for level in &mut self.stack {
            level.poisoned = true;
        }
    }

    pub(crate) fn take_done(&mut self) -> Vec<TypeTree> {
        std::mem::take(&mut self.done)
    }

    pub(crate) fn unclaimed(&self, claimed: usize) -> &[TypeTree] {
        self.done.get(claimed..).unwrap_or(&[])
    }
}
