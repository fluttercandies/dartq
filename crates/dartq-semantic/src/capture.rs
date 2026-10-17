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
//! Shapes the model cannot represent (a headless argument group) poison
//! the enclosing tree; a poisoned tree suppresses its consumer's
//! conversion — the under-report direction, never a fabricated type.
//! Record types and function types are first-class: their events open
//! their own capture levels and the completed nodes fold into typed
//! columns.

use dartq_source::{SourceRange, StringId};

/// One written type: the head name, the `?` flag, the explicit type
/// arguments, and the source span (head token through the argument
/// group's closer — the official `_typeArgumentErrorNode` anchor).
///
/// A record type (`(int, {String s})`) carries its field list instead of a
/// head — [`TypeTree::record`] holds the positional entries in source
/// order plus the named `(field name, type)` pairs; `head`/`args` stay
/// empty on those nodes. A function type (`int Function(String)`)
/// carries [`TypeTree::function`] the same way — the written return type
/// plus the formal columns; `head`/`args` stay empty on those nodes.
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
    /// A record type's fields: positional entries then named pairs.
    /// `Some` marks the node a record (`head` is meaningless there).
    pub record: Option<(Vec<TypeTree>, Vec<(StringId, TypeTree)>)>,
    /// A function type's columns. `Some` marks the node a function
    /// (`head` is meaningless there).
    pub function: Option<FnTree>,
    /// Head token through the group closer (leaves: the head token).
    pub range: SourceRange,
    /// An unmodelable shape was seen inside: the tree can never map.
    pub poisoned: bool,
}

/// A function-type node's columns: the written return type (`None` for a
/// bare `Function(...)` head — the implicit `dynamic`), the
/// type-parameter group's count, and the formal list split exactly like a
/// record's fields (named formals are the trailing entries that carried a
/// named-formal event).
#[derive(Clone, Debug)]
pub(crate) struct FnTree {
    /// The written return type.
    pub ret: Option<Box<TypeTree>>,
    /// The `<T, U>` group's count.
    pub type_params: u16,
    /// Positional formal types in source order.
    pub positional: Vec<TypeTree>,
    /// Named formals `(name, type)`, written order.
    pub named: Vec<(StringId, TypeTree)>,
}

impl TypeTree {
    pub(crate) fn leaf(head: StringId, nullable: bool, range: SourceRange) -> Self {
        Self {
            head,
            nullable,
            args: Vec::new(),
            record: None,
            function: None,
            range,
            poisoned: false,
        }
    }

    /// A record-type node: the field entries are in, the trailing `?` and
    /// the whole-type span ride the same slots a named node uses.
    fn record_node(
        positional: Vec<TypeTree>,
        named: Vec<(StringId, TypeTree)>,
        nullable: bool,
        range: SourceRange,
        poisoned: bool,
    ) -> Self {
        Self {
            head: StringId::NONE,
            nullable,
            args: Vec::new(),
            record: Some((positional, named)),
            function: None,
            range,
            poisoned,
        }
    }

    /// A function-type node: the columns are in, the trailing `?` and the
    /// whole-type span ride the same slots a named node uses. The span's
    /// start extends over the written return type when one parsed (the
    /// node covers the whole annotation, not just the `Function` keyword).
    fn function_node(
        ret: Option<Box<TypeTree>>,
        type_params: u16,
        positional: Vec<TypeTree>,
        named: Vec<(StringId, TypeTree)>,
        nullable: bool,
        range: SourceRange,
        poisoned: bool,
    ) -> Self {
        let range = match &ret {
            Some(ret) if ret.range.offset < range.offset => SourceRange {
                offset: ret.range.offset,
                length: range.end() - ret.range.offset,
            },
            _ => range,
        };
        Self {
            head: StringId::NONE,
            nullable,
            args: Vec::new(),
            record: None,
            function: Some(FnTree {
                ret,
                type_params,
                positional,
                named,
            }),
            range,
            poisoned,
        }
    }
}

