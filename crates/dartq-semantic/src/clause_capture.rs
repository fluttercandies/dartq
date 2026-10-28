//! The written supertype-clause capture behind
//! [`crate::resolved_ops::ClauseTrees`] (#2229): one parse of a gated unit
//! rebuilds the argument-bearing supertype faces the element model strips
//! to names, so `LibraryTypes::build_with_clauses` can edge
//! `class A extends S<int>` as `A → S<int>` instead of the to-bounds
//! `A → S<dynamic>`.
//!
//! The listener routes exactly the parser events the diagnosing fold's
//! boundedness capture feeds (`crate::bounds`), so a capture here is
//! faithful to that shape: type heads and argument groups complete into a
//! [`TreeSink`] while a container header is open, clause events claim the
//! completed roots, and a body begin finalizes. A capture whose clause set
//! did not parse faithfully — a clause count mismatch, an unclaimed root
//! at body time, a poisoned tree, an extension type's header — is `open`
//! and contributes no entry: the consumer keeps the name-level edges, the
//! under-report direction. Named mixin applications never finalize their
//! header (no body-begin event, the diagnosing capture's own shape) and
//! contribute nothing either.
//!
//! Unlike the `crate::bounds` index — which records only containers with
//! a type-variable window, the boundedness checks' only subjects — the
//! ledger here records every named container: a non-generic
//! `class IntHolder extends Holder<int>` is exactly the shape the clause
//! trees exist for.

use rustc_hash::FxHashMap;

use crate::bounds::TypeTree;
use crate::capture::TreeSink;
use crate::resolved_ops::{ClauseCapture, ClauseTrees};
use dartq_parser::{
    DeclarationKind, IdentifierContext, Parser, ParserFeatures, ParserListener, TokenSpan as Token,
};
use dartq_scanner::TokenTable;
use dartq_source::{FileId, SourceRange, StringId, StringTable};

/// One open container header's supertype capture: the completed trees plus
/// the claim cursor the clause events advance. The mirror of
/// `crate::bounds::HeaderSink` scoped to what the clause trees consume —
/// the claims only drive the faithfulness bit, the final supertype set is
/// every completed root.
struct HeaderCapture {
    sink: TreeSink,
    claimed: usize,
    /// A clause's count disagreed with its captured roots.
    open: bool,
}

impl HeaderCapture {
    /// Claims the roots since the cursor for the completing clause; a
    /// count mismatch marks the capture unfaithful.
    fn claim(&mut self, expected: Option<usize>) {
        let since = self.sink.unclaimed(self.claimed).len();
        if let Some(count) = expected
            && since != count
        {
            self.open = true;
        }
        self.claimed = self.sink.done.len();
    }

    /// Body time: a root no clause claimed, or an unclosed group, means
    /// the capture is not the faithful supertype set.
    fn finish(mut self) -> (Vec<TypeTree>, bool) {
        let open = self.open
            || !self.sink.unclaimed(self.claimed).is_empty()
            || !self.sink.stack.is_empty();
        (std::mem::take(&mut self.sink.done), open)
    }
}

/// One container's ledger row: the type-parameter spellings adopted at the
/// begin event, each parameter's written bound tree in the same order, the
/// supertype trees written at the body, and the faithfulness bit.
struct Row {
    params: Vec<StringId>,
    /// Each parameter's bound tree, parallel to `params` — `None` for an
    /// unbounded parameter (the implicit `Object?`), `Some` for a written
    /// `extends` bound. Captured while the declaring `<...>` window was
    /// open (the parser completes the variables in reverse order, each
    /// bound's roots closing just before its own `end_type_variable`).
    param_bounds: Vec<Option<TypeTree>>,
    supers: Vec<TypeTree>,
    open: bool,
}

/// The per-unit capture listener. See the [module docs](self) for the
/// event contract.
struct CaptureListener<'src, 'tab> {
    source: &'src str,
    tokens: &'tab TokenTable,
    strings: &'src StringTable,
    /// Every named container of the unit, keep-first on a shared name (the
    /// duplicate's second header is the `duplicate_definition` face's
    /// business, same as the element model).
    containers: FxHashMap<StringId, Row>,
    /// The open container header: its name and its clause cursor.
    header: Option<(StringId, HeaderCapture)>,
    /// The open `<...>` type-variables group's bound capture: completed
    /// bound roots accumulate in `bound_sink`, each variable's completion
    /// claims its own.
    bound_sink: Option<TreeSink>,
    /// The open group's variable names in declaration order
    /// (`begin_type_variable` fires in written order); `end_type_variable`
    /// pops the last — the parser completes the variables in reverse
    /// declaration order, re-parsing each bound anchored at its own
    /// `extends` token, so the completing variable is always the
    /// last-pushed name.
    var_names: Vec<StringId>,
    /// The open group's variables in completion order (reverse
    /// declaration order): the name and the bound tree claimed at its
    /// `end_type_variable`.
    window: Vec<(StringId, Option<TypeTree>)>,
    /// The just-closed window's names, bounds, and close offset, awaiting
    /// the container begin event that adopts them.
    pending: Option<(Vec<StringId>, Vec<Option<TypeTree>>, u32)>,
    /// The type head the parser arms at a type reference and re-fires
    /// after its argument group (`Box` of `Box<int>`) — the group's head.
    pending_head: Option<(StringId, SourceRange)>,
}

impl<'src, 'tab> CaptureListener<'src, 'tab> {
    fn name_of(&self, token: Token) -> Option<StringId> {
        self.tokens
            .get(token.id as usize)
            .map(|t| self.strings.intern(&self.source[t.byte_range()]))
    }

    /// Opens the container's header window and records its ledger row: the
    /// parked type-variable window is this container's when the name
    /// precedes the group's close (the token-order guard — a foreign
    /// window never adopts).
    fn note_container(&mut self, name: Option<Token>, is_extension_type: bool) {
        let Some(token) = name else {
            return;
        };
        let Some(id) = self.name_of(token) else {
            return;
        };
        self.header = Some((
            id,
            HeaderCapture {
                sink: TreeSink::default(),
                claimed: 0,
                open: false,
            },
        ));
        // The just-closed type-variable window is this container's when its
        // declaring name precedes the group's close — the same token-order
        // guard `crate::bounds` applies. A foreign window (a preceding
        // generic function's, a typedef's) never adopts.
        let (params, param_bounds) = match self.pending.take() {
            Some((names, bounds, window_end)) if token.range.offset < window_end => {
                (names, bounds)
            }
            _ => (Vec::new(), Vec::new()),
        };
        self.containers.entry(id).or_insert(Row {
            params,
            param_bounds,
            supers: Vec::new(),
            // An extension type's supertype face is not its class-style
            // edge set: the capture stays unfaithful, the snapshot never
            // walks through it.
            open: is_extension_type,
        });
    }

    /// The body opened: the header capture finalizes onto the container.
    fn finish_header(&mut self) {
        let Some((name, header)) = self.header.take() else {
            return;
        };
        let (supers, open) = header.finish();
        let Some(row) = self.containers.get_mut(&name) else {
            return;
        };
        row.supers = supers;
        row.open |= open;
    }
}