/// One open type level: an argument group (the head node is absent for
/// headless groups — expression type arguments never carry one), a record
/// type's field list, or a function type's columns.
pub(crate) struct Level {
    head: Option<TypeTree>,
    nodes: Vec<TypeTree>,
    /// Record levels and function levels only, parallel to `nodes`: the
    /// field/formal name captured at `RecordFieldDeclaration` (records) or
    /// `end_formal_parameter` (functions) (`None` = no written name).
    field_names: Vec<Option<StringId>>,
    /// Whether this level is a record type's field list.
    record: bool,
    /// Whether this level is a function type's columns.
    function: bool,
    /// The `{…}` named region's field/formal count, latched at
    /// `end_record_type_named_fields` (records) or per named formal
    /// (functions) — the named entries are always the trailing `count`
    /// nodes (`(int x, {int y})`: `x` is positional even though it carries
    /// a written name).
    named_count: usize,
    /// Function levels only: the node count when the formal list opened —
    /// everything before it is the written return type.
    formal_mark: usize,
    /// Function levels only: the type-variable group's count, latched
    /// while the group is open.
    type_params: u16,
    /// A type-variable group is open at this level (`<T extends …>`): the
    /// bound shapes are not type-identity columns — their nodes drop.
    in_type_variables: bool,
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
        // A type-variable bound's head is not a shape column (the
        // group's count is) — its node drops.
        if self
            .stack
            .last()
            .is_some_and(|level| level.in_type_variables)
        {
            return;
        }
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
            field_names: Vec::new(),
            record: false,
            function: false,
            named_count: 0,
            formal_mark: 0,
            type_params: 0,
            in_type_variables: false,
            poisoned: false,
        });
    }

    /// A record type opened (`begin_record_type`): the level collects one
    /// field node per `record_type_entry`, in source order. The record is
    /// its own root — a `(int, int)` annotation fires no argument-group
    /// events, so the capture opens here when none did
    /// (`builder.capture` stays `None` until then).
    pub(crate) fn begin_record(&mut self) {
        self.stack.push(Level {
            head: None,
            nodes: Vec::new(),
            field_names: Vec::new(),
            record: true,
            function: false,
            named_count: 0,
            formal_mark: 0,
            type_params: 0,
            in_type_variables: false,
            poisoned: false,
        });
    }

    /// A function type opened (`begin_function_type`, or the old-style
    /// function-typed formal's `begin_function_typed_formal_parameter` —
    /// the same shape in legacy spelling): the level collects the written
    /// return type, then the formal types. Like the record, the function
    /// is its own root — the capture opens here when none did.
    pub(crate) fn begin_function(&mut self) {
        self.stack.push(Level {
            head: None,
            nodes: Vec::new(),
            field_names: Vec::new(),
            record: false,
            function: true,
            named_count: 0,
            formal_mark: 0,
            type_params: 0,
            in_type_variables: false,
            poisoned: false,
        });
    }

    /// Whether the open level collects a function type's columns (the
    /// collector routes formal-list events by it).
    pub(crate) fn top_is_function(&self) -> bool {
        self.stack.last().is_some_and(|level| level.function)
    }

    /// The function level's formal list opened (`begin_formal_parameters`
    /// under a function level): everything before this is the written
    /// return type.
    pub(crate) fn function_formals_begin(&mut self) {
        if let Some(level) = self.stack.last_mut()
            && level.function
        {
            level.formal_mark = level.nodes.len();
        }
    }

    /// One formal closed (`end_formal_parameter` under a function level):
    /// the formal's type node landed just before, so the name pairs with
    /// the last one — the same trailing discipline as a record field. A
    /// formal whose type fired no node (recovery) carries no name.
    pub(crate) fn function_formal_end(&mut self, named: bool, name: Option<StringId>) {
        let Some(level) = self.stack.last_mut() else {
            return;
        };
        if !level.function || level.nodes.is_empty() {
            return;
        }
        if named {
            level.named_count += 1;
        }
        if level.field_names.len() < level.nodes.len() {
            level.field_names.resize(level.nodes.len(), None);
            if let Some(slot) = level.field_names.last_mut() {
                *slot = name;
            }
        }
    }

    /// A type-variable group opened at the open level (`begin_type_variables`).
    pub(crate) fn type_variables_begin(&mut self) {
        if let Some(level) = self.stack.last_mut() {
            level.in_type_variables = true;
        }
    }

    /// A type-variable group closed (`end_type_variables`).
    pub(crate) fn type_variables_end(&mut self) {
        if let Some(level) = self.stack.last_mut() {
            level.in_type_variables = false;
        }
    }

    /// One type variable declared in the open group
    /// (`begin_type_variable`) — a function level's own `<T, U>` group
    /// counts into its shape.
    pub(crate) fn function_type_variable(&mut self) {
        if let Some(level) = self.stack.last_mut()
            && level.function
            && level.in_type_variables
        {
            level.type_params = level.type_params.saturating_add(1);
        }
    }

    /// One record field's name (`handle_identifier` under
    /// `RecordFieldDeclaration`, or `handle_no_name` for the positional
    /// entries): the field's type completes before its name parses
    /// (`metadata type identifier?`), so the name pairs with the field's
    /// already-pushed node — the last one in `nodes` (pad-free: every
    /// entry emits exactly one type node and one name event in that
    /// order, so `field_names` trails `nodes` by at most one).
    pub(crate) fn record_field_name(&mut self, name: Option<StringId>) {
        if let Some(level) = self.stack.last_mut()
            && level.record
            && !level.nodes.is_empty()
        {
            level.field_names.resize(level.nodes.len(), None);
            if let Some(slot) = level.field_names.last_mut() {
                *slot = name;
            }
        }
    }

    /// A record field closed (`end_record_type_entry`): a recovered hole
    /// entry (`(int, ,)` — `compute_type` emits a synthetic head that
    /// resolves to no spelling, so no leaf landed) still holds a field
    /// slot upstream — push the hole node so the arity stays honest
    /// (`(int, ,)` binds `(int, InvalidType)`, matching the analyzer's
    /// InvalidType splice). `head` = `StringId::NONE` marks the hole; the
    /// conversion maps it to [`crate::model::Ty::Unknown`].
    pub(crate) fn record_entry_end(&mut self, mark: usize, range: SourceRange) {
        if let Some(level) = self.stack.last_mut()
            && level.record
            && level.nodes.len() == mark
        {
            level
                .nodes
                .push(TypeTree::leaf(StringId::NONE, false, range));
        }
    }

    /// The record level's node count — [`Self::record_entry_end`]'s mark.
    pub(crate) fn record_entry_mark(&self) -> usize {
        self.stack.last().map_or(0, |level| level.nodes.len())
    }

    /// The `{…}` region closed (`end_record_type_named_fields`): its
    /// `count` entries are the record's trailing nodes — the named set.
    pub(crate) fn record_named_region(&mut self, count: usize) {
        if let Some(level) = self.stack.last_mut()
            && level.record
        {
            level.named_count = count;
        }
    }

    /// The record type closed (`end_record_type`): the completed fields
    /// fold into a record node — the trailing `named_count` entries are
    /// the named pairs (each carrying its `RecordFieldDeclaration` name),
    /// every earlier entry a positional field; a positional field's own
    /// written name is decoration only (`(int x)` parses like `int` — the
    /// official `RecordTypeAnnotation` keeps the positional slot nameless
    /// for type identity).
    pub(crate) fn end_record(&mut self, nullable: bool, end: SourceRange) {
        let Some(level) = self.stack.pop() else {
            return;
        };
        debug_assert!(level.record, "end_record under a non-record level");
        let named_start = level.nodes.len().saturating_sub(level.named_count);
        let mut positional = Vec::with_capacity(named_start);
        let mut named = Vec::with_capacity(level.named_count);
        for (index, node) in level.nodes.into_iter().enumerate() {
            let name = level.field_names.get(index).copied().flatten();
            match name {
                Some(name) if index >= named_start => named.push((name, node)),
                _ => positional.push(node),
            }
        }
        let node = TypeTree::record_node(positional, named, nullable, end, level.poisoned);
        self.push_node(node);
    }

    /// The function type closed (`end_function_type`, or the old-style
    /// form's `end_function_typed_formal_parameter` — the same shape in
    /// legacy spelling): the captured nodes fold into a function node —
    /// everything before the formal list opened is the written return
    /// type (absent on a bare `Function(...)` head — the implicit
    /// `dynamic`; more than one is a recovery mangle and poisons the
    /// shape), the trailing `named_count` of the rest are the named
    /// formals, every earlier entry a positional formal.
    pub(crate) fn end_function(&mut self, nullable: bool, end: SourceRange) {
        let Some(level) = self.stack.pop() else {
            return;
        };
        debug_assert!(level.function, "end_function under a non-function level");
        let mut poisoned = level.poisoned;
        let mut nodes = level.nodes;
        let mut names = level.field_names;
        names.resize(nodes.len(), None);
        let mark = level.formal_mark.min(nodes.len());
        let ret: Option<Box<TypeTree>> = if mark > 1 {
            poisoned = true;
            None
        } else {
            let head: Vec<_> = nodes.drain(..mark).collect();
            names.drain(..mark.min(names.len()));
            head.into_iter().next().map(Box::new)
        };
        let named_start = nodes.len().saturating_sub(level.named_count);
        let mut positional = Vec::with_capacity(named_start);
        let mut named = Vec::with_capacity(level.named_count);
        for (index, node) in nodes.into_iter().enumerate() {
            match names.get(index).copied().flatten() {
                Some(name) if index >= named_start => named.push((name, node)),
                _ => positional.push(node),
            }
        }
        let node = TypeTree::function_node(
            ret,
            level.type_params,
            positional,
            named,
            nullable,
            end,
            poisoned,
        );
        self.push_node(node);
    }

    /// Feeds a completed node to the enclosing level (or `done`). A node
    /// completing inside a type-variable group is a bound's shape — not a
    /// type-identity column — and drops (the group's count is the only
    /// face the model carries).
    fn push_node(&mut self, node: TypeTree) {
        match self.stack.last_mut() {
            Some(parent) if !parent.in_type_variables => parent.nodes.push(node),
            Some(_) => {}
            None => self.done.push(node),
        }
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
        self.push_node(node);
    }

    /// An unmodelable shape inside the open trees (a headless group): the
    /// enclosing tree can never map, so its consumer's conversion
    /// suppresses — the under-report direction, never a fabricated type.
    /// Function types and record types are first-class shapes and never
    /// poison (their own events open their levels).
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