impl ParserListener for CaptureListener<'_, '_> {
    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        if matches!(
            context,
            IdentifierContext::TypeReference | IdentifierContext::TypeReferenceContinuation
        ) && let Some(name) = self.name_of(token)
        {
            self.pending_head = Some((name, token.range));
        }
    }

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        self.pending_head = None;
        let Some(head) = self.name_of(name) else {
            return;
        };
        let range = match question {
            Some(q) => SourceRange {
                offset: name.range.offset,
                length: q.range.end() - name.range.offset,
            },
            None => name.range,
        };
        // The routing mirrors `crate::bounds`: an open container header
        // wins (a nested function type's own `<T>` group inside a clause
        // completes its bound leaves into that function's own level —
        // they fold into the one function node); otherwise the
        // type-variables window's bound sink takes them (a container's own
        // `<T extends num>` parses before its header opens).
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.leaf(head, question.is_some(), range);
        } else if let Some(sink) = self.bound_sink.as_mut() {
            sink.leaf(head, question.is_some(), range);
        }
    }

    fn handle_void_keyword(&mut self, token: Token) {
        let void_id = self.strings.intern("void");
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.leaf(void_id, false, token.range);
        } else if let Some(sink) = self.bound_sink.as_mut() {
            sink.leaf(void_id, false, token.range);
        }
    }

    fn begin_type_arguments(&mut self, _token: Token) {
        let head = self.pending_head.take();
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.begin_group(head);
        } else if let Some(sink) = self.bound_sink.as_mut() {
            sink.begin_group(head);
        }
    }

    fn end_type_arguments(&mut self, _count: usize, begin: Token, end: Token) {
        let range = SourceRange {
            offset: begin.range.offset,
            length: end.range.end() - begin.range.offset,
        };
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.end_group(range);
        } else if let Some(sink) = self.bound_sink.as_mut() {
            sink.end_group(range);
        }
    }

    fn handle_no_type_arguments(&mut self, _token: Token) {
        self.pending_head = None;
    }

    fn begin_function_type(&mut self, _begin: Token) {
        // The function form is a first-class capture fact here too: its
        // columns open their own level in the open window, so a
        // function-shaped clause argument (`extends S<void Function()>`)
        // or a function-shaped bound captures as one faithful root
        // instead of poisoning the window. The poison belt stays for the
        // shapes the model cannot represent (record types, headless
        // groups); a function-form tree a consumer cannot map still
        // degrades there (the fold sites refuse it), the same
        // under-report direction the poison produced.
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.begin_function();
        }
        if let Some(sink) = self.bound_sink.as_mut() {
            sink.begin_function();
        }
    }

    fn end_function_type(&mut self, function: Token, question: Option<Token>) {
        let range = SourceRange {
            offset: function.range.offset,
            length: question
                .map_or(function.range, |q| q.range)
                .end()
                - function.range.offset,
        };
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.end_function(question.is_some(), range);
        }
        if let Some(sink) = self.bound_sink.as_mut() {
            sink.end_function(question.is_some(), range);
        }
    }

    fn begin_record_type(&mut self, _left_paren: Token) {
        if let Some((_, header)) = self.header.as_mut() {
            header.sink.poison();
        }
        if let Some(sink) = self.bound_sink.as_mut() {
            sink.poison();
        }
    }

    fn begin_type_variables(&mut self, _token: Token) {
        self.pending = None;
        self.bound_sink = Some(TreeSink::default());
        self.var_names.clear();
        self.window.clear();
    }

    fn begin_type_variable(&mut self, token: Token) {
        if let Some(name) = self.name_of(token) {
            self.var_names.push(name);
        }
    }

    fn end_type_variable(
        &mut self,
        token: Token,
        _index: usize,
        extends_or_super: Option<Token>,
        _variance: Option<Token>,
    ) {
        let Some(name) = self.var_names.pop().or_else(|| self.name_of(token)) else {
            return;
        };
        let bound = if extends_or_super.is_some()
            && let Some(sink) = self.bound_sink.as_mut()
        {
            // The parser completes the variables in reverse order, each
            // bound's roots closing just before its own `end_type_variable`
            // — the roots completed since the previous completion are this
            // variable's bound. Exactly one root is faithful; anything else
            // is a recovery shape the conversion suppresses (kept as `Some`
            // poisoned or `None` — the consumer maps only modelable trees).
            let mut roots = sink.take_done();
            match roots.len() {
                1 => roots.pop(),
                _ => None,
            }
        } else {
            None
        };
        self.window.push((name, bound));
    }

    fn end_type_variables(&mut self, _begin: Token, end: Token) {
        self.bound_sink = None;
        // Completion order reverses back to declaration order — the
        // capture's parameter order is the written one.
        let mut window = std::mem::take(&mut self.window);
        window.reverse();
        let (names, bounds): (Vec<StringId>, Vec<Option<TypeTree>>) =
            window.into_iter().unzip();
        self.pending = (!names.is_empty()).then_some((names, bounds, end.range.offset));
    }

    fn handle_class_extends(&mut self, extends: Option<Token>, count: usize) {
        if extends.is_some()
            && let Some((_, header)) = self.header.as_mut()
        {
            header.claim(Some(count));
        }
    }

    fn handle_implements(&mut self, implements: Option<Token>, count: usize) {
        if implements.is_some()
            && let Some((_, header)) = self.header.as_mut()
        {
            header.claim(Some(count));
        }
    }

    fn handle_class_with_clause(&mut self, _with_keyword: Token) {
        if let Some((_, header)) = self.header.as_mut() {
            header.claim(None);
        }
    }

    fn handle_named_mixin_application_with_clause(&mut self, _with_keyword: Token) {
        if let Some((_, header)) = self.header.as_mut() {
            header.claim(None);
        }
    }

    fn handle_enum_with_clause(&mut self, _with_keyword: Token) {
        if let Some((_, header)) = self.header.as_mut() {
            header.claim(None);
        }
    }

    fn handle_mixin_with_clause(&mut self, _with_keyword: Token) {
        if let Some((_, header)) = self.header.as_mut() {
            header.claim(None);
        }
    }

    fn handle_mixin_on(&mut self, on: Option<Token>, count: usize) {
        if on.is_some()
            && let Some((_, header)) = self.header.as_mut()
        {
            header.claim(Some(count));
        }
    }

    fn begin_class_or_mixin_or_extension_body(&mut self, _kind: DeclarationKind, _token: Token) {
        self.finish_header();
    }

    fn begin_enum_body(&mut self, _token: Token) {
        self.finish_header();
    }

    fn begin_class_declaration(
        &mut self,
        _begin: Token,
        _abstract_kw: Option<Token>,
        _sealed_kw: Option<Token>,
        _base_kw: Option<Token>,
        _interface_kw: Option<Token>,
        _final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _mixin_kw: Option<Token>,
        name: Token,
    ) {
        self.note_container(Some(name), false);
    }

    fn end_class_declaration(&mut self, _begin: Token, _end: Token) {
        self.header = None;
    }

    fn begin_named_mixin_application(
        &mut self,
        _begin: Token,
        _abstract_kw: Option<Token>,
        _sealed_kw: Option<Token>,
        _base_kw: Option<Token>,
        _interface_kw: Option<Token>,
        _final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _mixin_kw: Option<Token>,
        name: Token,
    ) {
        self.note_container(Some(name), false);
    }

    fn end_named_mixin_application(
        &mut self,
        _begin: Token,
        _class: Token,
        _equals: Token,
        _implements: Option<Token>,
        _end: Token,
    ) {
        // No body-begin finalizes the alias's header; the end event drops
        // it like the diagnosing capture's own shape — the alias's trees
        // contribute no entry and the name-level edges stand.
        self.header = None;
    }

    fn begin_mixin_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _base_kw: Option<Token>,
        _mixin: Token,
        name: Token,
    ) {
        self.note_container(Some(name), false);
    }

    fn end_mixin_declaration(&mut self, _begin: Token, _end: Token) {
        self.header = None;
    }

    fn begin_enum_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _enum_keyword: Token,
        name: Token,
    ) {
        self.note_container(Some(name), false);
    }

    fn end_enum_declaration(
        &mut self,
        _begin: Token,
        _enum_keyword: Token,
        _left_brace: Token,
        _member_count: usize,
        _end: Token,
    ) {
        self.header = None;
    }

    fn begin_extension_declaration(
        &mut self,
        _augment: Option<Token>,
        _extension_keyword: Token,
        name: Option<Token>,
    ) {
        self.note_container(name, false);
    }

    fn end_extension_declaration(
        &mut self,
        _begin: Token,
        _extension_keyword: Token,
        _on: Option<Token>,
        _end: Token,
    ) {
        self.header = None;
    }

    fn begin_extension_type_declaration(
        &mut self,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        name: Token,
    ) {
        self.note_container(Some(name), true);
    }

    fn end_extension_type_declaration(
        &mut self,
        _begin: Token,
        _augment_kw: Option<Token>,
        _extension_keyword: Token,
        _type_keyword: Token,
        _end: Token,
    ) {
        self.header = None;
    }
}

/// Parses one compilation unit and threads every faithfully captured
/// container clause set into `into`, keyed by `file` and the container
/// name. `tokens` must be the scan of `source` and `strings` the table the
/// ids resolve against — the same pairing the element collection runs on.
///
/// A capture lands only when its clause set parsed faithfully (`open`
/// clear, non-empty supertype set): every other container keeps the
/// name-level edges, the transport's under-report posture.
pub fn capture_clause_trees(
    file: FileId,
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
    into: &mut ClauseTrees,
) {
    let mut listener = CaptureListener {
        source,
        tokens,
        strings,
        containers: FxHashMap::default(),
        header: None,
        bound_sink: None,
        var_names: Vec::new(),
        window: Vec::new(),
        pending: None,
        pending_head: None,
    };
    Parser::new(source, tokens, features, &mut listener).parse_unit();
    for (name, row) in listener.containers {
        // A capture lands when its header parsed faithfully (`open` clear)
        // and carries something the name-level face lacks: argument-bearing
        // supertype trees, or written parameter bounds (`class C<T
        // extends num>` has no supers to speak of — its bounds are the
        // payload).
        if row.open
            || (row.supers.is_empty() && row.param_bounds.iter().all(Option::is_none))
        {
            continue;
        }
        into.insert(
            file,
            name,
            ClauseCapture {
                params: row.params.into_boxed_slice(),
                param_bounds: row.param_bounds.into_boxed_slice(),
                supers: row.supers,
            },
        );
    }
}

/// Merges `source`'s captured trees into `target` — the transport the
/// engine's dependency-interface folds ride: a dep file's captures build
/// once per analyze (they belong to no library's member set) and join
/// every library's snapshot through this merge. First-wins per
/// `(file, name)` key, the same discipline [`capture_clause_trees`]
/// applies.
pub fn extend_clause_trees(target: &mut ClauseTrees, source: &ClauseTrees) {
    for (&key, capture) in &source.trees {
        target.trees.entry(key).or_insert_with(|| ClauseCapture {
            params: capture.params.clone(),
            param_bounds: capture.param_bounds.clone(),
            supers: capture.supers.clone(),
        });
    }
}
