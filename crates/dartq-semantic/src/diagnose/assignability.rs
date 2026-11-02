use super::engine_core::flatten_ty;
use super::*;

/// The #2487 pin's instruction-count faces for the bounded
/// assignment-target scan (`assignment_target_index`'s `cfg(test)`
/// twin): the predicate visits the bounded reverse scan spent across
/// every call (the new walk's total), and the widest post-operator tail
/// any call saw (`rows_len - bound` — every row the old `.rev().find`
/// would have walked past before reaching the prefix).
#[cfg(test)]
static ASSIGNMENT_TARGET_VISITS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
/// See [`ASSIGNMENT_TARGET_VISITS`].
#[cfg(test)]
static ASSIGNMENT_TARGET_MAX_TAIL: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// The structural-descent budget of the shared assignability verdict
/// (#3333): the nesting depth of [`Engine::assignable`] frames one ask may
/// hold. The alias-name chain is bounded elsewhere ([`Engine::fold_alias_ty`]'s
/// hop set), but the STRUCTURAL descent — `assignable` →
/// [`Engine::slot_subtype`] → [`Engine::covariant_args`] →
/// [`Engine::fn_sig_subtype`] → `assignable` — is not: a self-referential
/// alias pair re-derives the same `(from, to)` pair at every covariance
/// level, and one ask crossing the pair overflowed the stack of the
/// engine's worker thread. Every cycle edge re-enters `assignable`, so the
/// frame depth is the cycle's budget.
const ASSIGNABLE_DEPTH_CAP: u32 = 256;

thread_local! {
    /// The live [`Engine::assignable`] frame depth on this thread (each
    /// analyzer run owns its thread, so one counter per thread is one
    /// counter per ask chain).
    static ASSIGNABLE_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// One [`Engine::assignable`] frame's depth ticket: `enter` refuses past
/// [`ASSIGNABLE_DEPTH_CAP`], and `Drop` unwinds the count on every return
/// path (early returns and panics alike), so the counter never leaks.
struct AssignDepth;

impl AssignDepth {
    fn enter() -> Option<Self> {
        ASSIGNABLE_DEPTH.with(|depth| {
            let live = depth.get();
            if live >= ASSIGNABLE_DEPTH_CAP {
                return None;
            }
            depth.set(live + 1);
            Some(Self)
        })
    }
}

impl Drop for AssignDepth {
    fn drop(&mut self) {
        ASSIGNABLE_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Whether a written type-argument segment spells exactly `void` around
/// whitespace and a trailing group closer (the set/map segment scan keeps
/// the `>` inside its last segment): a written `void` argument fires no
/// type-use row (the parser routes the keyword through
/// `handle_void_keyword`, never `handle_type`), so the segment is read
/// from its own bytes (#3361).
fn spells_void(bytes: &[u8], start: usize, end: usize) -> bool {
    let mut inner = bytes[start..end].trim_ascii();
    if let Some(stripped) = inner.strip_suffix(b">") {
        inner = stripped.trim_ascii();
    }
    inner == b"void"
}

impl<'a> Engine<'a> {
    /// A literal `<…>` group's argument leaf as a [`Ty`]: `dynamic`/`Null`
    /// intern their dedicated faces — the same mapping the type
    /// completion applies (`handle_type`) — since the oracle's
    /// `DynamicType`/`NullType` are never interface heads and a
    /// `Named{dynamic}` leaf answers every subtype question `Ty::Dynamic`
    /// decides (the covariant element rule's `dynamic <: int` rejection).
    /// Every other written head keeps its named shape. `void` never
    /// reaches here — the keyword fires no type-use row at all (the
    /// parser routes it through `handle_void_keyword`), and the literal
    /// group paths answer it from their own bytes.
    fn literal_arg_leaf(&self, name: StringId, nullable: bool) -> Ty {
        match self.strings.resolve(name) {
            "dynamic" => Ty::Dynamic,
            "Null" => Ty::Null,
            _ => Ty::Named {
                name,
                nullable,
                args: ArgListId::EMPTY,
            },
        }
    }

    pub(crate) fn typed_list_literal_ty(&mut self, left: Token) -> Option<Ty> {
        let bytes = self.source.as_bytes();
        // `left.range.offset` is a UTF-16 offset: convert before walking
        // source bytes so a wide char upstream cannot desync the group probe.
        let mut cursor = self.byte_of(left.range.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        // A `=>` arrow before the bracket ends in the same `>` byte a
        // written `<...>` group closes with (`seq(Parser other) => [this,
        // other]`) — the pair is an expression arrow, not a group, and the
        // nearest earlier type-use is a pattern head, not an element type.
        if cursor < 2 || bytes[cursor - 1] != b'>' || bytes[cursor - 2] == b'=' {
            return None;
        }
        // A written `void` argument fires no type-use row (the parser routes
        // the keyword through `handle_void_keyword`, never `handle_type`), so
        // the type-use find below never lands on it: the group's own bytes
        // answer, and `<void>[]` carries the canonical `List<void>` element
        // the `use_of_void_result` family reads (#3361).
        let close = cursor - 1;
        if let Some(open) = bytes[..close].iter().rposition(|&b| b == b'<')
            && spells_void(bytes, open + 1, close)
        {
            let list = self.intern("List");
            return Some(self.boundary.args.borrow_mut().named(list, false, &[Ty::Void]));
        }
        let (name_range, name) = self
            .type_uses
            .iter()
            .rev()
            .find(|(range, _)| range.end() <= left.range.offset)
            .copied()?;
        // The type-use must close the group itself: only the nullable
        // suffix, whitespace and `>` may separate it from the bracket. A
        // group spelling no type-use row (`<void Function()?>[]`) would
        // otherwise borrow an unrelated earlier name as its element.
        let name_end = self.byte_of(name_range.end()).min(cursor);
        if !bytes[name_end..cursor]
            .iter()
            .all(|&b| b == b'?' || b == b'>' || b.is_ascii_whitespace())
        {
            return None;
        }
        // An unresolvable element head still types the literal: the oracle's
        // `List<InvalidType>` keeps the `List` face (a closure formal over
        // its members infers `Object?` from the invalid argument, the walk a
        // declared `List<Und>` takes). A non-type element keeps the opaque
        // row.
        if matches!(self.type_resolution(name), (TypeResolution::NonType, _)) {
            return None;
        }
        // The written element's own nullability (`<String?>[…]`): the
        // type-use row names `String` but the `?` rides the suffix, so the
        // element type is `String?` — without it every element reads as a
        // `String? → String` assignability break (#2161). `dynamic`/`Null`
        // intern their dedicated faces — `List<dynamic>` against
        // `List<int>` is the covariant rejection the oracle reports, which
        // a `Named{dynamic}` leaf never reaches.
        let nullable = self.written_nullable_suffix(name_range.end());
        let elem = self.literal_arg_leaf(name, nullable);
        let list = self.intern("List");
        let ty = self.boundary.args.borrow_mut().named(list, false, &[elem]);
        Some(ty)
    }

    /// A typed set/map literal's static type (`<int>{}` → `Set<int>`,
    /// `<K, V>{}` → `Map<K, V>` — one written type argument is a set, two a
    /// map, the same grammar split the list face reads for `[…]`). Returns
    /// the anchor head byte (the `<` group's start, `const` handled by the
    /// caller) alongside the type; an unresolvable head, a wrong shape, or
    /// a nested-argument group (a comma the flat scan cannot attribute)
    /// keeps the literal's opaque row.
    pub(crate) fn typed_set_or_map_literal_ty(&mut self, left: Token) -> Option<(u32, Ty)> {
        let bytes = self.source.as_bytes();
        // `left.range.offset` is a UTF-16 offset: convert before walking
        // source bytes (the same conversion the list face applies).
        let mut cursor = self.byte_of(left.range.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        // The byte before `{` must close a written group (`<int>{}`) — an
        // expression-bodied arrow's `>` (`f() => {}`) is not a group, and a
        // group walk over it never balances.
        if cursor < 2 || bytes[cursor - 1] != b'>' || bytes[cursor - 2] == b'=' {
            return None;
        }
        // Walk back over the balanced `<>` group to its head; the segments
        // scan starts just after the `<`.
        let mut depth = 0usize;
        let head = loop {
            if cursor == 0 {
                // An unbalanced residue: recovery shape, not a group.
                return None;
            }
            cursor -= 1;
            match bytes[cursor] {
                b'>' => depth += 1,
                b'<' => {
                    depth -= 1;
                    if depth == 0 {
                        break cursor as u32;
                    }
                }
                _ => {}
            }
        };
        // Top-level segments: comma splits at bracket depth 0, string
        // contents skipped — one segment is a set, two a map.
        let mut segments: Vec<(usize, usize)> = Vec::new();
        let mut seg_start = head as usize + 1;
        let mut i = seg_start;
        let mut bracket = 0usize;
        let group_end = self.byte_of(left.range.offset);
        while i < group_end {
            match bytes[i] {
                b'(' | b'[' | b'{' => bracket += 1,
                b')' | b']' | b'}' => bracket = bracket.saturating_sub(1),
                b',' if bracket == 0 => {
                    segments.push((seg_start, i));
                    seg_start = i + 1;
                }
                b'\'' | b'"' => {
                    let quote = bytes[i];
                    i += 1;
                    while i < group_end {
                        if bytes[i] == b'\\' {
                            i += 2;
                        } else {
                            if bytes[i] == quote {
                                break;
                            }
                            i += 1;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
        segments.push((seg_start, group_end));
        if segments.len() > 2 {
            return None;
        }
        let args: Vec<Ty> = segments
            .iter()
            .map(|(start, end)| {
                // Segment bounds are bytes; `type_uses` ranges are UTF-16.
                let (start16, end16) = (self.utf16_of(*start), self.utf16_of(*end));
                match self
                    .type_uses
                    .iter()
                    .rev()
                    .find(|(range, _)| range.offset >= start16 && range.end() <= end16)
                    .copied()
                {
                    Some((name_range, name)) => {
                        if !matches!(self.type_resolution(name), (TypeResolution::Type, _)) {
                            return None;
                        }
                        Some(self.literal_arg_leaf(
                            name,
                            self.written_nullable_suffix(name_range.end()),
                        ))
                    }
                    // A written `void` argument fires no type-use row (the
                    // parser routes the keyword through
                    // `handle_void_keyword`, never `handle_type`): the
                    // segment's own bytes answer (`<void>{}` → `Set<void>`,
                    // `<int, void>{}` → `Map<int, void>` — #3361).
                    None => spells_void(bytes, *start, *end).then_some(Ty::Void),
                }
            })
            .collect::<Option<Vec<_>>>()?;
        let (set, map) = (self.collection_heads[1], self.collection_heads[2]);
        let ty = if args.len() == 1 {
            self.boundary.args.borrow_mut().named(set, false, &args)
        } else {
            self.boundary.args.borrow_mut().named(map, false, &args)
        };
        Some((head, ty))
    }

    /// The typed list literal's element check (`listLiteral`'s resolution,
    /// type_analyzer.dart's element loop): each element's static type must
    /// be assignable to the written element type —
    /// `list_element_type_not_assignable` at the failing element. Untyped
    /// literals infer, an unresolvable element type leaves the row to its
    /// own face, and untypeable elements stay silent (the under-report
    /// posture every assignability face shares).
    /// The element face a `...spread` operand's receiver carries: an
    /// `Iterable`-family head answers its first written argument, and a
    /// head whose superchain (extends/`with`/implements, name-level)
    /// REACHES one of the iterable heads answers the receiver's first
    /// argument the same way — `NodeListImpl<AnnotationImpl> implements
    /// List<AnnotationImpl>` (ast.dart's `..._metadata` into
    /// `<AstNode>[]`) keeps `AnnotationImpl`, the element the clause's
    /// own parameter spells. The clause's written arguments live only in
    /// the source bytes; the first-position substitution is exact for
    /// every single-parameter wrapper chain and under-reports the rest.
    fn spread_element_face(&mut self, head: StringId, args: ArgListId) -> Option<Ty> {
        let iterable_heads = [
            self.intern("Iterable"),
            self.intern("List"),
            self.intern("Set"),
        ];
        let first = || {
            self.boundary
                .pool()
                .get(args)
                .first()
                .copied()
                .filter(|ty| *ty != Ty::UNKNOWN)
        };
        if iterable_heads.contains(&head) {
            return first();
        }
        let mut queue: Vec<StringId> = vec![head];
        let mut seen: FxHashSet<StringId> = FxHashSet::from_iter([head]);
        while let Some(current) = queue.pop() {
            let Some((decl, _)) = crate::library::resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                current,
            ) else {
                continue;
            };
            let supers = decl
                .superclass
                .into_iter()
                .chain(decl.with_clause.iter().copied())
                .chain(decl.implements_clause.iter().copied());
            for next in supers {
                if iterable_heads.contains(&next) {
                    return first();
                }
                if seen.insert(next) {
                    queue.push(next);
                }
            }
        }
        None
    }

    pub(crate) fn check_list_literal_elements(&mut self, left: Token, right: Token) {
        let bytes = self.source.as_bytes();
        // A typed literal's `<` group sits immediately before the `[`.
        // `left`/`right` ranges are UTF-16: convert before walking bytes so a
        // wide char anywhere upstream cannot desync the walk (or slice
        // mid-character at the element segmentation — #2594).
        let mut cursor = self.byte_of(left.range.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        // The same arrow face [`Self::typed_list_literal_ty`] rejects: a
        // `=> [` opens an untyped switch-arm/arrow-body literal, and the
        // byte before its bracket is the arrow's `>`, never a group close.
        let declared = if cursor >= 2 && bytes[cursor - 1] == b'>' && bytes[cursor - 2] != b'=' {
            // The nearest type reference ending before the bracket names the
            // element type's GROUP — but a generic element type
            // (`<Map<K, V>>[…]`) ends with its LAST inner argument, not the
            // head. The group's own `<…>` span is walked back from the `>`
            // (angle-balanced), and the FIRST use inside the group is the
            // head — `Map`, not `V` — with the raw face the pool's
            // covariant element rule answers against the written arguments.
            let mut depth = 1usize;
            let mut group_open = cursor - 1;
            while group_open > 0 {
                match bytes[group_open - 1] {
                    b'>' => depth += 1,
                    b'<' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                group_open -= 1;
            }
            if depth != 0 {
                return;
            }
            let head_start = group_open;
            // The walked group bounds are bytes; `type_uses` ranges are UTF-16.
            let (head_start16, group_close16) =
                (self.utf16_of(head_start), self.utf16_of(cursor - 1));
            let Some((name_range, name)) = self
                .type_uses
                .iter()
                .filter(|(range, _)| range.offset >= head_start16 && range.end() <= group_close16)
                .min_by_key(|(range, _)| range.offset)
                .copied()
            else {
                return;
            };
            if !matches!(self.type_resolution(name), (TypeResolution::Type, _)) {
                return;
            }
            // The `?` suffix rides the token after the type-use range, the same
            // suffix `typed_list_literal_ty` reads (`<String?>` declares `String?`
            // elements, not `String` — #2161).
            Ty::Named {
                name,
                nullable: self.written_nullable_suffix(name_range.end()),
                args: crate::model::ArgListId::EMPTY,
            }
        } else {
            // No written group: the untyped literal resolves its elements
            // against the position's context — `=> […]` and `return […]`
            // read the innermost executable's effective return (the
            // closure's matched context `K` for a literal, the folded
            // declared return otherwise), whose element argument is the
            // declared face each element answers (`typed_literal_resolver
            // .dart` `_fromContextType`). A deeper position — an
            // initializer, an argument, a nested slot — has no return
            // context of its own here and keeps the opaque posture.
            let Some(declared) = self.list_literal_ctx_element(left) else {
                return;
            };
            declared
        };
        // Top-level element segments: comma splits at bracket depth 0,
        // string-literal contents skipped.
        let mut segments: Vec<(usize, usize)> = Vec::new();
        let mut seg_start = self.byte_of(left.range.end());
        let end = self.byte_of(right.range.offset);
        let mut i = seg_start;
        let mut depth = 0usize;
        while i < end {
            match bytes[i] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    segments.push((seg_start, i));
                    seg_start = i + 1;
                }
                b'\'' | b'"' => {
                    let quote = bytes[i];
                    i += 1;
                    while i < end && bytes[i] != quote {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        segments.push((seg_start, end));
        for (start, stop) in segments {
            // A null-aware element (`?primary`) contributes its unwrapped
            // value — the oracle's null-aware element resolver types the
            // element itself as the element type when the `?` marker is
            // present and asks no assignability question of it (the
            // `?` gate already admits the nullable operand).
            let seg_text = &self.source[start..stop];
            if seg_text.trim_start().starts_with('?') {
                continue;
            }
            // A spread element (`...elements`) contributes the spread
            // operand's ELEMENT type (`List<Directive>` spreads
            // `Directive`s): the official's element loop reads the
            // spread's flattened element (type_analyzer.dart's
            // spread-element resolution), never the iterable whole.
            let spread = seg_text.trim_start().starts_with("...");
            // A control element (`if (…) e` / `for (…) e`) contributes the
            // BODY's type, never the condition/header's — the oracle's
            // element resolver resolves the element expression alone, so a
            // row search falling back onto the condition would ask the
            // wrong question entirely (a promoted `bool` condition read
            // against the element type). The search window opens after the
            // header's balanced closing paren; a body with no typed row
            // contributes nothing.
            let mut window_start = start as u32;
            let trimmed = seg_text.trim_start();
            if trimmed.starts_with("if") || trimmed.starts_with("for") {
                if let Some(open) = seg_text.find('(') {
                    let mut depth = 0usize;
                    let bytes_head = seg_text.as_bytes();
                    let mut j = open;
                    while j < bytes_head.len() {
                        match bytes_head[j] {
                            b'(' => depth += 1,
                            b')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    window_start = start as u32 + j as u32 + 1;
                }
            }
            // Row ranges are UTF-16; the window/segment bounds are bytes.
            let (window16, stop16) = (self.utf16_of(window_start as usize), self.utf16_of(stop));
            let Some((row_ty, row_range)) = self
                .rows
                .iter()
                .rev()
                .find(|row| {
                    row.ty != Ty::UNKNOWN
                        && row.range.offset >= window16
                        && row.range.end() <= stop16
                })
                .map(|row| (row.ty, row.range))
            else {
                continue;
            };
            let mut element_ty = row_ty;
            if spread {
                // A spread element contributes its operand's ELEMENT type
                // (type_analyzer.dart's spread-element resolution), never
                // the operand whole — the face answers that contribution
                // only for an `Iterable`-reaching head. Every other shape
                // contributes nothing to the assignability ask: `dynamic`
                // `Null`/a non-iterable operand is the spread verifier's
                // row (`not_iterable_spread`/`not_null_aware_null_spread`),
                // and a raw `Iterable` donates `dynamic` — all silent here.
                // Falling back to `row_ty` fabricates the element code on
                // operands the oracle never asks this question of.
                match row_ty {
                    Ty::Named { name, args, .. } => {
                        let Some(first) = self.spread_element_face(name, args) else {
                            continue;
                        };
                        element_ty = first;
                    }
                    _ => continue,
                }
            }
            if self.assignable(element_ty, declared, false) == Some(false) {
                let (a0, a1) = (self.ty_display(element_ty), self.ty_display(declared));
                self.emit(
                    SemanticCode::ListElementTypeNotAssignable,
                    row_range,
                    &a0,
                    &a1,
                );
            }
        }
    }

    /// The untyped `[…]`'s context element type when the literal sits in a
    /// return position: the token immediately before its bracket spells
    /// `=>` or `return` (the token table's spelling — the same table
    /// [`Self::assignment_ctx_at`] reads for the write channel), and the
    /// innermost executable's effective return supplies the context. A
    /// literal's return comes from its matched context `K`
    /// ([`Self::closure_context_return`]); a declared executable's from
    /// its folded written return. `List`/`Set`/`Iterable` donate their
    /// single argument; every other shape answers `None` — including a
    /// `Function`-returning context, whose element ask is meaningless.
    fn list_literal_ctx_element(&self, left: Token) -> Option<Ty> {
        let tokens = self.tokens.as_slice();
        let index = tokens
            .partition_point(|t| t.utf16_offset < left.range.offset)
            .checked_sub(1)?;
        let text = &self.source[tokens[index].byte_range()];
        let in_return_position = text == "=>" || text == "return";
        if !in_return_position {
            return None;
        }
        let to = self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Executable { ret: Some(ret), .. } => Some(ret.ty),
            FrameKind::Executable { ret: None, .. } => self.closure_context_return(),
            _ => None,
        })?;
        let to = self.fold_alias_ty(to);
        let Ty::Named { name, args, .. } = to else {
            return None;
        };
        let heads = &self.collection_heads;
        if !(name == heads[0] || name == heads[1] || name == heads[3]) {
            return None;
        }
        self.boundary.args.borrow().get(args).first().copied()
    }

    /// The written `on` clause of a named extension, its head and written
    /// type-argument names re-read from the source (the face model is
    /// name-level): `extension E on List<Book>` answers
    /// `(List, [Book])`. `None` when the header never reaches an honest
    /// `on` — the caller keeps the name-level type.
    pub(crate) fn extension_on_type_args(
        &self,
        name: StringId,
    ) -> Option<(StringId, Vec<StringId>)> {
        // Extensions never join `decl_sites` (the unused-element face owns
        // that table's membership), so the header is located textually: the
        // `extension <name>` needle is word-bounded on both sides and the
        // read continues only over the honest `on` clause shape — anything
        // else (a string literal or comment mention) fails the shape and
        // falls back to the bare-head verdict.
        let name_text = self.strings.resolve(name);
        let needle = format!("extension {name_text}");
        let bytes = self.source.as_bytes();
        let ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
        let mut search = 0usize;
        loop {
            let at = self.source[search..].find(&needle)? + search;
            let before_ok = at == 0 || !ident_byte(bytes[at - 1]);
            let mut i = at + needle.len();
            let after_ok = !bytes.get(i).is_some_and(|b| ident_byte(*b));
            search = at + 1;
            if !before_ok || !after_ok {
                continue;
            }
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            // A generic declaration carries its type-parameter group
            // between the name and `on` (`extension E<T extends B> on
            // List<T>`): balance-skip the `<...>` before the keyword read.
            // Anything else (a string literal or comment mention) still
            // fails the `on` shape below.
            if bytes.get(i) == Some(&b'<') {
                let mut depth = 0usize;
                let mut j = i;
                while let Some(&b) = bytes.get(j) {
                    match b {
                        b'<' => depth += 1,
                        b'>' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                if depth != 0 {
                    continue;
                }
                i = j + 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
            }
            // The `on` keyword, word-bounded.
            if !(bytes.get(i) == Some(&b'o')
                && bytes.get(i + 1) == Some(&b'n')
                && !bytes.get(i + 2).is_some_and(|b| ident_byte(*b)))
            {
                continue;
            }
            i += 2;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let head_start = i;
            while i < bytes.len() && ident_byte(bytes[i]) {
                i += 1;
            }
            if i == head_start {
                return None;
            }
            let head = self.intern(&self.source[head_start..i]);
            let mut args = Vec::new();
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if bytes.get(j) == Some(&b'<') {
                j += 1;
                let mut depth = 1usize;
                let mut arg_start = j;
                loop {
                    match bytes.get(j) {
                        Some(b'<') => depth += 1,
                        Some(b'>') => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        Some(b',') if depth == 1 => {
                            args.push(self.intern(self.source[arg_start..j].trim()));
                            arg_start = j + 1;
                        }
                        None => return None,
                        _ => {}
                    }
                    j += 1;
                }
                let tail = self.source[arg_start..j].trim();
                // An extended type over the extension's own parameters
                // (`extension Ext<T> on List<T>`) is generic: the override
                // instantiates it from its type arguments or by inference
                // against the receiver (extension_member_resolver.dart
                // `_inferTypeArguments`), which the name level does not
                // model — `List<int>` into the written `List<T>` is no
                // mismatch (probed: dart analyze 3.13.1, `Ext<int>(l)` over
                // `List<int> l` reports no
                // `extension_override_argument_not_assignable`).
                args.push(self.intern(tail));
            }
            return Some((head, args));
        }
    }

    /// Whether the written `on` clause of extension `name` mentions one of
    /// the extension's own type parameters. Read textually like
    /// [`Self::extension_on_type_args`]: the first word-bounded
    /// `extension <name><...> on` header; no header (or no type-parameter
    /// group) answers `false`.
    pub(crate) fn extension_on_is_generic(&self, name: StringId) -> bool {
        let ident = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
        let needle = format!("extension {}", self.strings.resolve(name));
        let mut search = 0usize;
        while let Some(found) = self.source[search..].find(&needle) {
            let at = search + found;
            search = at + 1;
            let before_ok = self.source[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !ident(c));
            let rest = &self.source[at + needle.len()..];
            if !before_ok || rest.chars().next().is_some_and(ident) {
                continue;
            }
            let rest = rest.trim_start();
            let Some(group) = rest.strip_prefix('<') else {
                return false;
            };
            // The parameter group, balanced.
            let mut depth = 1usize;
            let mut close = None;
            for (k, c) in group.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(k);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(close) = close else {
                return false;
            };
            // Each top-level segment's leading identifier is a parameter.
            let mut params: Vec<&str> = Vec::new();
            let mut nest = 0usize;
            let mut seg = 0usize;
            let inner = &group[..close];
            for (k, c) in inner
                .char_indices()
                .chain(std::iter::once((inner.len(), ',')))
            {
                match c {
                    '<' => nest += 1,
                    '>' => nest = nest.saturating_sub(1),
                    ',' if nest == 0 => {
                        let word: String = inner[seg..k]
                            .trim_start()
                            .chars()
                            .take_while(|c| ident(*c))
                            .collect();
                        if !word.is_empty() {
                            params.push(&inner[seg..k].trim_start()[..word.len()]);
                        }
                        seg = k + 1;
                    }
                    _ => {}
                }
            }
            let after = group[close + 1..].trim_start();
            let Some(on_clause) = after.strip_prefix("on") else {
                return false;
            };
            let on_clause = on_clause.split(['{', ';']).next().unwrap_or("");
            return on_clause
                .split(|c: char| !ident(c))
                .any(|word| params.contains(&word));
        }
        false
    }

    /// The explicit extension override's receiver check
    /// (`extension_member_resolver.dart:288-313`): the single argument is
    /// the receiver, reported first when it is `void` (`use_of_void_result`
    /// — the `else if` keeps the not-assignable verdict off that argument);
    /// otherwise `!isAssignableTo(receiverType, extendedType)` reports
    /// `extension_override_argument_not_assignable`. Both anchor the WHOLE
    /// receiver expression (`.at(receiverExpression)` — `E(f()).g` anchors
    /// at `f()`, a `c.bad` operand at the property-access span). The `on`
    /// type is name-level (`on int` → `int`); a qualified or generic `on`
    /// face keeps its bare head. A missing/mistyped `on` suppresses (the
    /// inapplicable extension).
    pub(crate) fn check_extension_override_argument(
        &mut self,
        name: StringId,
        args: &[(Option<StringId>, Ty, bool, SourceRange)],
    ) {
        let [(_, arg_ty, literal, arg_range)] = args else {
            return;
        };
        // The void receiver is upstream's first arm — `resolveOverride`'s
        // `if (receiverType is VoidType)` reports before the `on`-type ask
        // and before the assignability verdict below. The argument arrives
        // as its own row (`f`), so the anchor widens to the expression the
        // oracle reports over (`f()`, `c.bad`).
        if *arg_ty == Ty::Void {
            let range = self
                .invocation_span(*arg_range)
                .or_else(|| self.property_access_span(*arg_range))
                .unwrap_or(*arg_range);
            // One row per receiver expression (the contract
            // [`Engine::emit_use_of_void_result`] enforces): the invocation
            // seam stages this row at the receiver's own row span before
            // the check runs here, so a staged row realigns to the widened
            // anchor rather than a second row stacking beside it.
            let staged = self.report.diagnostics.iter().position(|row| {
                row.code.name == "use_of_void_result" && row.range.offset == arg_range.offset
            });
            match staged {
                Some(index) => self.report.diagnostics[index].range = range,
                None => self.emit(SemanticCode::UseOfVoidResult, range, "", ""),
            }
            return;
        }
        let Some(on_type) = self
            .index
            .get(self.file)
            .and_then(|f| {
                f.extension_faces
                    .iter()
                    .find(|face| face.name == Some(name))
            })
            .and_then(|face| face.on_type)
        else {
            return;
        };
        // A written type-argument group rides the head (`on List<Book>`) —
        // the assignability verdict is generic-aware only when the
        // extension's own face carries the arguments.
        // An extended type over the extension's own type parameters
        // (`extension Ext<T> on List<T>`, or `on T`) is generic: the
        // override instantiates it from its type arguments or by inference
        // against the receiver (extension_member_resolver.dart
        // `_inferTypeArguments`), which the name level does not model —
        // the face suppresses (probed: dart analyze 3.13.1 — `Ext<int>(l)`
        // over `List<int> l` reports no
        // `extension_override_argument_not_assignable`).
        if self.extension_on_is_generic(name) {
            return;
        }
        let extended = match self.extension_on_type_args(name) {
            Some((head, args)) if head == on_type && !args.is_empty() => {
                let arg_tys: Vec<Ty> = args.iter().map(|arg| Ty::named(*arg)).collect();
                self.boundary.args.borrow_mut().named(head, false, &arg_tys)
            }
            _ => Ty::named(on_type),
        };
        let rejects = match self.assignable(*arg_ty, extended, *literal) {
            Some(verdict) => !verdict,
            // Name-level generic faces answer `None` (`on List<Book>`
            // against a `List<int>` receiver — the subtype walk cannot
            // compare unresolved argument names). The resolver's invariance
            // decides from the written arguments alone: different heads
            // reject outright, a same-head pair rejects on any differing
            // argument (unknown slots defer — the under-report posture).
            None => match (*arg_ty, extended) {
                (
                    Ty::Named {
                        name: from_name,
                        args: from_args,
                        ..
                    },
                    Ty::Named {
                        name: to_name,
                        args: to_args,
                        ..
                    },
                ) => {
                    if from_name != to_name {
                        true
                    } else {
                        let pool = self.boundary.pool();
                        let (from_args, to_args) = (pool.get(from_args), pool.get(to_args));
                        from_args.len() == to_args.len()
                            && from_args
                                .iter()
                                .zip(to_args.iter())
                                .any(|(a, b)| match (a, b) {
                                    (Ty::Named { name: an, .. }, Ty::Named { name: bn, .. }) => {
                                        an != bn
                                    }
                                    (a, b) => {
                                        a != b
                                            && !matches!(a, Ty::Unknown)
                                            && !matches!(b, Ty::Unknown)
                                    }
                                })
                    }
                }
                _ => false,
            },
        };
        if !rejects {
            return;
        }
        let range = self
            .invocation_span(*arg_range)
            .or_else(|| self.property_access_span(*arg_range))
            .unwrap_or(*arg_range);
        let (a0, a1) = (self.ty_display(*arg_ty), self.ty_display(extended));
        // The why-not-promoted rows hang on the receiver expression's read
        // (#1877; upstream Dart SDK source,
        // pkg/analyzer/lib/src/dart/resolver/extension_member_resolver.dart:297-313
        // — `whyNotPromotedArguments[0]`).
        self.emit_non_promotion(
            SemanticCode::ExtensionOverrideArgumentNotAssignable,
            range,
            &a0,
            &a1,
            arg_range.offset,
        );
    }

    /// The receiver classification of a bare row's info.
    pub(crate) fn receiver_of_bare(info: BareInfo) -> Receiver {
        match info {
            BareInfo::Container { name, kind } => Receiver::Container { name, kind },
            BareInfo::Prefix { name } => Receiver::Prefix { name },
            BareInfo::Plain { ty } => match ty {
                Ty::Named {
                    name: type_name, ..
                } => Receiver::Typed { type_name, ty },
                _ => Receiver::Unknown,
            },
            // The receiver itself is undefined; its own row reports, so the
            // member access must not.
            BareInfo::Missing { .. } => Receiver::Unknown,
        }
    }

    /// The `Never` type identity at name level: the non-nullable `Never`
    /// spelling (`identical(receiverType, NeverTypeImpl.instance)`;
    /// `Never?` interns as `Null`, so the nullable projection never
    /// answers yes). Both interned faces answer — the `Ty::Named` written
    /// spelling and the canonical [`Ty::Never`] variant (a `throw`'s row
    /// rewrite, a normalized promotion staging, or the resolved seam's
    /// `TypeKind::Never` fold, resolved_ops.rs:6606).
    pub(crate) fn is_never_ty(&self, ty: Ty) -> bool {
        match ty {
            Ty::Never => true,
            Ty::Named {
                name,
                nullable: false,
                ..
            } => name == self.intern("Never"),
            _ => false,
        }
    }

    /// The non-null face of a semantic type (#1409): the nullable named
    /// form drops its `?`; a `Null`-declared source takes the non-nullable
    /// `Never` face (the official `promoteToNonNull` maps the null type to
    /// `Never`, pkg/analyzer/lib/src/dart/element/type_system.dart:1467-1495;
    /// the flow layer's own non-null face does the same, `Base::Null →
    /// NEVER`). `dynamic`, `void`, and unknowns are themselves — a top
    /// type or the sentinel is promotion-inert. The `Never` face is the
    /// name-level spelling: downstream it answers [`Engine::is_never_ty`]
    /// and re-enters the assignability matrix, where a distinct-name pair
    /// suppresses (the under-report direction) — the `Null` face's
    /// decidable refusals drop out. A nullable type-parameter reference
    /// falls to the inert `other => other` arm where upstream builds the
    /// `T & NonNull` face (type_system.dart:1467-1495's parameter branch)
    /// — no promotion stands here, so no row fires: the documented
    /// under-report policy.
    pub(crate) fn non_null_face(&self, ty: Ty) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable: true,
                args,
            } => Ty::Named {
                name,
                nullable: false,
                args,
            },
            Ty::Null => Ty::Named {
                name: self.intern("Never"),
                nullable: false,
                args: ArgListId::EMPTY,
            },
            other => other,
        }
    }

    /// The receiver row's expression type at a never-receiver junction: a
    /// declared-type read keeps its push-time snapshot; an invoked callee's
    /// result is the callee's declared return type (`f()`'s static type IS
    /// `f`'s declared return type — the official `staticType` of the
    /// resolved invocation). Container and keyword receivers take the
    /// static-access faces (a declaration name is never the type) and
    /// everything untyped suppresses — the under-report direction.
    pub(crate) fn receiver_static_ty(&self, row: &Row) -> Ty {
        // The row type wins when set: it carries the postfix upgrades
        // (`c!`'s `promoteToNonNull` rewrite in [`Self::check_null_assert`])
        // the push-time `Plain` payload cannot see — `x!?.m`'s receiver
        // reads non-null at the merge (`_checkForUnnecessaryNullAware`'s
        // flow-aware static type).
        if row.ty != Ty::UNKNOWN {
            return row.ty;
        }
        if let RowShape::Bare(BareInfo::Plain { ty }) = &row.shape
            && *ty != Ty::UNKNOWN
        {
            return *ty;
        }
        if !matches!(row.intent, Intent::Invoke) {
            // A getter/field READ's static type is the member's declared
            // type (the official PropertyAccess/Identifier staticType):
            // `sub.key`'s receiver reads the getter's declared return with
            // no invocation in between — `Sub get sub` types the bare
            // implicit-this read and the merged `this.sub` chain node alike
            // (webview android_webview_controller.dart:1131
            // `_androidParams.key`). A method read is a tear-off — its
            // static type is the function signature the name-level model
            // does not render, and answering the return type would resolve
            // the next member against the wrong face — so only
            // accessor/field entries answer.
            return self.accessor_read_ty(row);
        }
        self.callee_return_ty(row)
    }

    /// The declared type of a getter/field read row. The implicit-this bare
    /// face resolves the name through the enclosing container's member set
    /// (`member_declared_ty` answers fields only — getters ride here); a
    /// merged member row resolves through the receiver it names. Every
    /// other shape (a broken climb, executables, unknown receivers) keeps
    /// the unknown face.
    /// The accessor (getter/field) face of `name` read on `recv`, keyed by
    /// the recorded member-read facts rather than a live row — the face the
    /// if-case null-check binder gate re-derives for a MEMBER scrutinee
    /// (`transformer?.value case final t?`), whose merged row's own ty
    /// stays empty (the merge's declared-type write answers fields only,
    /// the documented under-report posture for getters). The container
    /// walk plus instantiation mirror [`Self::accessor_read_ty`]'s Member
    /// arm; the SDK static fallbacks stay out (they read row-relative
    /// facts this face has no row for) — a miss keeps `Unknown`.
    pub(crate) fn accessor_face_for(&self, recv: &Receiver, name: StringId) -> Ty {
        let container = match recv {
            Receiver::This { container, .. }
            | Receiver::Super { container, .. }
            | Receiver::Container {
                name: container, ..
            }
            | Receiver::Created {
                name: container, ..
            }
            | Receiver::Typed {
                type_name: container,
                ..
            } => Some(*container),
            _ => None,
        };
        let Some(container) = container else {
            return Ty::UNKNOWN;
        };
        let found = if matches!(recv, Receiver::Super { .. }) {
            self.walk_from(container, name, true)
        } else {
            self.walk_members(container, name)
        };
        match found {
            Walk::Found(entry) if matches!(entry.kind, MemberKind::Getter | MemberKind::Field) => {
                let declaring = self.member_faces.last_origin().unwrap_or(container);
                let answer = self.accessor_declared_ty(declaring, name, &entry);
                self.instantiate_accessor_static(container, recv, answer)
            }
            _ => Ty::UNKNOWN,
        }
    }

    /// The accessor entry's declared static type: a field's model `var_ty`
    /// (with the inferred overlay — [`Self::field_model_ty`]) or a getter's
    /// return, folded through a type alias first (the resolved-face
    /// convention every declared type the frames bind follows). `container`
    /// is the member walk's starting container; `name` the member spelling
    /// the overlay keys on.
    fn accessor_declared_ty(&self, container: StringId, name: StringId, entry: &MemberEntry) -> Ty {
        match entry.kind {
            MemberKind::Field => {
                self.fold_alias_ty(self.field_model_ty(container, name, entry.var_ty))
            }
            _ => self.fold_alias_ty(entry.return_ty),
        }
    }

    fn accessor_read_ty(&self, row: &Row) -> Ty {
        match &row.shape {
            RowShape::Bare(BareInfo::Plain { .. }) if !row.qualified => {
                let resolved = self.resolve_bare(row.name);
                match resolved {
                    (_, RefTarget::Member { container }) => {
                        match self.walk_members(container, row.name) {
                            Walk::Found(entry)
                                if matches!(entry.kind, MemberKind::Getter | MemberKind::Field) =>
                            {
                                self.accessor_declared_ty(container, row.name, &entry)
                            }
                            _ => Ty::UNKNOWN,
                        }
                    }
                    // The top-level getter face (#2449 F1): `Plain get g`
                    // captured as a `TopLevelFn` carries its static type on
                    // `return_ty` while `var_ty` stays `Unknown`, so the
                    // push-time `BareInfo::Plain` is information-less and
                    // every downstream member/null-aware check on the read
                    // silently skipped. The read's static type IS the
                    // getter's declared return — the official
                    // `PropertyAccess`/`Identifier.staticType` resolves the
                    // accessor element and reads its return. A top-level
                    // *function* (a formal list stands — `formals.is_some()`)
                    // read as a value is a tear-off whose static type is the
                    // function signature the name-level model does not
                    // render, so it keeps the unknown face rather than
                    // answering the return type (that would resolve the next
                    // member against the callee's result, not the function
                    // type).
                    (_, RefTarget::Decl { .. }) => match self.top_level_decl(row.name) {
                        Some(decl)
                            if decl.kind == MicroKind::TopLevelFn && decl.formals.is_none() =>
                        {
                            self.fold_alias_ty(decl.return_ty)
                        }
                        // A top-level VARIABLE's read is its declared type —
                        // the model's `var_ty`, overlaid by the inferred face
                        // for a `var`/`final` declaration the collector
                        // cannot type.
                        Some(decl) if decl.kind == MicroKind::TopLevelVar => {
                            self.fold_alias_ty(self.top_var_model_ty(row.name, decl.var_ty))
                        }
                        _ => Ty::UNKNOWN,
                    },
                    _ => Ty::UNKNOWN,
                }
            }
            // A prefixed top-level read (`diag.deadCode`): the prefix's
            // import table resolves the declaration, whose declared type is
            // the read's static type — a variable's `var_ty`, a getter's
            // return (the bare arm's getter rule; a function tear-off keeps
            // the unknown face). Containers and misses stay unknown.
            RowShape::Member {
                recv: Receiver::Prefix { name: prefix },
            } => {
                let dartq_resolution::Lookup::Found(NamespaceEntry::Decl(file)) = self
                    .namespaces
                    .lookup_prefixed(self.graph, self.lib, *prefix, row.name)
                else {
                    return Ty::UNKNOWN;
                };
                let Some(decl) = self.index.get(file).and_then(|f| f.decl(row.name)) else {
                    return Ty::UNKNOWN;
                };
                let decl = self.boundary.top(file, row.name, decl);
                match decl.kind {
                    MicroKind::TopLevelVar => self.fold_alias_ty(decl.var_ty),
                    MicroKind::TopLevelFn if decl.formals.is_none() => {
                        self.fold_alias_ty(decl.return_ty)
                    }
                    _ => Ty::UNKNOWN,
                }
            }
            RowShape::Member { recv } => {
                // A `Created` receiver (`Holder().field`) names the created
                // container exactly like the typed/this faces — the member
                // read resolves on the creation's class, so the leaf
                // accessor type answers (`field`'s declared `Plain?`).
                let container = match recv {
                    Receiver::This { container, .. }
                    | Receiver::Super { container, .. }
                    | Receiver::Container {
                        name: container, ..
                    }
                    | Receiver::Created {
                        name: container, ..
                    }
                    | Receiver::Typed {
                        type_name: container,
                        ..
                    } => Some(*container),
                    _ => None,
                };
                match container {
                    Some(container) => {
                        // A `super.member` read resolves against the SUPER
                        // declaration, never the enclosing override — the
                        // override's own covariant type would make a legal
                        // `super.member!` read unnecessary (the official's
                        // super property lookup starts past the enclosing
                        // class's own member map, the same face
                        // `walk_from(skip_own)` owns for `super.m` formals).
                        //
                        // Otherwise the receiver re-derives against the
                        // flow's promoted read first (#1465 — the same
                        // re-decision `resolve_member_row` makes for the
                        // member verdict): a read whose receiver promoted
                        // (`type is TypeParameterType` arm) answers the
                        // member's face on the PROMOTED spelling
                        // (`TypeParameterElement`, never the declared
                        // face's wider `Element?`). The promoted walk wins
                        // only when it finds an accessor — every other
                        // outcome (no promotion, an unresolvable promoted
                        // head, a method-only hit) keeps the declared
                        // container's own walk, so the re-derivation can
                        // never widen or lose a face the declared walk
                        // answered.
                        let found = if matches!(recv, Receiver::Super { .. }) {
                            self.walk_from(container, row.name, true)
                        } else {
                            self.walk_members(container, row.name)
                        };
                        let recv = recv.clone();
                        // The declaring container of the found member (the
                        // climb's origin): an inherited accessor (`first`
                        // declared on `ListMixin<E>`) instantiates against
                        // the receiver viewed as that container, the same
                        // `asInstanceOf` env the send solver reads.
                        let declaring = self.member_faces.last_origin().unwrap_or(container);
                        match found {
                            Walk::Found(entry)
                                if matches!(entry.kind, MemberKind::Getter | MemberKind::Field) =>
                            {
                                let declared = self.accessor_declared_ty(declaring, row.name, &entry);
                                let inherited = match &recv {
                                    Receiver::Typed { ty, .. } if declaring != container => self
                                        .member_recv_env(*ty, declaring, &entry)
                                        .and_then(|env| self.subst_params(declared, &env)),
                                    _ => None,
                                };
                                inherited.unwrap_or_else(|| {
                                    self.instantiate_accessor_static(container, &recv, declared)
                                })
                            }
                            _ => self
                                .core_accessor_read_ty(row, container, &recv)
                                .or_else(|| self.core_element_read_ty(row, container, &recv))
                                .unwrap_or_else(|| self.object_instance_read_ty(row)),
                        }
                    }
                    None => self.object_instance_read_ty(row),
                }
            }
            _ => Ty::UNKNOWN,
        }
    }

    /// A builtin core receiver's accessor READ on the member-walk miss:
    /// the SDK libraries carry no element model, so the getter-shaped
    /// static-table entries answer instead — every
    /// [`crate::member_face::CoreRet::RecvArg`] entry is a getter
    /// (`Map<K, V>.values` answers the `Iterable` of the receiver's
    /// second written argument, official map.dart `Iterable<V> get
    /// values`), so a method tear-off read never lands here by
    /// construction. A container the graph element-models resolves
    /// through the walk above and never reaches this face; a receiver
    /// without written arguments keeps the miss.
    fn core_accessor_read_ty(&self, row: &Row, container: StringId, recv: &Receiver) -> Option<Ty> {
        let crate::member_face::CoreRet::RecvArg(head, index) =
            crate::member_face::core_member_return(
                self.strings.resolve(container),
                self.strings.resolve(row.name),
            )?
        else {
            return None;
        };
        let name = self.members.core_ret_id(head)?;
        let Receiver::Typed { ty, .. } = recv else {
            return None;
        };
        let Ty::Named { args, .. } = ty else {
            return None;
        };
        let elem = self
            .boundary
            .pool()
            .get(*args)
            .get(index as usize)
            .copied()?;
        Some(
            self.boundary
                .args
                .borrow_mut()
                .named(name, false, std::slice::from_ref(&elem)),
        )
    }

    /// The element-getter twin of [`Self::core_accessor_read_ty`]:
    /// `Iterable<E>.first` answers `E` verbatim (iterable.dart), so a
    /// `??=` right side over a `List<Node>.last` read types `Node` and
    /// the write promotes the target non-null (treebuilder.dart's
    /// `parent ??= openElements.last` — the oracle's silent `.nodes`
    /// read after it).
    fn core_element_read_ty(&self, row: &Row, container: StringId, recv: &Receiver) -> Option<Ty> {
        let crate::member_face::CoreRet::ElemArg(index) = crate::member_face::core_member_return(
            self.strings.resolve(container),
            self.strings.resolve(row.name),
        )?
        else {
            return None;
        };
        let Receiver::Typed { ty, .. } = recv else {
            return None;
        };
        let Ty::Named { args, .. } = ty else {
            return None;
        };
        self.boundary.pool().get(*args).get(index as usize).copied()
    }

    /// Instantiates a resolved accessor's declared type with the
    /// receiver's written type arguments — the official `_executeGetter`
    /// face (`property_element_resolver.dart`): the getter's return
    /// carries the receiver's instantiation, so `Map<K, V>.values` under
    /// a `Map<String, ClassItem>` receiver answers `Iterable<ClassItem>`.
    /// A container without parameters keeps the declared face verbatim; a
    /// receiver the model cannot instantiate (no written arguments, an
    /// arity the declaration rejects) keeps the under-report — the RAW
    /// schema must never leak as the read's static type (its
    /// type-parameter spellings are unprovable in the caller's scope and
    /// starve every downstream solve into the `Object?` recovery).
    pub(crate) fn instantiate_accessor_static(
        &self,
        container: StringId,
        recv: &Receiver,
        declared: Ty,
    ) -> Ty {
        // The declaring container's parameter list: this unit's own capture
        // first, the dart:core table for the SDK heads whose capture lives
        // in their own unit's parse. An EMPTY capture never claims the
        // authority — a head the index met cross-unit (the distilled core
        // face's container rows carry no parameter spellings) must fall
        // through to the table, else the raw schema leaks as the read's
        // static type (a `MapEntry.key` answering the bare `K`).
        let own_params = self
            .bounds
            .container_params(container)
            .filter(|params| !params.is_empty());
        let core_params = own_params.is_none().then(|| {
            CORE_CONTAINER_PARAMS
                .iter()
                .find(|(head, ..)| *head == self.strings.resolve(container))
                .map(|(_, params)| {
                    params
                        .iter()
                        .map(|p| (self.intern(p), None, false))
                        .collect::<Vec<_>>()
                })
        });
        let params = match own_params.or_else(|| core_params.flatten()) {
            Some(params) if !params.is_empty() => params,
            _ => {
                let raw = self.raw_receiver_env(recv);
                if raw.is_empty()
                    && let Receiver::Typed {
                        ty: Ty::Named { args, .. },
                        ..
                    } = *recv
                    && args != ArgListId::EMPTY
                    && let Some(found) = resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        container,
                    )
                    .map(|(found, _)| found)
                    .or_else(|| {
                        // A head reached only through a foreign member
                        // signature (not importable here): the member walk
                        // that just resolved the accessor recorded its
                        // declaring file.
                        (self.member_faces.last_origin() == Some(container))
                            .then(|| self.member_faces.last_found_file())
                            .flatten()
                            .and_then(|file| self.index.get(file)?.container(container))
                    })
                {
                    // A foreign container's own spellings (its element
                    // model's declaration order) pair the receiver's
                    // written arguments: a `T?` face lifts, and a compound
                    // face (`Res<V>? get result` over `Obs<N>`) substitutes
                    // to the receiver's instantiation (`Res<N>?`).
                    let written = self.boundary.pool().get(args).to_vec();
                    if found.type_param_names.len() == written.len() {
                        let map: Vec<(StringId, Ty)> = found
                            .type_param_names
                            .iter()
                            .copied()
                            .zip(written)
                            .collect();
                        if let Some(lifted) = self.lift_nullable_param(declared, &map) {
                            return lifted;
                        }
                        if let Some(out) = self.subst_params(declared, &map) {
                            return out;
                        }
                    }
                }
                return if raw.is_empty() {
                    declared
                } else {
                    self.subst_params(declared, &raw).unwrap_or(Ty::UNKNOWN)
                };
            }
        };
        let receiver_ty = match recv {
            Receiver::Typed { ty, .. } => *ty,
            // `this` carries the enclosing face's own instantiation.
            Receiver::This { container, kind } => self.this_static_ty(*container, *kind),
            Receiver::Created {
                name,
                kind: ContainerKind::Class,
                ..
            } => Ty::named(*name),
            _ => return Ty::UNKNOWN,
        };
        let Ty::Named { args, .. } = receiver_ty else {
            return Ty::UNKNOWN;
        };
        let recv_args = self.boundary.pool().get(args).to_vec();
        if recv_args.is_empty() {
            let raw = self.raw_receiver_env(recv);
            if !raw.is_empty() {
                return self.subst_params(declared, &raw).unwrap_or(Ty::UNKNOWN);
            }
        }
        if recv_args.len() != params.len() {
            return Ty::UNKNOWN;
        }
        let map: Vec<(StringId, Ty)> = params
            .iter()
            .zip(recv_args.iter())
            .map(|((param, ..), arg)| (*param, *arg))
            .collect();
        if let Some(lifted) = self.lift_nullable_param(declared, &map) {
            return lifted;
        }
        self.subst_params(declared, &map).unwrap_or(Ty::UNKNOWN)
    }

    /// A bare nullable parameter face (`T? get value`) read through a
    /// receiver whose argument for `T` is a named type (`R<N>`, `R<C<int>>`)
    /// reads that type made nullable — `N?`, `C<int>?` — the substitution
    /// [`Self::subst_params`] declines for the `?` leaf (its formal/override
    /// consumers keep that posture). A scope-parameter argument's
    /// nullability classification rides the parameter's own bound (an
    /// invalid bound makes `N?` null-or-equivalent, the oracle's dead
    /// `!= null` arm). This is a member READ's static type only.
    fn lift_nullable_param(&self, declared: Ty, map: &[(StringId, Ty)]) -> Option<Ty> {
        let Ty::Named {
            name,
            nullable: true,
            args: ArgListId::EMPTY,
        } = declared
        else {
            return None;
        };
        let (_, arg) = map.iter().find(|(param, _)| *param == name)?;
        match *arg {
            Ty::Named { .. } => Some(Self::nullable_form(*arg)),
            _ => None,
        }
    }

    /// The `Object` getter face for a member READ the container walk
    /// cannot resolve (#2521): `hashCode`/`runtimeType` are `Object`'s own
    /// getters, present on every receiver, so an unindexed head's walk
    /// miss still types them (`callback.hashCode` answers `int` through
    /// the function value's chain exactly as the official resolver does).
    /// A method spelling never lands here — a bare `toString` read is a
    /// tear-off typed by its signature, which this face cannot spell —
    /// and the static class-head face (a `Container` receiver) keeps the
    /// miss: statics are not inherited. An Unknown-classified local
    /// receiver re-reads the merge's side-table static type for the same
    /// face (a function value, a record, a `dynamic`).
    fn object_instance_read_ty(&self, row: &Row) -> Ty {
        let Some((_, head)) = OBJECT_GETTER_RETURNS
            .iter()
            .find(|(member, _)| *member == &self.strings.resolve(row.name)[..])
        else {
            return Ty::UNKNOWN;
        };
        match &row.shape {
            RowShape::Member {
                recv:
                    Receiver::Typed { .. }
                    | Receiver::This { .. }
                    | Receiver::Super { .. }
                    | Receiver::Created { .. },
            } => Ty::named(self.intern(head)),
            RowShape::Member {
                recv: Receiver::Unknown,
            } if let Some(declared) = self.member_recv_tys.get(&row.range.offset)
                && !matches!(
                    self.fold_alias_ty(*declared),
                    Ty::Unknown | Ty::Void | Ty::Never
                ) =>
            {
                Ty::named(self.intern(head))
            }
            _ => Ty::UNKNOWN,
        }
    }

    /// The flow read type at one read span (#1056's rows): the promoted
    /// type when a promotion stands, the declared type otherwise. `None`
    /// when no single type stands — a repeat read walked under two flow
    /// states stages diverging rows, and the static face decides (the same
    /// poison rule [`PromotionStaging`] applies to its own rows).
    pub(crate) fn read_type_at(&self, range: SourceRange) -> Option<Ty> {
        let start = self
            .read_types
            .partition_point(|(row, _)| row.offset < range.offset);
        let end =
            start + self.read_types[start..].partition_point(|(row, _)| row.offset == range.offset);
        let mut rows = self.read_types[start..end].iter();
        let (row, ty) = *rows.next()?;
        (row == range && rows.all(|(_, other)| *other == ty)).then_some(ty)
    }

    /// The receiver a merged member row resolves against, re-decided
    /// against the receiver expression's re-derived static type
    /// ([`Self::rederived_receiver`]).
    pub(crate) fn promoted_receiver(&self, member_offset: u32, recv: Receiver) -> Receiver {
        self.rederived_receiver(member_offset, recv)
    }

    /// [`Self::callee_return_ty`] widened with the builtin core member
    /// face — the ASSIGNMENT-RHS staging's resolver alone (the flow
    /// write's promotion needs the resolved return; every other send
    /// consumer keeps the unknown-suppress posture over receivers the
    /// graph carries no element model for, where a wrong face would
    /// fire rows the oracle never reports). `context` is the write
    /// target's declared face: a bare-container element constructor
    /// (`List<E> of(Iterable<E>)`) spells no receiver arguments, and the
    /// oracle's downward inference reads `E` from the assignment's
    /// context — `Iterable<String>? x; x = List.of(xs)` writes
    /// `List<String>`, the context's element.
    pub(crate) fn callee_return_ty_for_write(&self, row: &Row, context: Ty) -> Ty {
        let resolved = self.callee_return_ty(row);
        if resolved != Ty::UNKNOWN {
            return resolved;
        }
        let container = match &row.shape {
            RowShape::Member {
                recv:
                    Receiver::This { container, .. }
                    | Receiver::Super { container, .. }
                    | Receiver::Container {
                        name: container, ..
                    }
                    | Receiver::Typed {
                        type_name: container,
                        ..
                    },
            } => Some(*container),
            _ => None,
        }
        .or_else(|| {
            let index = self
                .rows
                .partition_point(|probe| probe.range.offset < row.range.offset);
            let receiver = self.rows.get(index.checked_sub(1)?)?;
            (receiver.range.end() <= row.range.offset).then_some(receiver.name)
        });
        let Some(container) = container else {
            return Ty::UNKNOWN;
        };
        // A builtin core receiver's member send: the SDK libraries carry
        // no element model, so the container walk below cannot resolve
        // the declared return — the static face table answers instead
        // (`s.substring(..)` reads `String`; a collection transform
        // re-heads the receiver's own arguments, `xs.toList()` answering
        // the `List` of what `xs` is; a member whose element type rides
        // the callee's inference answers the bare head, keeping the
        // question decidable). The lookup keys the receiver head's
        // spelling, so a container the graph element-models (a user
        // class, however spelled) never hits; constructors stay out of
        // the table and keep their own face below.
        let member = self.strings.resolve(row.name);
        if let Some(face) =
            crate::member_face::core_member_return(self.strings.resolve(container), member)
        {
            let head = |spelling: &str| self.members.core_ret_id(spelling);
            let receiver_args = || {
                let index = self
                    .rows
                    .partition_point(|probe| probe.range.offset < row.range.offset);
                self.rows
                    .get(index.checked_sub(1)?)
                    .and_then(|recv| match recv.ty {
                        Ty::Named { args, .. } if recv.range.end() <= row.range.offset => {
                            Some(args)
                        }
                        _ => None,
                    })
            };
            // A bare container head's element constructors (`List<E>
            // unmodifiable(Iterable<E> elements)`, `List<E> of(Iterable<E>)`)
            // spell no receiver arguments — the receiver row's EMPTY list
            // would instantiate `E` to its bound (`dynamic`), staging a
            // `List<dynamic>` the promotion gate rightly refuses against
            // an `Iterable<String>` interest. The oracle's inference reads
            // `E` from the assignment's downward context: the write
            // target's declared element args re-head the constructor.
            let element_from_context = || {
                if member != "unmodifiable" && member != "of" {
                    return None;
                }
                match context {
                    Ty::Named {
                        name: ctx_head,
                        args,
                        ..
                    } if args != crate::model::ArgListId::EMPTY
                        && matches!(
                            self.strings.resolve(ctx_head),
                            "Iterable" | "List" | "Set"
                        ) =>
                    {
                        Some(args)
                    }
                    _ => None,
                }
            };
            return match face {
                crate::member_face::CoreRet::Fixed(h) => head(h).map(Ty::named),
                crate::member_face::CoreRet::BareHead(h) => head(h).map(Ty::named),
                crate::member_face::CoreRet::RecvHead(h) => head(h).and_then(|name| {
                    let args = receiver_args()
                        .filter(|args| *args != crate::model::ArgListId::EMPTY)
                        .or_else(element_from_context)
                        .or_else(receiver_args);
                    args.map(|args| Ty::Named {
                        name,
                        nullable: false,
                        args,
                    })
                }),
                crate::member_face::CoreRet::RecvArg(h, index) => head(h).and_then(|name| {
                    // One SELECTED receiver argument types the head —
                    // a `Map<K, V>.values` send answers the `Iterable`
                    // of the second written argument alone.
                    let elem = receiver_args().and_then(|args| {
                        self.boundary.pool().get(args).get(index as usize).copied()
                    });
                    elem.map(|elem| {
                        self.boundary.args.borrow_mut().named(
                            name,
                            false,
                            std::slice::from_ref(&elem),
                        )
                    })
                }),
                crate::member_face::CoreRet::ElemArg(index) => receiver_args().and_then(|args| {
                    // The receiver's argument VERBATIM — `Iterable<E>.first`
                    // answers `E` (iterable.dart's `E get first`).
                    self.boundary.pool().get(args).get(index as usize).copied()
                }),
            }
            .unwrap_or(Ty::UNKNOWN);
        }
        Ty::UNKNOWN
    }

    /// The declared return type of an invoked callee row (a completed
    /// send's result type): a bare callee resolves through the top-level
    /// namespace — a same-named local shadows it (suppress; its declared
    /// type already rode the row when captured) — and a member callee
    /// walks the container its merged receiver names. Unresolvable shapes
    /// suppress.
    pub(crate) fn callee_return_ty(&self, row: &Row) -> Ty {
        // The numeric-call refinement's answer (`clamp`, `remainder`)
        // wins over the member walk's declared return — it stashed at the
        // argument group's close, where the typed arguments were live.
        if let Some(refined) = self.refined_call_returns.get(&row.range.offset) {
            return *refined;
        }
        if !row.qualified {
            if self
                .frames
                .iter()
                .rev()
                .any(|f| f.locals.contains(&row.name))
            {
                // A local callee's return type: only the old-style
                // function-typed formal carries a written one
                // (`int f()` — its `fn_formal_rets` slot, #1758). Every
                // other local's annotation is its own type, not a
                // signature's, so the under-report posture stands — except
                // a formal whose written annotation IS a function type
                // (`bool Function(E) test`): its signature is stashed in
                // `fn_var_sigs`, and the invoke's return is its `return_ty`
                // (`if (test(event))` reads `bool` — without which
                // `non_bool_condition` over-fires).
                if let Some(ret) = self
                    .frames
                    .iter()
                    .rev()
                    .find_map(|f| f.fn_formal_rets.get(&row.name).copied())
                {
                    return ret;
                }
                if let Some(sig) = self.fn_var_sigs.get(&row.name) {
                    return sig.return_ty;
                }
                // A local typed by a parameterized function-type alias
                // (`Predicate<T> test` — the alias expands to a signature
                // whose return drives the send's result; without it the
                // unknown result over-fires `non_bool_condition` on
                // `if (test(item))`).
                if let RowShape::Bare(BareInfo::Plain {
                    ty: Ty::Named { name, .. },
                }) = &row.shape
                {
                    if let Some(sig) = self.alias_fn_sig(*name) {
                        return sig.return_ty;
                    }
                }
                // An implicit-call local (`spec(context)` over a local
                // promoted to a class carrying `call`): the invocation's
                // result is the `call` member's return on the callee's
                // FLOW read type — the promoted spelling at the callee's
                // own span (a `switch (x) case XPathExpression():` arm's
                // `x(context)` answers `XPathSequence`, not the declared
                // `Object`); no flow row or no `call` member keeps the
                // unknown under-report posture.
                if let Some(read) = self.read_type_at(row.range)
                    && let Some((_, call_ret, _)) = self.implicit_call_face(read)
                {
                    return call_ret;
                }
                return Ty::UNKNOWN;
            }
            // An unqualified member invocation (`die(x)` inside the class —
            // the implicit-this call): the bare scope resolves it to the
            // enclosing container's member, whose declared return drives the
            // send's result (`Never die(...)` marks the continuation dead —
            // the real-corpus `usageException` idiom, #2161).
            if let (_, RefTarget::Member { container }) = self.resolve_bare(row.name)
                && let Walk::Found(entry) = self.walk_members(container, row.name)
            {
                let result = self.member_invoke_result(&entry);
                if result != Ty::UNKNOWN {
                    let kind = self
                        .container_kind_of(container)
                        .unwrap_or(ContainerKind::Class);
                    let recv = Receiver::This { container, kind };
                    return self.instantiate_call_return(row, &recv, result);
                }
            }
            return self.top_level_decl(row.name).map_or(Ty::UNKNOWN, |decl| {
                // A container head invoked bare is a creation — `C()`,
                // `C<int>()` — whose static type is the instantiated head
                // (`visitInstanceCreationExpression`), never the
                // declaration's absent `return_ty`. The written
                // `<...>` group instantiates it when spelled; a generic
                // head with no group stays the inferencing residual the
                // send-close solver owns (`Unknown`).
                if matches!(
                    decl.kind,
                    MicroKind::Class | MicroKind::Mixin | MicroKind::Enum
                ) {
                    return self
                        .creation_row_ty(row.name, false, row.range.offset)
                        .unwrap_or(if decl.type_params == 0 {
                            Ty::named(row.name)
                        } else {
                            Ty::UNKNOWN
                        });
                }
                self.instantiate_call_return(row, &Receiver::Unknown, decl.return_ty)
            });
        }
        // A prefixed top-level callee (`path.basename(p)`): the prefix's
        // import table resolves the declaration directly — there is no
        // container to walk, and the below-row fallback would name the
        // prefix itself as the head. The declared return types the send
        // exactly like the unqualified top-level arm (through the
        // projected declaration, so a generic callee's free parameter
        // collapses to the unknown face rather than leaking its raw
        // spelling); an unresolved prefix hit keeps the unknown face
        // (its own miss row reports).
        if let RowShape::Member {
            recv: Receiver::Prefix { name: prefix },
        } = &row.shape
            && let dartq_resolution::Lookup::Found(NamespaceEntry::Decl(file)) = self
                .namespaces
                .lookup_prefixed(self.graph, self.lib, *prefix, row.name)
            && let Some(decl) = self.index.get(file).and_then(|f| f.decl(row.name))
        {
            let decl = self.boundary.top(file, row.name, decl);
            return self.instantiate_call_return(
                row,
                &Receiver::Prefix { name: *prefix },
                decl.return_ty,
            );
        }
        let container = match &row.shape {
            RowShape::Member {
                recv:
                    Receiver::This { container, .. }
                    | Receiver::Super { container, .. }
                    | Receiver::Container {
                        name: container, ..
                    }
                    | Receiver::Typed {
                        type_name: container,
                        ..
                    },
            } => Some(*container),
            _ => None,
        };
        // The receiver row beneath the member row names the class-static
        // head when the dot merge left no receiver shape (the SDK head of
        // `RangeError.checkValidRange(...)` resolves through the synthetic
        // face file, which arms no `BareInfo::Container`, so the merged
        // row keeps `Receiver::Unknown`): the push-ordered row before the
        // member row is the receiver's own, and its spelling keys the
        // unindexed-container dispatch below. A fully-contained receiver
        // row (`a.b.c`'s `.b`) yields no table hit and under-reports as
        // before.
        let container = container.or_else(|| {
            let index = self
                .rows
                .partition_point(|probe| probe.range.offset < row.range.offset);
            let receiver = self.rows.get(index.checked_sub(1)?)?;
            (receiver.range.end() <= row.range.offset).then(|| {
                // An UNQUALIFIED implicit-this receiver (`_reader
                // .readEnum(...)`, the callee row still the pre-merge
                // selector at this send-close read): the bare scope
                // resolves the receiver name to the enclosing
                // container's member, whose WRITTEN declared type heads
                // the container the member walk resolves on — and only
                // when the walk finds the callee there (a foreign or
                // stub head walks nowhere). Locals resolve to non-member
                // targets and qualified receivers
                // (`_sourceTable.$primaryKey`, a chain-computed face)
                // keep the bare-name path (the SDK static head,
                // `RangeError.checkValidRange`).
                if !receiver.qualified {
                    if let (_, RefTarget::Member { container }) = self.resolve_bare(receiver.name) {
                        if let Ty::Named { name, .. } =
                            self.member_declared_ty(container, receiver.name)
                            && matches!(self.walk_members(name, row.name), Walk::Found(_))
                        {
                            return name;
                        }
                    }
                }
                receiver.name
            })
        });
        let Some(container) = container else {
            return Ty::UNKNOWN;
        };

        // `C.name(...)` on a class head: a named constructor outranks any
        // member (the oracle rewrites the creation first), and a static
        // member must be the class's own — statics are not inherited, so
        // the superclass walk never answers a static send.
        if let RowShape::Member {
            recv: Receiver::Container { .. },
        } = &row.shape
            && let Some((found, owner)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                container,
            )
        {
            // A GENERIC class's constructor result is the instantiated
            // head (`List.of(ranges)` answers `List<RangeCharPredicate>`
            // from the arguments), never the raw one: the raw `List` would
            // poison every downstream schema. This name-level read cannot
            // run the call-site inference itself, so the generic arm
            // answers the face the creation machinery PUBLISHED for this
            // class segment — a written `<...>` group (`G<int>.value(1)`)
            // or a solved fold (`creation_row_ty`'s latch, keyed on the
            // class head's offset). A creation the machinery published
            // nothing for (the argument-solved spelling `G.value(1)`, whose
            // solve the send close keeps to itself) stays `Unknown`: a raw
            // head or a to-bounds fill would stand in for an instantiation
            // nobody decided and stage rows the oracle never reports, and
            // the unknown under-report is the only sound stand-in (#3412).
            if found.constructors.contains(&row.name) {
                if found.type_params == 0 {
                    return Ty::named(container);
                }
                return self
                    .creation_class_offset(row.range, container)
                    .and_then(|head| self.creation_row_ty(container, false, head))
                    .filter(|ty| *ty != Ty::UNKNOWN)
                    .unwrap_or(Ty::UNKNOWN);
            }
            return match found.members.get(&row.name) {
                Some(entry) if entry.is_static => {
                    // The entry belongs to `owner`'s pool: project it before
                    // its types flow (the same F1 rule the walk's Find arm
                    // applies).
                    let recv = Receiver::Container {
                        name: container,
                        kind: found.kind,
                    };
                    let projected = self.boundary.member(owner, container, row.name, entry);
                    self.instantiate_call_return(row, &recv, projected.return_ty)
                }
                _ => Ty::UNKNOWN,
            };
        }
        match self.walk_members(container, row.name) {
            Walk::Found(entry) if matches!(entry.kind, MemberKind::Method | MemberKind::Getter) => {
                let recv = match &row.shape {
                    RowShape::Member { recv } => *recv,
                    // The pre-merge callee selector row (a dot-spelled
                    // send read at its own close): the container the
                    // below-row fallback resolved stands in as the
                    // receiver, so the return instantiation reads the
                    // method's own parameters and the send-close
                    // solution (`_reader.readEnum(Tag.values)`'s `T`).
                    _ => Receiver::This {
                        container,
                        kind: self
                            .container_kind_of(container)
                            .unwrap_or(ContainerKind::Class),
                    },
                };
                self.instantiate_call_return(row, &recv, entry.return_ty)
            }
            // The unindexed dart:core container's static face
            // ([`CORE_STATIC_RETURNS`]): the member walk chain-breaks on
            // every SDK unit, so the declared return rides the table
            // instead of the walk (`RangeError.checkValidRange(...)`,
            // #2172). An indexed container keeps the miss face — a
            // user-declared same-spelled container never borrows the SDK
            // table.
            _ => {
                // The universal `Object` face first: `toString`/
                // `hashCode`/`runtimeType` exist on every instance
                // receiver whatever its head, so a walk that bottoms out
                // without the spelling — an unindexed primitive's chain,
                // a function-typed alias, an indexed user class that
                // simply does not redeclare it — still resolves through
                // `Object` and the send types its declared return. The
                // static class-head face (a `Container` receiver) keeps
                // the miss: statics are not inherited, and `Object`
                // declares no statics under these spellings.
                if let RowShape::Member { recv } = &row.shape
                    && matches!(
                        recv,
                        Receiver::Typed { .. }
                            | Receiver::This { .. }
                            | Receiver::Super { .. }
                            | Receiver::Created { .. }
                    )
                    && let Some((_, head)) = OBJECT_INSTANCE_RETURNS
                        .iter()
                        .find(|(member, _)| *member == &self.strings.resolve(row.name)[..])
                {
                    return Ty::named(self.intern(head));
                }
                // The same face over the classifier's non-named dispatch: a
                // local receiver the fold types as a function, record, or
                // `dynamic` (`callback.toString()` on a typedef'd
                // parameter, `d.toString()` on a dynamic one) merges as
                // `Receiver::Unknown` — the payload names no container —
                // yet the official resolver reaches `Object` there exactly
                // the same way (probed 3.13.1: `s = d.toString()` promotes
                // `s`, so the send types `String`, not `dynamic`). The
                // merge's side table carries the receiver's static type —
                // the row itself folded away — and only a typed entry arms
                // the fallback; an unannotated receiver keeps the unknown
                // face (the under-report direction).
                if let RowShape::Member {
                    recv: Receiver::Unknown,
                } = &row.shape
                    && let Some((_, head)) = OBJECT_INSTANCE_RETURNS
                        .iter()
                        .find(|(member, _)| *member == &self.strings.resolve(row.name)[..])
                    && let Some(declared) = self.member_recv_tys.get(&row.range.offset)
                    && !matches!(
                        self.fold_alias_ty(*declared),
                        Ty::Unknown | Ty::Void | Ty::Never
                    )
                {
                    return Ty::named(self.intern(head));
                }
                let indexed = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    container,
                )
                .is_some();
                if indexed {
                    return Ty::UNKNOWN;
                }
                CORE_STATIC_RETURNS
                    .iter()
                    .find(|(recv, member, _)| {
                        *member == &self.strings.resolve(row.name)[..]
                            && self.intern(recv) == container
                    })
                    .map_or(Ty::UNKNOWN, |(_, _, head)| Ty::named(self.intern(*head)))
            }
        }
    }

    /// The class segment a named-constructor send's selector sits behind
    /// (`G<int>.value(` reads `G`'s offset): the identifier left of the
    /// selector's `.`, across a balanced `<...>` group when one is spelled.
    /// `None` when no dot precedes the selector, or when the walked
    /// spelling names another container — the send is then no creation of
    /// `container`, and a same-spelled instance member's receiver face
    /// keeps its own type.
    ///
    /// The creation latches key on the class head's own offset (the
    /// written group's `call_type_args` entry, `publish_creation_
    /// instantiation`'s solved fold), so this walk is what turns a merged
    /// member row back into its creation's lookup key. The scan mirrors the
    /// creation machinery's own class-segment reader; it lives here because
    /// [`Self::callee_return_ty`]'s generic-constructor arm is its only
    /// reader outside that module.
    fn creation_class_offset(&self, selector: SourceRange, container: StringId) -> Option<u32> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(selector.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor == 0 || bytes[cursor - 1] != b'.' {
            return None;
        }
        cursor -= 1;
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        // A written instantiation between the class and the selector
        // (`G<int>.value(`): the head sits before the balanced group.
        if cursor > 0 && bytes[cursor - 1] == b'>' {
            let mut depth = 0usize;
            while cursor > 0 {
                let byte = bytes[cursor - 1];
                cursor -= 1;
                if byte == b'>' {
                    depth += 1;
                } else if byte == b'<' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            if depth != 0 {
                return None;
            }
            while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
                cursor -= 1;
            }
        }
        let end = cursor;
        while cursor > 0
            && (bytes[cursor - 1].is_ascii_alphanumeric()
                || bytes[cursor - 1] == b'_'
                || bytes[cursor - 1] == b'$')
        {
            cursor -= 1;
        }
        if cursor == end || self.source.get(cursor..end)? != self.strings.resolve(container) {
            return None;
        }
        Some(self.utf16_of(cursor))
    }

    /// Stages the never-receiver junction (`receiverOfTypeNever`,
    /// messages.yaml :29566). The staged check re-decides at the teardown
    /// join, where a promoted-read fact on the receiver's exact span
    /// replaces the type — the official read's flow-aware staticType.
    ///
    /// Two gates reproduce the official decision order at its five entry
    /// points (`property_element_resolver.dart:189` index,
    /// `method_invocation_resolver.dart:195` send,
    /// `function_expression_invocation_resolver.dart:81` function position,
    /// `binary_expression_resolver.dart:454` operator, and the
    /// prefix/postfix increment faces `prefix_expression_resolver.dart:183`
    /// / `postfix_expression_resolver.dart:134`):
    ///
    /// * **Face** — the Never check exists only where upstream's resolver
    ///   consults `identical(type, NeverTypeImpl.instance)`: a plain
    ///   property read or write (`x.foo`, `x.foo = v`, `x.hashCode`) walks
    ///   `_resolve`'s member path, which carries no Never arm — and the
    ///   `!` operand routes through `_resolveNegation` the same way. Both
    ///   stay silent upstream (the dead-code row reports instead), so the
    ///   staging skips them; every other spelling (member invocation,
    ///   index, operator, increment, function position) is a fire face.
    /// * **Type** — the receiver's parse-time `Never` is the ordinary arm;
    ///   a promotable local read (`local_hits`) stages too, so the join can
    ///   substitute the flow's promoted `Never` — a local read's
    ///   `typeOrThrow` answers the promoted static type (an inferred-`Never`
    ///   `var x = throw` read, or a `Never` the staged `promotions` join
    ///   carries).
    pub(crate) fn stage_receiver_of_type_never(&mut self, at: SourceRange, receiver: Ty) {
        if !(self.is_never_ty(receiver) || self.promotable_read_at(at)) {
            return;
        }
        if !self.never_receiver_face(at) {
            return;
        }
        self.staged_checks.push(StagedCheck {
            kind: StagedKind::NeverReceiver,
            range: at,
            from: receiver,
            to: Ty::UNKNOWN,
            literal: false,
        });
    }

    /// Whether the receiver span is a promotable read the flow layer can
    /// carry a `Never` promotion on — a bare local (or parameter) read,
    /// `local_hits`-keyed. A parenthesized receiver's inner read (`(x).m()`)
    /// hits inside the group's span. Only a local read can promote to
    /// `Never` upstream (`PromotedPropertyRead` promotes to non-null only,
    /// and `this` never promotes), so member/`this`/top-level receivers
    /// need no staging for the flow arm.
    fn promotable_read_at(&self, at: SourceRange) -> bool {
        self.local_hits.contains_key(&at.offset)
            || self
                .local_hits
                .keys()
                .any(|offset| at.offset < *offset && *offset < at.end())
    }

    /// The face gate of [`Self::stage_receiver_of_type_never`]: reads the
    /// source past the receiver span. A member-access continuation
    /// (`recv.name` / `recv?.name` / `recv..name`) fires only when the
    /// member is INVOKED (or carried through a type-argument group into a
    /// call) — upstream's property `_resolve` reports nothing on a `Never`
    /// receiver, its method/index/send resolvers do. Everything else
    /// (operator operands, function-position `x(...)`, index `x[...]`,
    /// prefix/postfix increments) is a fire face except the bare `!x`
    /// operand, whose `_resolveNegation` path reports `non_bool_negation`,
    /// never `receiver_of_type_never`.
    fn never_receiver_face(&self, at: SourceRange) -> bool {
        let bytes = self.source.as_bytes();
        let mut i = self.byte_of(at.end());
        // Member-access continuation: `.` or `?.` (plus the cascade `..`),
        // whitespace-tolerant (`x\n.foo()` merges identically).
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let dot = match bytes.get(i) {
            Some(b'.') => {
                i += 1;
                true
            }
            Some(b'?') if bytes.get(i + 1) == Some(&b'.') => {
                i += 2;
                true
            }
            _ => false,
        };
        if dot {
            if bytes.get(i) == Some(&b'.') {
                i += 1; // cascade section `..`
            }
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
            {
                i += 1;
            }
            // A generic send's type-argument group (`x.foo<T>(…)`) stands
            // between the member and its `(` — the `<` is a generic group
            // only when a `>(` closes it; `x.foo < y` is the operator face
            // (upstream's own `<`-lookahead rule decides identically).
            if bytes.get(i) == Some(&b'<') {
                let mut depth = 0u32;
                let mut j = i;
                return loop {
                    match bytes.get(j) {
                        Some(b'<') => depth += 1,
                        Some(b'>') => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break bytes.get(j + 1) == Some(&b'(');
                            }
                        }
                        // A `(`/`;`/terminator inside the group means the
                        // `<` was the binary operator — the member resolves
                        // as a getter access upstream (silent), not a send.
                        Some(b'(' | b')' | b';' | b'{' | b'}' | b'[' | b']') | None => {
                            break false;
                        }
                        _ => {}
                    }
                    j += 1;
                };
            }
            return bytes.get(i) == Some(&b'(');
        }
        // The logical operators: `&&`/`||` resolve through
        // `_resolveLogicalAnd`/`_resolveLogicalOr`, which carry no Never
        // arm — the receiver read's flow state feeds the branch, not a
        // member consult. (`&`/`|` single are user-definable operators and
        // DO fire; only the doubled spellings are silent.)
        if (bytes.get(i) == Some(&b'&') && bytes.get(i + 1) == Some(&b'&'))
            || (bytes.get(i) == Some(&b'|') && bytes.get(i + 1) == Some(&b'|'))
        {
            return false;
        }
        // Not a member access: every remaining junction is a fire face
        // (binary operators, prefix `-`/`~`/`++`/`--`, postfix `++`/`--`,
        // function position, index) except the bare `!x` operand —
        // distinguishable only by the `!` abutting the receiver's front.
        let mut j = self.byte_of(at.offset);
        while j > 0 && bytes[j - 1].is_ascii_whitespace() {
            j -= 1;
        }
        bytes.get(j.wrapping_sub(1)) != Some(&b'!')
    }

    /// Arms the pending type head: the identifier just parsed a type
    /// annotation and may be followed by a type-argument group.
    pub(crate) fn arm_type_head(&mut self, name: StringId, range: SourceRange) {
        self.pending_type_head = Some((name, range));
    }

    /// The expression type of an already-pushed row shaped as a receiver or
    /// value (`a?[i]`'s receiver check).
    pub(crate) fn receiver_row_ty(row: &Row) -> Ty {
        match &row.shape {
            RowShape::Bare(info) => match info {
                BareInfo::Plain { ty } => *ty,
                BareInfo::Container { name, .. } => Ty::named(*name),
                _ => Ty::UNKNOWN,
            },
            RowShape::Receiver(recv) => match recv {
                Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                    Ty::named(*container)
                }
                Receiver::Typed { ty, .. } => *ty,
                _ => Ty::UNKNOWN,
            },
            _ => Ty::UNKNOWN,
        }
    }

    /// Marks the assignment target: the closest row wholly before the
    /// operator, skipping folded value rows (index results, groups). Bare and
    /// member rows upgrade their intent; index-operator rows record the write
    /// form (`c[i] = v` writes through `[]=`, never the index argument).
    pub(crate) fn mark_assignment(&mut self, operator: Token) {
        let Some(op_text) = self.text_of(operator) else {
            return;
        };
        let limit = operator.range.offset;
        let compound = op_text != "=";
        let index_op = self.intern("[]");
        // The constructor-initializer world the assignment may join,
        // resolved before the row walk borrows the stack.
        let init_container = if self.initializer_depth > 0 {
            self.container_context().map(|(c, _)| c)
        } else {
            None
        };
        let init_ctor_site = if self.initializer_depth > 0 {
            self.open_ctor_site
        } else {
            None
        };
        // The candidate target row: the closest row wholly before the
        // operator, skipping folded value rows (index results, groups) —
        // the shared `assignment_target_index` scan.
        let Some(index) = self.assignment_target_index(limit) else {
            return;
        };
        // An unfolded index target keeps its index arguments as plain read
        // rows ABOVE the receiver's folded value stand-in (`t[0][c] = v`'s
        // receiver `t[0]` folds — its result is a value stand-in just below
        // `c` — so the outer bracket never folds and `c`'s row stays on the
        // stack). The official walk visits an index target's subexpressions
        // normally (`_visitIndexExpression`: only the target's own element
        // skips the read), so `c` is a use: the candidate yields to the
        // index-operator row carrying the stand-in's bracket and its read
        // credit stands. The guard needs the stand-in's own `[` right
        // before the candidate — a leftover value row from an earlier
        // statement is separated by that statement's trailing tokens, never
        // a bare bracket.
        let index = if matches!(self.rows[index].intent, Intent::Read) && index > 0 && {
            let below = &self.rows[index - 1];
            matches!(below.shape, RowShape::Value)
                && self.is_open_bracket_between(below.range.end(), self.rows[index].range.offset)
        } {
            let stand_in = &self.rows[index - 1];
            let (in_offset, in_end) = (stand_in.range.offset, stand_in.range.end());
            // The receiver chain's row carries the bracket span its fold
            // produced — the row whose `[]` bracket contains the stand-in
            // is the write's true target.
            match self.rows[..index - 1]
                .iter()
                .rposition(|row| match &row.intent {
                    Intent::Operator(op, bracket) => {
                        *op == index_op && bracket.offset <= in_offset && in_end <= bracket.end()
                    }
                    _ => false,
                }) {
                Some(head) => head,
                // A receiver whose own resolution left no operator row (a
                // call result): nothing to convert — the index argument's
                // read credit stands either way.
                None => return,
            }
        } else {
            index
        };
        let row = &mut self.rows[index];
        match &row.intent {
            Intent::Read => {
                // A write alone never uses its target local (the
                // official `_useAssignmentTargetElement` early return)
                // — and only a bare row IS that local. A member row's
                // hit is the RECEIVER's occurrence credit: `v.m = 1`
                // reads `v`, and the official `_visitNameExpression`
                // spares it through the lexical resolution regardless
                // of the member access resolving (#2189-b — the
                // receiver-write census rows).
                if !compound
                    && matches!(row.shape, RowShape::Bare(_))
                    && let Some(hit) = self.local_hits.remove(&row.range.offset)
                {
                    self.locals[hit as usize].uses -= 1;
                }
                row.intent = if compound {
                    Intent::ReadWrite
                } else {
                    Intent::Write
                };
                // FAM18: a constructor initializer list's plain
                // assignment marks the field initialized
                // (`updateWithInitializers`,
                // constructor_fields_verifier.dart) — the container's
                // flush consults the set. Only the this-face spellings
                // count: the bare LHS, the still-unmerged `this` row,
                // and the merged `this.b` member (`ConstructorField-
                // Initializer`'s `this.b` prefix resolves the same
                // field; a `super.b` write initializes the superclass's
                // field, never ours), and only the plain `=` (the
                // official face is a `ConstructorFieldInitializer`
                // node).
                if !compound
                    && let Some(site) = init_ctor_site
                    && init_container.is_some()
                    && matches!(
                        row.shape,
                        RowShape::Bare(_)
                            | RowShape::Receiver(Receiver::This { .. })
                            | RowShape::Member {
                                recv: Receiver::This { .. }
                            }
                    )
                {
                    self.ctor_init_writes.insert((site, row.name));
                }
            }
            Intent::Operator(op, _) if *op == index_op => {
                row.write_op = Some(if compound {
                    OpWrite::Compound
                } else {
                    OpWrite::Assign
                });
            }
            _ => {}
        }
    }

    /// Whether the source between two UTF-16 offsets is exactly the `[`
    /// token (whitespace carries no token): the unfolded-index signature
    /// the [`Engine::mark_assignment`] index-argument guard reads.
    fn is_open_bracket_between(&self, from: u32, to: u32) -> bool {
        if from >= to {
            return false;
        }
        let tokens = self.tokens.as_slice();
        let lo = tokens.partition_point(|token| token.utf16_offset < from);
        let mut text = String::new();
        for token in &tokens[lo..] {
            if token.utf16_offset >= to {
                break;
            }
            text.push_str(&self.source[token.byte_range()]);
        }
        text == "["
    }

    /// Marks an operator operand: the closest row wholly before `op`, when it
    /// is a bare identifier, a typed receiver, a member access, or an already
    /// folded value (`a + b`, `this + b`, `a.b + c`, `a + b + c` — the inner
    /// fold's [`RowShape::Value`] result row is the outer operator's left
    /// operand).
    pub(crate) fn mark_operator(&mut self, op: StringId, range: SourceRange) {
        for row in self.rows.iter_mut().rev() {
            if row.range.end() > range.offset {
                continue;
            }
            if matches!(
                row.shape,
                RowShape::Bare(_)
                    | RowShape::Receiver(_)
                    | RowShape::Member { .. }
                    | RowShape::Record(_)
                    | RowShape::Value
            ) && row.intent == Intent::Read
            {
                row.intent = Intent::Operator(op, range);
            }
            return;
        }
    }

    /// The top row index when it can be an invocation callee.
    pub(crate) fn callee_index(&self) -> Option<usize> {
        let index = self.rows.len().checked_sub(1)?;
        matches!(
            self.rows[index].shape,
            RowShape::Bare(_) | RowShape::Member { .. }
        )
        .then_some(index)
    }

    /// The declared type of the assignment target row (`x = e`, `obj.f = e`):
    /// a bare name reads its snapshot, a member walk answers the write face —
    /// the field's declared type or the setter's value formal
    /// (`writeAcceptedType`); executables and unknown receivers stay
    /// `Unknown`.
    ///
    /// A bare read's push-time snapshot is the GETTER's face when the name
    /// names an accessor pair or an implicit-this member, never the write
    /// type the oracle checks (`AssignmentExpressionImpl.writeType` is
    /// `writeElement.type` — the `name=` setter's value formal, or the
    /// field/variable's declared type). So the bare arm re-derives the write
    /// face from the name's resolution ([`Self::member_write_ty`] for an
    /// implicit-this member, [`Self::top_level_write_ty`] for a library
    /// name): a local binding keeps its snapshot (the flow layer owns the
    /// local's read face), and every other face answers the write target the
    /// setter/field walk gives.
    pub(crate) fn assignment_target_ty(&mut self, row: &Row) -> Ty {
        match &row.shape {
            RowShape::Bare(BareInfo::Plain { ty }) => {
                let snapshot = *ty;
                match self.resolve_bare(row.name).1 {
                    // A local binding (and every face that is no write
                    // target at all) keeps its push-time capture: the local's
                    // declared type IS its write type, and the other faces
                    // report through their own codes (`undefined_setter`,
                    // `assignment_to_type`).
                    RefTarget::Local
                    | RefTarget::TypeParam
                    | RefTarget::Prefix
                    | RefTarget::This
                    | RefTarget::Opaque
                    | RefTarget::Missing => snapshot,
                    // An implicit-this member read (`b ??= e` inside the
                    // class that declares the pair): the write resolves
                    // through `name=`/the field, exactly like `this.b = e`.
                    RefTarget::Member { container } => self.member_write_ty(container, row.name),
                    // A top-level name: the setter's value formal first (the
                    // `void set b(B? value)` write face), else the variable's
                    // own declared type; a getter-only name keeps the empty
                    // face (upstream's `InvalidType` write type accepts
                    // everything, so the assignment check stays silent and
                    // `undefined_setter` owns the report).
                    RefTarget::Decl { .. } => self.top_level_write_ty(row.name),
                }
            }
            RowShape::Bare(BareInfo::Missing { this_type, .. })
                if row.intent == Intent::Write =>
            {
                // A setter-only implicit-`this` write (`set only(T v)` with
                // no getter): the tentative read missed the getter
                // namespace, so the row carries no `Plain` face, yet the
                // write element the oracle resolves is the `name=` setter
                // and the checked type is its value formal
                // (`AssignmentExpressionImpl.writeType`,
                // assignment_expression_resolver.dart:54/:83/:361). The
                // row-resolution face owns both halves — the same implicit-
                // `this` `name=` hit [`Self::bare_write_target_exists`]
                // rescues from `undefined_identifier` decides the face here
                // ([`Self::bare_write_formal_ty`]), and only a plain `=`
                // asks (a compound `??=`/`+=` reads the name too, whose
                // miss keeps its own report).
                self.bare_write_formal_ty(row.name, *this_type)
            }
            RowShape::Bare(BareInfo::Container { name, kind }) => {
                // A container name is never a valid assignment target — the
                // oracle reports assignment-to-`this`-prefix shapes with
                // other codes; suppress here.
                let _ = (name, kind);
                Ty::UNKNOWN
            }
            RowShape::Member { recv } => match recv {
                Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                    self.member_write_ty(*container, row.name)
                }
                // A folded creation receiver (`C().x = e`, `C<T>().x = e`):
                // the instance write face is the declaring container's —
                // the setter's value formal or the field's declared type
                // (upstream `getWriteType`, the same answer
                // [`Engine::member_write_ty`] gives). The receiver's written
                // arguments instantiate the member face exactly like the
                // `Receiver::Typed` arm below: `Box<B>().slot` writes against
                // `B`, never the container's bare `T`. The staged
                // member-read record keys the receiver's span (the creation
                // head's offset), and [`Engine::creation_row_ty`] re-derives
                // the written `<B>` from it; a raw `Box()` spells no group,
                // so the instantiate-to-bounds residual
                // ([`Engine::instantiated_head_ty`]) answers `Box<bounds>`
                // the oracle's `staticType` reports.
                Receiver::Created { name, .. } => {
                    let recv_ty = self
                        .member_reads
                        .binary_search_by(|read| read.offset.cmp(&row.range.offset))
                        .ok()
                        .map(|index| self.member_reads[index].recv_span)
                        .and_then(|span| self.creation_row_ty(*name, false, span.offset))
                        .unwrap_or_else(|| self.instantiated_head_ty(Ty::named(*name)));
                    self.instantiated_member_write_ty(*name, row.name, recv_ty)
                }
                Receiver::Typed { type_name, ty } => {
                    self.instantiated_member_write_ty(*type_name, row.name, *ty)
                }
                Receiver::Container {
                    name: container, ..
                } => {
                    // F4: the resolved container's write face, projected
                    // when the container is foreign — a static field's
                    // declared type or the static setter's value formal.
                    let setter = self.setter_spelling(row.name);
                    resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        *container,
                    )
                    .and_then(|(c, owner)| {
                        c.members
                            .get(&row.name)
                            .filter(|e| e.kind == MemberKind::Field)
                            .map(|e| (owner, e.var_ty))
                            .or_else(|| {
                                let formal_ty = c
                                    .members
                                    .get(&setter)
                                    .filter(|e| e.kind == MemberKind::Setter)
                                    .and_then(|e| e.formals.as_deref())
                                    .and_then(|f| f.positional.first())
                                    .map(|formal| formal.ty)?;
                                Some((owner, formal_ty))
                            })
                    })
                    .map_or(Ty::UNKNOWN, |(owner, ty)| self.boundary.ty(owner, ty))
                }
                _ => Ty::UNKNOWN,
            },
            _ => Ty::UNKNOWN,
        }
    }

    /// The member write face of `member` on `container`, instantiated by the
    /// receiver's own static type `recv_ty` (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/generated/error_detection_helpers.dart:72-75 —
    /// the verdict asks `correspondingParameter.type`, the
    /// already-instantiated formal): a generic receiver's declared member
    /// face spells the container's `T`, but the oracle's write type
    /// substitutes the receiver's written arguments — `C<String>`'s
    /// `T? field` writes against `String?`, never `T?`. The environment
    /// composes the same way the member send's does
    /// ([`Engine::member_recv_env`]): the field, else the `name=` setter the
    /// write resolves against, supplies the entry the parameter-name
    /// pairing needs. A receiver with no written arguments (a bare head, or
    /// `recv_ty` carrying `ArgListId::EMPTY`) keeps the declared face —
    /// [`Engine::member_recv_env`] answers `None` and the result is the
    /// `declared` answer unchanged.
    fn instantiated_member_write_ty(
        &mut self,
        container: StringId,
        member_name: StringId,
        recv_ty: Ty,
    ) -> Ty {
        let declared = self.member_write_ty(container, member_name);
        let setter = self.setter_spelling(member_name);
        let entry = match self.walk_members(container, member_name) {
            Walk::Found(entry) if entry.kind == MemberKind::Field => Some(entry),
            _ => match self.walk_members(container, setter) {
                Walk::Found(entry) if entry.kind == MemberKind::Setter => Some(entry),
                _ => None,
            },
        };
        entry
            .and_then(|entry| self.member_recv_env(recv_ty, container, &entry))
            .map(|env| {
                let keys: FxHashMap<StringId, Ty> = env.into_iter().collect();
                self.substitute_ty(declared, &keys)
            })
            .unwrap_or(declared)
    }

    /// A top-level name's write face (`getWriteType` over the library's
    /// declaration table): the `name=` setter's single value formal when the
    /// pair declares one (the `void set b(B? value)` arm — `b ??= e` and
    /// `b = e` both check against `B?`), else a top-level variable's declared
    /// type (the inferred overlay included). Getters, functions, and type
    /// aliases answer nothing: their assignment reports through
    /// `undefined_setter`/`assignment_to_function`, never the assignability
    /// face (upstream's write element is `InvalidType` there, which accepts
    /// every source).
    ///
    /// Strays stay under-report: a name the scoped lookup missed, a broken
    /// chain, and an untyped declaration all answer `Unknown`.
    fn top_level_write_ty(&mut self, name: StringId) -> Ty {
        let setter = self.setter_spelling(name);
        if let Some(decl) = self.top_level_decl(setter)
            && decl.kind == MicroKind::TopLevelFn
            && let Some(formal) = decl
                .formals
                .as_deref()
                .and_then(|formals| formals.positional.first())
        {
            let ty = formal.ty;
            return self.fold_alias_ty(ty);
        }
        match self.top_level_decl(name) {
            Some(decl) if decl.kind == MicroKind::TopLevelVar => {
                let model = decl.var_ty;
                self.fold_alias_ty(self.top_var_model_ty(name, model))
            }
            _ => Ty::UNKNOWN,
        }
    }

    /// Whether `ty` is an invalid written type (upstream `InvalidType`):
    /// a named head that resolves to no type, or a written argument tree
    /// that carries one — the oracle's named-type walk poisons the whole
    /// spelling (`named_type_resolver.dart` `_resolveToElement` plus the
    /// argument recursion). An invalid side asks no assignability question
    /// (`isAssignableTo` answers true the moment either side is invalid),
    /// so every assignability face gates before its matrix.
    pub(crate) fn written_ty_invalid(&self, ty: Ty) -> bool {
        let Ty::Named { name, args, .. } = ty else {
            return false;
        };
        if !self.named_type_is_type(name) {
            return true;
        }
        self.boundary
            .pool()
            .get(args)
            .iter()
            .any(|arg| matches!(arg, Ty::Named { .. }) && self.written_ty_invalid(*arg))
    }

    /// The whole-expression span of a staged assignability check (the
    /// oracle's `.at(right)` / `.at(expression)` anchor — upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/resolver/
    /// assignment_expression_resolver.dart:167-178, return_type_verifier.dart
    /// `:150-206`): the staged row's range covers the operand's own row —
    /// a callee, a bare read, a merged member — while the official node is
    /// the full `Expression` (an `await e`, a `new C()`, a `x as T`, a
    /// collection literal's brackets, a record literal's parens, a unary
    /// or parenthesized prefix, a dot-shorthand's leading `.`).
    ///
    /// The base is the widest recorded sub-span (invocation, then property
    /// access, then the row itself); the left walk swallows the
    /// expression-forming prefix tokens (`await`/`new`/`const`/`throw`,
    /// unary `-`/`!`/`~`/`++`/`--`, a shorthand `.`, an enclosing `(`);
    /// the right walk swallows the suffix forms (postfix `!`, `is`/`as`
    /// type-test continuations through the tested type's end, a closing
    /// `[]`/`{}`/`>` after a type-argument head) and a degenerate
    /// single-byte closer row re-anchors through its matching opener.
    /// Every arm is token-table driven; an unrecognized neighbor leaves
    /// the span untouched — the row's own anchor is the fallback, never a
    /// widened guess.
    pub(crate) fn staged_expression_span(&self, range: SourceRange) -> SourceRange {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let mut span = self
            .invocation_span(range)
            .or_else(|| self.property_access_span(range))
            .unwrap_or(range);
        let toks = self.tokens.as_slice();
        if toks.is_empty() {
            return span;
        }
        // Whether a token can END an expression — the prefix walk's guard:
        // a `-`/`.`/`(` neighbor after such a token is an infix/member/call
        // arm, never an expression prefix of the staged operand.
        let ends_expression = |kind: TK| -> bool {
            matches!(
                kind,
                TK::Identifier
                    | TK::Int
                    | TK::Double
                    | TK::Hexadecimal
                    | TK::String
                    | TK::CloseParen
                    | TK::CloseSquare
                    | TK::CloseCurly
                    | TK::Gt
                    | TK::Bang
                    | TK::PlusPlus
                    | TK::MinusMinus
                    | TK::Index
                    | TK::Keyword(Kw::This | Kw::Super | Kw::True | Kw::False | Kw::Null)
            )
        };
        // A degenerate row re-anchors through the group it closes: a
        // single-byte `)`/`]`/`}` range (the record/set fold's merged tail
        // — `(a: 1, b: 2)`'s row sits ON the `)`) spans the whole group,
        // the same `.at(expression)` answer the oracle's RecordLiteral /
        // collection node gives. The balance walk finds the opener the
        // close pairs with.
        if span.length == 1 {
            let close_idx = toks.partition_point(|t| t.utf16_offset < span.offset);
            if let Some(&close) = toks.get(close_idx)
                && close.utf16_offset == span.offset
                && matches!(
                    close.kind,
                    TK::CloseParen | TK::CloseSquare | TK::CloseCurly
                )
            {
                let mut depth = 0u32;
                for index in (0..=close_idx).rev() {
                    match toks[index].kind {
                        kind if kind == close.kind => depth += 1,
                        TK::OpenParen if close.kind == TK::CloseParen => {
                            depth -= 1;
                            if depth == 0 {
                                span = SourceRange {
                                    offset: toks[index].utf16_offset,
                                    length: close.utf16_offset + close.byte_len
                                        - toks[index].utf16_offset,
                                };
                                break;
                            }
                        }
                        TK::OpenSquare if close.kind == TK::CloseSquare => {
                            depth -= 1;
                            if depth == 0 {
                                span = SourceRange {
                                    offset: toks[index].utf16_offset,
                                    length: close.utf16_offset + close.byte_len
                                        - toks[index].utf16_offset,
                                };
                                break;
                            }
                        }
                        kind if close.kind == TK::CloseCurly
                            && matches!(
                                kind,
                                TK::OpenCurly | TK::StringInterpolationExpression
                            ) =>
                        {
                            depth -= 1;
                            if depth == 0 {
                                span = SourceRange {
                                    offset: toks[index].utf16_offset,
                                    length: close.utf16_offset + close.byte_len
                                        - toks[index].utf16_offset,
                                };
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        // The suffix walk: `cast_rewrites` already carries every `as`-cast's
        // whole span (the expression end the type-checker recorded), so a
        // staged row inside one takes the cast's own extent outright. The
        // token walk then covers what the rewrite table does not: postfix
        // `!`, `is`/`as` type-test continuations through the tested type's
        // end, and a literal bracket group after a type-argument head.
        if let Some((cast_span, _)) = self
            .cast_rewrites
            .iter()
            .find(|(cast, _)| cast.offset <= range.offset && range.end() <= cast.end())
        {
            if span.end() <= cast_span.end() {
                span = SourceRange {
                    offset: span.offset.min(cast_span.offset),
                    length: cast_span.end() - span.offset.min(cast_span.offset),
                };
            }
        }
        loop {
            let at = toks.partition_point(|t| t.utf16_offset < span.end());
            let Some(&next) = toks.get(at) else { break };
            match next.kind {
                // `x as T` / `x is T` / `x is! T`: the tested type runs to
                // the depth-0 statement boundary — a `,` `;` `:` `=` `=>`
                // `&&` `||` `?` (the conditional's own `?` — a `?` INSIDE
                // the type is consumed, one at the boundary is not), a
                // closing bracket at the current depth, or EOF. Begin
                // groups jump whole through the precomputed pairing, so
                // `List<int>`, `void Function(int)`, and `Map<K, V>` never
                // stop the walk early.
                TK::Keyword(Kw::As) | TK::Keyword(Kw::Is) => {
                    let mut end = at + 1;
                    if toks.get(end).is_some_and(|t| t.kind == TK::Bang) {
                        end += 1;
                    }
                    let mut last = end;
                    while let Some(t) = toks.get(end) {
                        if let Some(close) = self.tokens.matching_end(end) {
                            last = close;
                            end = close + 1;
                            continue;
                        }
                        match t.kind {
                            TK::Identifier
                            | TK::Keyword(_)
                            | TK::Period
                            | TK::Lt
                            | TK::Gt
                            | TK::GtGt
                            | TK::GtGtGt
                            | TK::Function
                            | TK::QuestionQuestion => {
                                last = end;
                                end += 1;
                            }
                            // `T?` stays inside only when what follows is
                            // still type syntax (another `?`, or `>`/`)`/
                            // `]`/`}`/`,` closing the type's own context);
                            // `e is T ? a : b`'s `?` opens the conditional.
                            TK::Question
                                if matches!(
                                    toks.get(end + 1).map(|n| n.kind),
                                    Some(
                                        TK::Question
                                            | TK::Gt
                                            | TK::GtGt
                                            | TK::GtGtGt
                                            | TK::Comma
                                            | TK::CloseParen
                                            | TK::CloseSquare
                                            | TK::CloseCurly
                                            | TK::Semicolon
                                            | TK::Eof
                                    )
                                ) =>
                            {
                                last = end;
                                end += 1;
                            }
                            _ => break,
                        }
                    }
                    if last >= at + 1 {
                        let last_tok = toks[last];
                        span = SourceRange {
                            offset: span.offset,
                            length: last_tok.utf16_offset + last_tok.byte_len - span.offset,
                        };
                    } else {
                        break;
                    }
                }
                // `x!`: the null-assert postfix rides the operand's span.
                TK::Bang if next.utf16_offset == span.end() => {
                    span = SourceRange {
                        offset: span.offset,
                        length: next.utf16_offset + next.byte_len - span.offset,
                    };
                }
                // A type-argument head's literal tail: `<num>[]` and
                // `<k, v>{}` spell the collection through the bracket
                // group's close, not the `>` it starts with.
                TK::OpenSquare | TK::OpenCurly if next.utf16_offset == span.end() => {
                    let Some(close) = self.tokens.matching_end(at) else {
                        break;
                    };
                    let close_tok = toks[close];
                    span = SourceRange {
                        offset: span.offset,
                        length: close_tok.utf16_offset + close_tok.byte_len - span.offset,
                    };
                }
                _ => break,
            }
        }
        // The prefix walk: expression-forming tokens directly before the
        // current start — `await`/`new`/`const`/`throw`, the unary heads, a
        // dot-shorthand's `.`, an enclosing `(`. Each accepted token extends
        // the start and re-arms the walk (nested prefixes: `await new C()`,
        // `!x`, `-(a + b)`). The `before` guard reads the candidate's own
        // left neighbor: a `-`/`.`/`(` right after an expression-ending
        // token is infix/member/call syntax, not a prefix.
        loop {
            let at = toks.partition_point(|t| t.utf16_offset < span.offset);
            let Some(&prev) = at.checked_sub(1).and_then(|i| toks.get(i)) else {
                break;
            };
            let before_expr = |i: usize| {
                i.checked_sub(2)
                    .and_then(|j| toks.get(j))
                    .is_some_and(|t| ends_expression(t.kind))
            };
            match prev.kind {
                TK::Keyword(Kw::Await | Kw::New | Kw::Const | Kw::Throw) => {
                    span = SourceRange {
                        offset: prev.utf16_offset,
                        length: span.end() - prev.utf16_offset,
                    };
                }
                TK::Bang | TK::Minus | TK::Tilde | TK::PlusPlus | TK::MinusMinus
                    if !before_expr(at) =>
                {
                    span = SourceRange {
                        offset: prev.utf16_offset,
                        length: span.end() - prev.utf16_offset,
                    };
                }
                TK::Period if !before_expr(at) => {
                    span = SourceRange {
                        offset: prev.utf16_offset,
                        length: span.end() - prev.utf16_offset,
                    };
                }
                // `(e)`: a parenthesized operand's node includes the parens
                // — extend only when this `(` pairs with a `)` at the
                // current end AND its own left neighbor cannot end an
                // expression (in `f((e))`, the inner `(e)` wraps `e` but the
                // outer `(` is the call's argument list, not a prefix).
                TK::OpenParen if !before_expr(at) => {
                    let Some(close) = self.tokens.matching_end(at - 1) else {
                        break;
                    };
                    if toks[close].utf16_offset != span.end() {
                        break;
                    }
                    span = SourceRange {
                        offset: prev.utf16_offset,
                        length: toks[close].utf16_offset + toks[close].byte_len - prev.utf16_offset,
                    };
                }
                _ => break,
            }
        }
        span
    }

    /// The right-hand side's result row: the last non-`TypeUse`,
    /// non-interpolation row strictly after `offset`. Every fold pushes
    /// the outermost expression's result last — a member merge leaves the
    /// member row, a cascade its target-typed value stand-in, a
    /// parenthesized group its own row, a literal/operator fold its value
    /// row — so a multi-row window resolves to its tail exactly like the
    /// single-row one resolves to its only entry.
    fn rhs_result_row(&self, offset: u32) -> Option<&Row> {
        self.rows
            .iter()
            .rev()
            .filter(|row| row.range.offset > offset)
            .filter(|row| !matches!(row.shape, RowShape::TypeUse(..)))
            .find(|row| !self.interpolation_child_rows.contains(&row.range.offset))
    }

    /// The index of the assignment's candidate target row: the closest row
    /// wholly before `limit` (the operator's offset), skipping folded value
    /// rows (index results, groups — the same scan `mark_assignment` runs).
    /// `rows` is offset-sorted (the `invocation.rs` `partition_point`
    /// invariant), so a binary search bounds the walk to the pre-operator
    /// prefix — the right-hand-side rows after `limit` are skipped in
    /// O(log n) instead of walked one-by-one. The bound is `offset <=
    /// limit`, not `<`: `end() <= limit` implies `offset <= limit` (a row
    /// never ends before it starts), so every verdict-eligible row sits
    /// inside the prefix — including a zero-width row parked exactly on
    /// the operator's offset. Inside that prefix `end()` is NOT monotone
    /// (a long outer row can out-end a later inner row), so the
    /// `end() <= limit` verdict still needs a bounded reverse scan — but
    /// only within the prefix, never across the right-hand tail.
    ///
    /// The `cfg(test)` twin counts predicate visits into
    /// [`ASSIGNMENT_TARGET_VISITS`] — the #2487 pin reads it to assert
    /// the bounded scan never walks the right-hand tail.
    #[cfg(not(test))]
    fn assignment_target_index(&self, limit: u32) -> Option<usize> {
        let bound = self
            .rows
            .partition_point(|row| row.range.offset <= limit);
        self.rows[..bound]
            .iter()
            .rposition(|row| row.range.end() <= limit && !matches!(row.shape, RowShape::Value))
    }

    /// [`Self::assignment_target_index`]'s instrumented twin (see its doc).
    #[cfg(test)]
    fn assignment_target_index(&self, limit: u32) -> Option<usize> {
        let bound = self
            .rows
            .partition_point(|row| row.range.offset <= limit);
        ASSIGNMENT_TARGET_MAX_TAIL.fetch_max(
            self.rows.len() - bound,
            std::sync::atomic::Ordering::Relaxed,
        );
        self.rows[..bound]
            .iter()
            .rposition(|row| {
                ASSIGNMENT_TARGET_VISITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                row.range.end() <= limit && !matches!(row.shape, RowShape::Value)
            })
    }

    /// The two operand rows of a `??` or conditional tail ending at `last`:
    /// rows `[.., left, last]` where the operator tokens sit between the
    /// left row's end and `last`'s start. Returns `(left, is_if_null)` —
    /// `??` joins `LUB(nonNull(left), last)`, `?:` joins `LUB(then, last)`.
    /// `None` for every other neighborhood (a single-operand tail, a
    /// nested composite whose operands the tail row does not pair with).
    fn binary_join_operands<'r>(&'r self, last: &'r Row) -> Option<(&'r Row, bool)> {
        let index = self.rows.iter().rposition(|row| std::ptr::eq(row, last))?;
        let left = self.rows[..index]
            .iter()
            .rev()
            .filter(|row| !matches!(row.shape, RowShape::TypeUse(..)))
            .find(|row| !self.interpolation_child_rows.contains(&row.range.offset))?;
        let toks = self.tokens.as_slice();
        let lo = toks.partition_point(|t| t.utf16_offset < left.range.end());
        let hi = toks.partition_point(|t| t.utf16_offset < last.range.offset);
        let mut colon = false;
        let mut ifnull = false;
        for token in &toks[lo..hi] {
            match token.kind {
                dartq_scanner::TokenKind::Colon => colon = true,
                dartq_scanner::TokenKind::QuestionQuestion => ifnull = true,
                _ => {}
            }
        }
        if ifnull {
            return Some((left, true));
        }
        if !colon {
            return None;
        }
        // A `:` between the operands is a conditional's else marker only
        // when its matching `?` precedes the left row (named-argument
        // labels, map-literal pairs, and switch-case colons pair no `?`).
        let before = self.rows[..index]
            .iter()
            .rev()
            .filter(|row| !matches!(row.shape, RowShape::TypeUse(..)))
            .filter(|row| !self.interpolation_child_rows.contains(&row.range.offset))
            .nth(1)
            .map_or(0, |row| row.range.end());
        let lo = toks.partition_point(|t| t.utf16_offset < before);
        toks[lo..]
            .iter()
            .take_while(|t| t.utf16_offset < left.range.offset)
            .any(|t| t.kind == dartq_scanner::TokenKind::Question)
            .then_some((left, false))
    }

    /// The join of two operand types for a `??`/`?:` tail — upstream
    /// answers `leastUpperBound` (`resolveIfNull` non-nulls the left
    /// first). The name lattice decides the same-head and declared-edge
    /// pairs (the join is then the supertype), `Null` legs join the
    /// nullable form of the other, and everything else keeps the right
    /// operand's face — the under-report direction (a shared-interface
    /// LUB this model cannot spell stays the operand's own type, never a
    /// false `Object` verdict).
    fn join_operand_ty(&self, left: Ty, right: Ty, non_null_left: bool) -> Ty {
        let left = if non_null_left {
            Self::non_null_form(left)
        } else {
            left
        };
        match (left, right) {
            (Ty::Null, other) | (other, Ty::Null) => Self::nullable_form(other),
            (
                Ty::Named {
                    name: l,
                    nullable: ln,
                    args: la,
                },
                Ty::Named {
                    name: r,
                    nullable: rn,
                    args: ra,
                },
            ) if ln == rn => {
                if l == r {
                    Ty::Named {
                        name: l,
                        nullable: ln,
                        args: if la == ra { la } else { ra },
                    }
                } else {
                    let world = self.name_world();
                    if world.named_sub(r, l) {
                        Ty::Named {
                            name: l,
                            nullable: ln,
                            args: la,
                        }
                    } else if world.named_sub(l, r) {
                        Ty::Named {
                            name: r,
                            nullable: rn,
                            args: ra,
                        }
                    } else {
                        right
                    }
                }
            }
            _ => right,
        }
    }

    /// The right-hand expression's static type from its result `row`:
    /// [`Self::expression_row_ty`] first, then the faces the row model
    /// leaves uncaptured at parse time — a member read on a creation
    /// receiver (the merge's declared-type write answers `Typed`
    /// receivers only), a bare creation head (an unsolved `C()` folds
    /// `Unknown` where the oracle's `staticType` is the instantiated
    /// container), and a `??`/`?:` tail (the join of its two operand
    /// rows). Everything unresolved keeps the under-report `Unknown`.
    ///
    /// An `await` operand's expression-level static type is
    /// `flatten(operand)` (`analyzeAwaitExpression`,
    /// type_analyzer.dart:542-548). The close-time row rewrite holds only
    /// the STORED `ty`: an invocation operand re-decides through
    /// [`Self::expression_row_ty`]'s send face and spells the raw
    /// `Future<T>` again, so the flatten re-applies over the operand
    /// extent — the same on-demand read the argument face runs
    /// (`flush_argument`/`flush_unclaimed`). Only the OUTER row takes it:
    /// the `??`/`?:` left operand below is a sub-expression of the same
    /// awaited operand, whose own static type upstream does not flatten.
    fn rhs_expression_ty(&self, row: &Row) -> Ty {
        let ty = self.rhs_row_ty(row);
        if ty != Ty::UNKNOWN
            && self
                .await_operand_extents
                .iter()
                .any(|&(start, end)| row.range.offset >= start && row.range.end() <= end)
        {
            self.flatten(ty)
        } else {
            ty
        }
    }

    /// The static type a `var`-inferred local's read still owns when the
    /// parse-time bind landed `Unknown`: `None` unless the read is a bare
    /// local whose declarator's initializer is a single send whose solved
    /// result the close left on the row as a function face. The send's
    /// result is the callee's instantiated return — the face
    /// [`Self::callee_return_ty`] re-derives — which the parse-time
    /// `operand_static_ty` arm misread as the uninvoked signature whenever
    /// the send-close fold kept the callee's function face on the row
    /// (`toF<X>` returning `F<X>` stores `void Function<Y extends X'>`,
    /// whose own type-parameter count defeats the callee-result unwrap
    /// and silently degrades the whole local to `Unknown`).
    fn var_inferred_send_init_ty(&self, read: &Row) -> Option<Ty> {
        if !matches!(read.shape, RowShape::Bare(_)) || read.qualified {
            return None;
        }
        // Only a `var`-inferred local can bind `Unknown` here — a written
        // annotation always lands a face. `typed_local` answers the bound
        // slot; `Some(Unknown)` is the tell that the bind degraded.
        if self.typed_local(read.name) != Some(Ty::Unknown) {
            return None;
        }
        let decl_offset = *self.local_decl_at_read.get(&read.range.offset)?;
        // The declarator's initializer send row: the single `Invoke` row
        // in the window between the declarator's `=` and this read. A
        // `var x = <single send>` keeps exactly one such row; any other
        // shape (a composite, a member read, a literal) leaves none or
        // more than one, and the bind's `Unknown` is honest — recover
        // only the single-send shape the parse-time arm provably misread.
        let mut send: Option<&Row> = None;
        for cand in self
            .rows
            .iter()
            .filter(|cand| cand.range.offset > decl_offset && cand.range.offset < read.range.offset)
            .filter(|cand| !matches!(cand.shape, RowShape::TypeUse(..)))
            .filter(|cand| cand.intent == Intent::Invoke)
        {
            if send.is_some() {
                return None;
            }
            send = Some(cand);
        }
        let send = send?;
        // The send must carry the result face the solve left behind — a
        // `Ty::Function` the callee's own return spelled, never the
        // callee's uninvoked signature (which this same misread would
        // then double-unwrap). `callee_return_ty` resolves the declared
        // return instantiated for the call site.
        if !matches!(send.ty, Ty::Function { .. }) {
            return None;
        }
        let ty = self.callee_return_ty(send);
        (ty != Ty::UNKNOWN).then_some(ty)
    }

    /// `rhs_expression_ty`'s undecorated face — the `??`/`?:` recursion
    /// reads sub-expression types, which stay unflattened inside an
    /// `await` operand (the flatten belongs to the whole expression).
    fn rhs_row_ty(&self, row: &Row) -> Ty {
        let mut ty = self.expression_row_ty(row);
        if ty == Ty::UNKNOWN {
            ty = match &row.shape {
                RowShape::Member { .. } if row.intent != Intent::Invoke => {
                    self.accessor_read_ty(row)
                }
                RowShape::Receiver(Receiver::Created { name, kind, .. })
                    if *kind != ContainerKind::Extension =>
                {
                    self.instantiated_head_ty(Ty::named(*name))
                }
                _ => Ty::UNKNOWN,
            };
        }
        if let Some((left, ifnull)) = self.binary_join_operands(row) {
            let left_ty = self.rhs_row_ty(left);
            if left_ty != Ty::UNKNOWN && ty != Ty::UNKNOWN {
                return self.join_operand_ty(left_ty, ty, ifnull);
            }
        }
        ty
    }

    /// The assignment check (`invalid_assignment`,
    /// `assignment_expression_resolver.dart:795`): a single-row right-hand
    /// side whose type the M1 facts know (`Null` or a captured declared
    /// type) against the target's declared type. The `??=` face shares the
    /// same verdict — `resolveIfNull` closes with the plain
    /// `_checkForInvalidAssignment(writeAcceptedType, node.value,
    /// valueType)` (:697-703), the RHS against the target's write type.
    /// Compound operators ride [`Self::check_compound_assignment`] (the
    /// augmented operator's result type against the same target).
    pub(crate) fn check_assignment(&mut self, operator: Token) {
        // F-C: a constructor-initializer list owns its assignments — the
        // field-initializer assignability question codes through the
        // dedicated initializer face (`field_initializer_not_assignable`),
        // never the plain `invalid_assignment`. The resolution below is
        // shared; the face branches at the verdict.
        let in_initializer = self.initializer_depth > 0;
        let Some(op_text) = self.text_of(operator) else {
            return;
        };
        if op_text != "=" && op_text != "??=" {
            self.check_compound_assignment(op_text, operator);
            return;
        }
        let limit = operator.range.offset;
        // The target: the closest row wholly before the operator, skipping
        // folded value rows (the shared `assignment_target_index` scan —
        // `mark_assignment` runs the same walk).
        let Some(target) = self.assignment_target_index(limit) else {
            return;
        };
        // `c[i] = e` writes through the `[]=` operator: the target's own
        // declared type is not the assignment's destination type.
        if matches!(self.rows[target].intent, Intent::Operator(..)) {
            return;
        }
        // The right-hand side's result row — the fold discipline keeps the
        // outermost expression's row last, so a multi-row window resolves
        // to its tail (`x = B().x`'s member row, `x = a..m`'s
        // target-typed value, `x = (a)`'s group row) exactly like the
        // single-row window resolved to its only entry.
        let Some(rhs_row) = self.rhs_result_row(limit).cloned() else {
            return;
        };
        // The oracle anchors at the whole right-hand expression
        // (`AssignmentExpression.rightHandSide`): a multi-row window spans
        // its first operand row through the result row's end — `x = c ?
        // a : b` marks the ternary, `x = a ?? b` the pair — while a
        // single-row window is the row's own range exactly as before.
        let range = {
            let first = self
                .rows
                .iter()
                .filter(|row| row.range.offset > limit)
                .filter(|row| !matches!(row.shape, RowShape::TypeUse(..)))
                .find(|row| !self.interpolation_child_rows.contains(&row.range.offset))
                .map_or(rhs_row.range.offset, |row| row.range.offset);
            SourceRange {
                offset: first,
                length: rhs_row.range.end() - first,
            }
        };
        // A `?.`-merged right side reads the short-circuit's nullable
        // result (`x = a?.b` asks `B?` against `x`'s slot) —
        // [`Engine::expression_row_ty`] re-arms the `?` the merge could not
        // join onto the row's declared face.
        let mut from = self.rhs_expression_ty(&rhs_row);
        // The promoted read (upstream feeds the flow-promoted `valueType`
        // into `_checkForInvalidAssignment`, :694-703): an unpromoted
        // dynamic/unknown right side a flow fact promoted (`y is String`
        // over an untyped parameter) re-decides at the read's exact span.
        if matches!(from, Ty::Unknown | Ty::Dynamic)
            && let Some(promoted) = self.read_type_at(range)
        {
            from = promoted;
        }
        // The initializer's tear-off twin: a bare function-name right
        // side reads the declaration's own `FunctionType`
        // ([`Self::tear_off_sig`]'s element face), so the assignment ask
        // runs on the signature instead of the `Unknown` row.
        if from == Ty::Unknown
            && let Some(sig) = self.tear_off_sig(&rhs_row)
        {
            from = self.fn_sig_ty(&sig);
        }
        // A `var`-inferred local read whose send-initializer's result the
        // parse-time bind degraded to `Unknown` — the same recovery
        // [`Self::check_decl_initializer`] runs (`var f = toF(src)`
        // feeding `x = f`).
        if from == Ty::Unknown
            && let Some(ty) = self.var_inferred_send_init_ty(&rhs_row)
        {
            from = ty;
        }
        let literal = Self::primitive_literal(&rhs_row, &self.members.primitive_ids);
        let target_row = self.rows[target].clone();
        // A bare initializer-list target names the FIELD (the
        // `ConstructorFieldInitializer`'s `fieldName` resolves in the
        // container, never the formal scope): a same-named parameter
        // (`content = ''` beside a `Map content` formal) must not lend its
        // type to the slot.
        let to = match (in_initializer, &target_row.shape) {
            (true, RowShape::Bare(_)) if !target_row.qualified => {
                match self.init_rows.get(&target_row.range.offset).copied() {
                    Some(container) => self.member_write_ty(container, target_row.name),
                    None => self.assignment_target_ty(&target_row),
                }
            }
            _ => self.assignment_target_ty(&target_row),
        };
        // An unannotated collection literal's row is opaque (`Unknown`):
        // the target's write type is the context that types it, and a
        // context its head cannot match leaves the leaves' own upward type
        // (#3424, [`Self::untyped_literal_ty`]). The literal anchors at its
        // own brackets — the window's first row is its first leaf, never
        // the `[`/`{` the oracle's `.at(rightHandSide)` starts at.
        let range = if from == Ty::Unknown
            && let Some((ty, anchor)) = self.untyped_literal_ty(&rhs_row, to, range.offset)
        {
            from = ty;
            anchor
        } else {
            range
        };
        // The invalid-source gate (upstream `InvalidType`): a right side
        // whose written head resolves to no type asks no assignability
        // question (probed 3.47.1: `expression = parent` with `AstNode?
        // parent` stays silent). The target side gates nothing — a
        // projected foreign declared type this unit cannot name
        // (`List<int>` of an import) still reports (`Null` into it is the
        // oracle's `invalid_assignment`), and an invalid target face
        // already decides silent through the target read's own recovery.
        if self.written_ty_invalid(from) {
            return;
        }
        // The void-operand use (upstream
        // `assignment_expression_resolver.dart:136` —
        // `writeType is! VoidType && _checkForUseOfVoidResult(right)`
        // short-circuits the whole ask): `x = v()` into a non-`void`
        // target reports `use_of_void_result` at the right operand and
        // answers no `invalid_assignment`. A constructor field initializer
        // resolves through the initializer helper instead, which asks
        // assignability FIRST — its void face rides the granted verdict
        // below.
        if !in_initializer && to != Ty::Void && from == Ty::Void {
            self.emit_use_of_void_result(from, &rhs_row);
            return;
        }
        // The constructor field initializer's `void` face (upstream
        // `checkForFieldInitializerNotAssignable` — error_detection_helpers
        // .dart — asks assignability FIRST and only then the void use): a
        // `void` source fits the top hosts alone (`dynamic`, `Object?`), so
        // those answer `use_of_void_result` at the initializer while every
        // other field type answers `field_initializer_not_assignable`.
        if in_initializer && from == Ty::Void && to != Ty::Void {
            if self.name_subtype(Ty::Void, to) {
                self.emit_use_of_void_result(from, &rhs_row);
            } else {
                let (from_text, to_text) = (self.ty_display(from), self.ty_display(to));
                let range = self.property_access_span(rhs_row.range).unwrap_or(rhs_row.range);
                self.emit_non_promotion(
                    SemanticCode::FieldInitializerNotAssignable,
                    range,
                    &from_text,
                    &to_text,
                    rhs_row.range.offset,
                );
            }
            return;
        }
        let stashed = self.stashed_fn_pair(&rhs_row, Some(&target_row), from, to);
        let verdict = stashed
            .as_ref()
            .map(|(verdict, ..)| *verdict)
            .or_else(|| self.assignment_verdict(from, to, literal));
        // The conversion's own face (#2870): a granted `int → double`
        // literal ask doubles as the imprecise-value check.
        if verdict == Some(true) {
            self.check_imprecise_int_literal(from, to, literal, rhs_row.range);
        }
        if verdict == Some(false) && stashed.is_some() && !in_initializer {
            // The stashed-signature rejection is final at the check: the
            // from side is a concrete function shape (never a promotable
            // read — the promotable twin keys on Unknown/Dynamic
            // sources), and the teardown's re-decision runs on the
            // interned Tys alone, which carry no parameter bounds. Emit
            // with the teardown's own anchor and message shape — the
            // message spells the deciding signatures (the oracle renders
            // the resolved types' parameters, #2849).
            let range = self
                .invocation_span(range)
                .or_else(|| self.property_access_span(range))
                .unwrap_or(range);
            let (_, from_sig, to_sig) = stashed.expect("checked above");
            let (from_text, to_text) =
                (self.fn_sig_display(&from_sig), self.fn_sig_display(&to_sig));
            self.emit_non_promotion(
                SemanticCode::InvalidAssignment,
                range,
                &from_text,
                &to_text,
                rhs_row.range.offset,
            );
            return;
        }
        if verdict == Some(false) {
            if in_initializer {
                let (from_text, to_text) = (self.ty_display(from), self.ty_display(to));
                // The oracle anchors at the whole right-hand expression —
                // a `receiver.member` operand reports over the property-access
                // span (`fieldInitializer.expression`), not the member name.
                let range = self.property_access_span(range).unwrap_or(range);
                // The why-not-promoted rows hang on the initializer
                // expression's read (#1877; upstream Dart SDK source,
                // pkg/analyzer/lib/src/generated/
                // error_detection_helpers.dart:198-206).
                self.emit_non_promotion(
                    SemanticCode::FieldInitializerNotAssignable,
                    range,
                    &from_text,
                    &to_text,
                    rhs_row.range.offset,
                );
            } else {
                self.stage(StagedKind::Assignment, range, from, to, literal);
            }
        } else if !in_initializer
            && matches!(from, Ty::Unknown | Ty::Dynamic)
            && matches!(rhs_row.shape, RowShape::Bare(_))
        {
            // The promotable-read staging: an unknown/dynamic bare-read
            // right side stages even on a non-negative parse-time verdict —
            // upstream checks the flow-PROMOTED `valueType`
            // (assignment_expression_resolver.dart:694-703), so `y is
            // String` over an untyped parameter CREATES the rejection the
            // parse-time dynamic face accepted. Only a variable read can
            // promote; every other face stays promotion-cleans-only (a
            // row with no flow fact re-derives to its parse-time capture
            // and suppresses at the teardown).
            self.stage(StagedKind::Assignment, range, from, to, literal);
        }
    }

    /// The compound-assignment face (`invalid_assignment`,
    /// assignment_expression_resolver.dart:308-313): `x op= e` asks the
    /// augmented operator's result type against the target's write type —
    /// `_computeCompoundOperatorResultType` types the read `x + e` through
    /// the same numeric refinement the binary fold runs, so
    /// `int total; total += <invalid receiver read>` answers `double` and
    /// reports "A value of type 'double' can't be assigned to a variable
    /// of type 'int'" (probed 3.13.1, the manifest_ast.dart census row).
    /// The target's read type resolves the operator; a `dynamic` or
    /// unknown read, an operator that does not resolve, an invalid
    /// computed type, and `void` sides stay silent. `??=` is the if-null
    /// face (upstream answers `InvalidType` — no operator element) and is
    /// excluded here; the RHS's own argument face is the under-report
    /// direction.
    fn check_compound_assignment(&mut self, op_text: &str, operator: Token) {
        let Some(op) = op_text.strip_suffix('=') else {
            return;
        };
        if !USER_OPERATORS.contains(&op) {
            return;
        }
        let limit = operator.range.offset;
        // The target: the closest row wholly before the operator, skipping
        // folded value rows (the shared `assignment_target_index` scan —
        // [`Self::check_assignment`] reads the same verdict).
        let Some(target) = self.assignment_target_index(limit) else {
            return;
        };
        // `c[i] op= e` writes through `[]=`: no assignability face here
        // (the plain face's rule).
        if matches!(self.rows[target].intent, Intent::Operator(..)) {
            return;
        }
        // The right-hand side's result row (the plain face's result-row
        // window — the outermost expression's row is the tail).
        let Some(rhs_row) = self.rhs_result_row(limit).cloned() else {
            return;
        };
        let target_row = self.rows[target].clone();
        let read_ty = self.receiver_static_ty(&target_row);
        let to = self.assignment_target_ty(&target_row);
        if matches!(read_ty, Ty::Unknown | Ty::Void) || matches!(to, Ty::Unknown | Ty::Void) {
            return;
        }
        let Some((param, declared, extension_resolved)) =
            self.compound_operator_sig(op, &target_row)
        else {
            return;
        };
        let right_ty = self.equality_operand_ty(&rhs_row);
        // An `await e` operand's augmented-operator read resolves against
        // the expression's own face — the flattened `flatten(T)` of the
        // operand's `Future<T>` capture (`x += await f()` asks the operator
        // `int`'s result, not `Future<int>`'s; the row's `ty` field keeps
        // the parse-time operand face while the await extent carries the
        // window the flatten re-applies over). Upstream flattens the
        // AwaitExpression's staticType at endAwaitExpression — the same
        // `operand_static_ty` + `flatten` the listener's row rewrite runs.
        let in_await = self
            .await_operand_extents
            .iter()
            .any(|&(start, end)| start <= rhs_row.range.offset && rhs_row.range.end() <= end);
        let right_ty = if in_await {
            self.flatten(right_ty)
        } else {
            right_ty
        };
        let from = self.compound_result_ty(op, read_ty, &rhs_row, right_ty, declared, extension_resolved);
        if matches!(from, Ty::Unknown | Ty::Void | Ty::Dynamic) || self.written_ty_invalid(from) {
            return;
        }
        let literal = Self::primitive_literal(&rhs_row, &self.members.primitive_ids);
        // The operator invocation's own argument row (upstream
        // `checkForArgumentTypeNotAssignableForArgument(rightHandSide)` —
        // assignment_expression_resolver.dart:368-372, every compound
        // operator except `=`/`??=`): the right side's static type asks the
        // operator's declared parameter (`int.+` takes `num`, `String.+`
        // takes `String`) independently of the write-back verdict —
        // `t += 1` reports ATNA alone (the `String` result writes back
        // clean), `i += "x"` reports ATNA beside the write-back refusal.
        // An invalid right-side head asks no question (the shared
        // invalid-source gate, upstream `InvalidType` recovery). The
        // promoted-read twin re-derives an unpromoted dynamic/unknown
        // right side at its span — the oracle checks the flow-promoted
        // static type.
        let arg_ty = if matches!(right_ty, Ty::Unknown | Ty::Dynamic)
            && let Some(promoted) = self.read_type_at(rhs_row.range)
        {
            promoted
        } else {
            right_ty
        };
        if !self.written_ty_invalid(arg_ty)
            && self.argument_verdict(arg_ty, param, literal) == Some(false)
        {
            let message = SemanticCode::ArgumentTypeNotAssignable.message3(
                &self.ty_display(arg_ty),
                &self.ty_display(param),
                "",
            );
            self.emit_message_with_correction(
                SemanticCode::ArgumentTypeNotAssignable,
                rhs_row.range,
                message,
            );
        }
        if self.assignable(from, to, literal) == Some(false) {
            // The oracle anchors at the whole right-hand expression
            // (`.at(right)`).
            self.stage(StagedKind::Assignment, rhs_row.range, from, to, literal);
        }
    }

    /// The compound operator's resolved signature over `target_row`: the
    /// `(param, declared result, extension-resolved)` triple
    /// [`Self::check_compound_assignment`] and [`Self::compound_element_ty`]
    /// share. The arm order mirrors the binary fold (`fold_binary`): an
    /// applicable extension over the nullable receiver resolves the
    /// operator FIRST, and an extension-declared signature is never
    /// numerically refined — `_refineNumericInvocationTypeNullSafe` answers
    /// the declared type unchanged for an `ExtensionElement` encloser
    /// (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/type_system.dart:2266-2277).
    /// Reading the extension arm through the shared `binary_operator_sig`
    /// would lose that provenance — the callers' `from` computation must
    /// know which arm answered, so the extension consult stands alone here
    /// and the generic path (which re-consults it internally and returns
    /// `None` only when no arm did) carries the refine flag.
    fn compound_operator_sig(
        &mut self,
        op: &str,
        target_row: &Row,
    ) -> Option<(Ty, Ty, bool)> {
        let spelling = self.intern(op);
        let recv = self.bound_face(self.equality_operand_ty(target_row));
        if let Some((param, declared)) = self.nullable_extension_operator_sig(recv, spelling) {
            return Some((param, declared, true));
        }
        let (param, declared) = self.binary_operator_sig(op, target_row)?;
        Some((param, declared, false))
    }

    /// The compound expression's result type (`x op= e`'s static type —
    /// `AssignmentExpressionImpl.staticType`, the resolved operator's
    /// return): `declared` verbatim when the signature came from an
    /// extension face (`x += null` over `on int? operator +` writes the
    /// declared `int?` back, never the `double` the builtin `int.+`
    /// refinement would invent), else the numeric refinement over the
    /// operand faces.
    fn compound_result_ty(
        &mut self,
        op: &str,
        read_ty: Ty,
        rhs_row: &Row,
        right_ty: Ty,
        declared: Ty,
        extension_resolved: bool,
    ) -> Ty {
        if extension_resolved {
            declared
        } else {
            let face = self.operand_face(rhs_row, right_ty);
            self.refine_numeric_binary(op, read_ty, face, declared)
        }
    }

    /// The `?e` element's value type when `e` is a compound assignment
    /// (`?x += 1` — the oracle's `node.value2.staticType` is the
    /// AssignmentExpression's resolved-operator result, `int?`, not the RHS
    /// literal's `int` the tail row carries). `handle_null_aware_element`
    /// consults this before the last-row fallback: a still-unfolded
    /// compound leaves its operand and RHS as two rows with the `op=` token
    /// between them, so the tail `[target(+adjacent compound op), rhs]`
    /// resolves the augmented result through the same extension-first
    /// signature the write-back verdict reads. `None` for every non-
    /// compound tail — a plain `?e` element, a `??=` if-null write (not a
    /// user operator), or a prefix `?-x`/`?x++` whose operand the verdict
    /// already reads correctly.
    pub(crate) fn compound_element_ty(&mut self) -> Option<Ty> {
        let rhs_row = self.rows.last()?.clone();
        let pos = self.rows.len() - 1;
        // The operand row sits immediately before the RHS; the `op=` token
        // rides `adjacent_operator` on it (`x += 1`'s `+` — the same probe
        // `operator_face_extension_resolved` runs for the unchecked face).
        let target_row = self.rows[..pos]
            .iter()
            .rev()
            .find(|row| !matches!(row.shape, RowShape::Value))?
            .clone();
        let (text, .., compound) = self.adjacent_operator(&target_row)?;
        if !compound {
            return None;
        }
        let op = text;
        let read_ty = self.receiver_static_ty(&target_row);
        if matches!(read_ty, Ty::Unknown | Ty::Void) {
            return None;
        }
        let (_, declared, extension_resolved) = self.compound_operator_sig(op, &target_row)?;
        let right_ty = self.equality_operand_ty(&rhs_row);
        let result = self.compound_result_ty(op, read_ty, &rhs_row, right_ty, declared, extension_resolved);
        (!matches!(result, Ty::Unknown | Ty::Void)).then_some(result)
    }

    /// The variable-declaration initializer check
    /// (`variable_declaration_resolver.dart:140`): the same decision as the
    /// assignment check against the declaration group's declared type.
    pub(crate) fn check_decl_initializer(&mut self) {
        let Some(to) = self.pending_decl_ty else {
            return;
        };
        let Some(mark) = self.init_mark else {
            return;
        };
        // The initializer's result row (the plain face's result-row
        // window — the outermost expression's row is the tail; `U f =
        // C()` / `String y = B().x` keep their callee/member rows ahead
        // of it).
        let Some(row) = self
            .rows
            .get(mark..)
            .and_then(|tail| {
                tail.iter().rev().find(|row| {
                    !matches!(row.shape, RowShape::TypeUse(..))
                        && !self.interpolation_child_rows.contains(&row.range.offset)
                })
            })
            .cloned()
        else {
            return;
        };
        let literal = Self::primitive_literal(&row, &self.members.primitive_ids);
        // A `?.`-merged initializer reads the short-circuit's nullable
        // result (`int x = a?.b` asks `B?` against `int`).
        let mut from = self.rhs_expression_ty(&row);
        // The promoted read: an unpromoted dynamic/unknown initializer a
        // flow fact promoted (`int x = y` under `y is String`) re-decides —
        // the oracle checks the promoted `valueType`
        // (assignment_expression_resolver.dart:783-800).
        if matches!(from, Ty::Unknown | Ty::Dynamic)
            && let Some(promoted) = self.read_type_at(row.range)
        {
            from = promoted;
        }
        // A bare function-name initializer IS the tear-off: the read's
        // own static type is the declaration's `FunctionType` (the
        // element face `tear_off_sig` composes — locals first, then the
        // top-level declaration tables), so `int x = f;` asks the
        // function-against-int question the oracle reports and the
        // `Unknown` row alone never could.
        if from == Ty::Unknown
            && let Some(sig) = self.tear_off_sig(&row)
        {
            from = self.fn_sig_ty(&sig);
        }
        // An unannotated collection literal's row is opaque (`Unknown`):
        // the declared type is the context that types it, and a context
        // its head cannot match leaves the leaves' own upward type (#3424
        // — `C<int> c = {3, 1, 4}` over `C<T> implements
        // LinkedHashSet<T>` asks `Set<int>` against `C<int>`). The stage
        // anchors at the literal's own span, `const` included (the empty
        // `[]`'s row is a zero-width `[`, never the oracle's node); the
        // window's first row proves the literal opens the initializer.
        let mut anchor = row.range;
        if from == Ty::Unknown && self.untyped_literals.contains_key(&row.range.offset) {
            let first = self.rows[mark..]
                .iter()
                .find(|candidate| {
                    !matches!(candidate.shape, RowShape::TypeUse(..))
                        && !self
                            .interpolation_child_rows
                            .contains(&candidate.range.offset)
                })
                .map_or(row.range.offset, |candidate| candidate.range.offset);
            if let Some((ty, span)) = self.untyped_literal_ty(&row, to, first) {
                from = ty;
                anchor = span;
            }
        }
        // The invalid-source gate: a written head that resolves to no type
        // poisons the whole spelling (upstream `InvalidType`) — probed
        // 3.47.1: `int x = astNodeVal` over an undefined-class parameter
        // stays silent. The declared target gates nothing: the group's
        // written annotation is the question's own face (the projected
        // foreign-type posture [`Self::check_assignment` records).
        if self.written_ty_invalid(from) {
            self.init_mark = None;
            return;
        }
        // The void-operand use (upstream
        // `assignment_expression_resolver.dart:136` reached through
        // `variable_declaration_resolver.dart` —
        // `writeType is! VoidType && _checkForUseOfVoidResult(right)`
        // short-circuits): `Object a = v()` reports `use_of_void_result`
        // at the initializer and answers no `invalid_assignment`. `var a`
        // never reaches here — `pending_decl_ty` is `None` for an
        // inferred slot.
        if to != Ty::Void && from == Ty::Void {
            let row = row.clone();
            self.emit_use_of_void_result(from, &row);
            self.init_mark = None;
            return;
        }
        // A `var`-inferred local whose initializer is a solved generic
        // send binds `Unknown` at parse time when the send-close fold kept
        // the callee's instantiated function face on the row and
        // `operand_static_ty` misread it as the uninvoked signature —
        // `var fsource = toF(source)` where `toF<X>` returns a
        // function-typed alias `F<X>`: the fold stores the RESULT face
        // `void Function<Y extends X'>` (X' the solved argument), whose
        // own type-parameter count `callable_result_of` cannot unwrap, so
        // the bind lands `Unknown` and every read of the local stays
        // `Unknown`. The send's honest result is the row's own face (the
        // solve already instantiated it); re-derive it here so the
        // bound-equality legs below see `F<A<int>?>` and report the
        // oracle's `invalid_assignment` against `F<A<num>?>?`.
        if from == Ty::Unknown
            && let Some(ty) = self.var_inferred_send_init_ty(&row)
        {
            from = ty;
        }
        let stashed = self.stashed_fn_pair(&row, None, from, to);
        let verdict = stashed
            .as_ref()
            .map(|(verdict, ..)| *verdict)
            .or_else(|| self.assignment_verdict(from, to, literal));
        // The conversion's own face (#2870): a granted `int → double`
        // literal ask doubles as the imprecise-value check.
        if verdict == Some(true) {
            let rhs_range = row.range;
            self.check_imprecise_int_literal(from, to, literal, rhs_range);
        }
        if verdict == Some(false) && stashed.is_some() {
            // The stashed-signature rejection is final at the check (see
            // [`Self::check_assignment`]'s direct-emit arm) — the
            // teardown's re-decision on the interned Tys alone carries no
            // parameter bounds. The message spells the deciding
            // signatures (#2849).
            let range = if self.closure_literal_starts.contains(&row.range.offset) {
                // A function-literal RHS anchors at its own span — the
                // `(`-formals' `call_ends` entry is the literal's own
                // parameter list, not an invocation to widen to.
                row.range
            } else {
                self.invocation_span(row.range)
                    .or_else(|| self.property_access_span(row.range))
                    .unwrap_or(row.range)
            };
            let (_, from_sig, to_sig) = stashed.expect("checked above");
            let (from_text, to_text) =
                (self.fn_sig_display(&from_sig), self.fn_sig_display(&to_sig));
            self.emit_non_promotion(
                SemanticCode::InvalidAssignment,
                range,
                &from_text,
                &to_text,
                row.range.offset,
            );
            self.init_mark = None;
            return;
        }
        if verdict == Some(false)
            || (matches!(from, Ty::Unknown | Ty::Dynamic) && matches!(row.shape, RowShape::Bare(_)))
        {
            // The promotable-read staging twin (see `check_assignment`):
            // the flow-promoted initializer face creates the rejection the
            // parse-time dynamic/unknown capture accepted
            // (assignment_expression_resolver.dart:694-703's promoted
            // `valueType`); a row with no flow fact suppresses at the
            // teardown.
            self.stage(StagedKind::Assignment, anchor, from, to, literal);
        }
        self.init_mark = None;
    }

    /// The untyped collection literal's own static type against the
    /// declared context `to` (#3424; upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/typed_literal_resolver.dart).
    /// The downward constraint `L<E> <: K` over the literal's head `L`
    /// (`setupGenericTypeInference`, `K` the context's `futureOrBase`)
    /// matches exactly when `K` is a top face or one of `L`'s own
    /// supertypes — `List`/`Set` reach `Iterable`, every head reaches
    /// `Object` — and the literal is then assignable by construction (its
    /// leaves are the element verifier's rows): `None`, the opaque row
    /// stands. A failed match leaves the arguments to the leaves' upward
    /// inference ([`Engine::collection_literal_ty`]): `{3, 1, 4}` against
    /// `C<int>` over `C<T> implements LinkedHashSet<T>` reads `Set<int>`,
    /// and an empty `{}` takes its side from the context
    /// ([`Self::empty_literal_is_set`]) — `Set<dynamic>` under
    /// `SplayTreeSet<int>`, `Map<dynamic, dynamic>` under `int` — and the
    /// answer carries the literal's own anchor. The literal must be the
    /// whole right-hand side, whose window opens at the `first` row. An
    /// invalid context, an undecidable side, and a leaf without an exact
    /// face answer `None` as well.
    fn untyped_literal_ty(&self, row: &Row, to: Ty, first: u32) -> Option<(Ty, SourceRange)> {
        let kind = *self.untyped_literals.get(&row.range.offset)?;
        // The whole-side gate: the window's first row starts inside the
        // brackets, and nothing but a `const` keyword widens the literal's
        // own span — a `?:`/`??` tail, a chained `y = {…}`, a cast, an
        // `await`, a unary head, or a postfix `!` carries another
        // expression's type and anchor. The walk starts from the bracket
        // span itself: the empty `[]`'s row is a zero-width `[`.
        let own = self.untyped_literal_span(row)?;
        let span = self.staged_expression_span(own);
        if first < own.offset || span.offset > own.offset || span.end() != own.end() {
            return None;
        }
        if span.offset < own.offset
            && self.source[self.byte_of(span.offset)..self.byte_of(own.offset)].trim_end()
                != "const"
        {
            return None;
        }
        // `futureOrBase`: a `FutureOr<X>` context matches through `X` (its
        // `Future<X>` leg never meets a collection head). Each step peels
        // one written level; the bound only guards a cyclic alias fold.
        let mut context = self.fold_alias_ty(to);
        for _ in 0..8 {
            let base = self.future_or_base(context);
            if base == context {
                break;
            }
            context = self.fold_alias_ty(base);
        }
        let object = self.intern("Object");
        if matches!(context, Ty::Dynamic | Ty::Void)
            || matches!(context, Ty::Named { name, .. } if name == object)
            || self.is_invalid_ty(context)
        {
            return None;
        }
        let [list, set, map, iterable] = self.collection_heads;
        let family = match kind {
            UntypedLiteral::List => list,
            UntypedLiteral::SetElements => set,
            UntypedLiteral::MapEntries => map,
            UntypedLiteral::EmptyBraces => {
                if self.empty_literal_is_set(context)? {
                    set
                } else {
                    map
                }
            }
        };
        if let Ty::Named { name, .. } = context
            && (name == family || (name == iterable && family != map))
        {
            return None;
        }
        Some((self.collection_literal_ty(row, family)?, span))
    }

    /// An empty `{}`'s side under a context its head may not match
    /// (`_fromContextType`'s `asInstanceOf(Iterable)` /
    /// `asInstanceOf(Map)`): a set literal exactly when the context
    /// implements `Iterable` and not `Map`; both or neither leave the
    /// empty literal's map default. A type parameter answers through its
    /// captured bound (`TypeParameterType.asInstanceOf` reads the bound);
    /// function, record, `Null`, and `Never` contexts implement neither.
    /// The implementation ask is the head-level declared walk
    /// ([`NameWorld::decide_subtype`] — `asInstanceOf` ignores the
    /// arguments), and a walk cut at an unmodeled head re-asks the core
    /// face for a definite hit alone (`SplayTreeSet` implements `Set` with
    /// no element model in reach; the core face's `false` is blind to the
    /// modeled edges and never decides). An undecidable `Iterable` answer
    /// is `None` unless the context provably implements `Map` (the map
    /// side either way); a proven `Iterable` beside an undecidable `Map`
    /// reads the set side — a type implementing both is a shape no library
    /// declares in practice.
    fn empty_literal_is_set(&self, context: Ty) -> Option<bool> {
        let mut context = context;
        // The bound chain (`T extends S`, `S extends Iterable<Object>`),
        // bounded like every parameter-bound walk.
        for _ in 0..4 {
            match context {
                Ty::Named { name, .. } if self.declared_head_is_type_param(name) => {
                    context = self.fold_alias_ty(self.param_bound(name)?);
                }
                _ => break,
            }
        }
        let Ty::Named { name, .. } = context else {
            return Some(false);
        };
        let [list, set, map, iterable] = self.collection_heads;
        if name == list || name == set || name == iterable {
            return Some(true);
        }
        if name == map {
            return Some(false);
        }
        // A bound chain deeper than the walk stays undecided.
        if self.declared_head_is_type_param(name) {
            return None;
        }
        let world = self.name_world();
        let core_hit = |interface: StringId| {
            implements_interface(
                self.graph,
                self.namespaces,
                self.index,
                self.strings,
                self.lib,
                self.file,
                name,
                interface,
                false,
            ) == Some(true)
        };
        let iterable_side = world
            .decide_subtype(name, iterable)
            .or_else(|| [iterable, set, list].into_iter().any(core_hit).then_some(true));
        let map_side = world
            .decide_subtype(name, map)
            .or_else(|| core_hit(map).then_some(true));
        match (iterable_side, map_side) {
            (Some(true), Some(true)) | (Some(false), _) | (None, Some(true)) => Some(false),
            (Some(true), _) => Some(true),
            (None, _) => None,
        }
    }

    /// The default-value assignability face (`invalid_assignment`,
    /// upstream `handleValuedFormalParameter`'s
    /// `checkForInvalidAssignment(defaultValue, paramType)` —
    /// error_verifier.dart: a `= e` default inside a formal list asks the
    /// expression's static type against the formal's written `to`). The
    /// expression's result row is the window's tail (the same
    /// [`Self::rhs_result_row`] discipline — `f({int x = C().s})` reads
    /// the member row); a const default promotes nothing, so a negative
    /// verdict emits directly rather than staging (the flow face never
    /// reaches a default's closed world). `None`/undecidable verdicts
    /// keep the under-report direction.
    pub(crate) fn check_default_value_assignable(&mut self, to: Ty) {
        let Some(rhs_row) = self
            .rows
            .iter()
            .rev()
            .find(|row| {
                !matches!(row.shape, RowShape::TypeUse(..))
                    && !self.interpolation_child_rows.contains(&row.range.offset)
            })
            .cloned()
        else {
            return;
        };
        let range = rhs_row.range;
        let mut from = self.rhs_expression_ty(&rhs_row);
        if matches!(from, Ty::Unknown | Ty::Dynamic)
            && let Some(promoted) = self.read_type_at(range)
        {
            from = promoted;
        }
        if from == Ty::Unknown
            && let Some(sig) = self.tear_off_sig(&rhs_row)
        {
            from = self.fn_sig_ty(&sig);
        }
        if self.written_ty_invalid(from) {
            return;
        }
        // The `void` value's own face (upstream
        // `checkForAssignableExpressionAtType`'s
        // `expectedStaticType is! VoidType && checkForUseOfVoidResult(...)`
        // short-circuit): a `void` default reports `use_of_void_result` at
        // the value and answers no `invalid_assignment`.
        if to != Ty::Void && from == Ty::Void {
            self.emit_use_of_void_result(from, &rhs_row);
            return;
        }
        let literal = Self::primitive_literal(&rhs_row, &self.members.primitive_ids);
        let stashed = self.stashed_fn_pair(&rhs_row, None, from, to);
        let verdict = stashed
            .as_ref()
            .map(|(verdict, ..)| *verdict)
            .or_else(|| self.assignment_verdict(from, to, literal));
        if verdict == Some(true) {
            self.check_imprecise_int_literal(from, to, literal, range);
        }
        if verdict != Some(false) {
            return;
        }
        let (from_text, to_text) = if let Some((_, from_sig, to_sig)) = stashed {
            (self.fn_sig_display(&from_sig), self.fn_sig_display(&to_sig))
        } else {
            (self.ty_display(from), self.ty_display(to))
        };
        let range = self
            .invocation_span(range)
            .or_else(|| self.property_access_span(range))
            .unwrap_or(range);
        self.emit_non_promotion(
            SemanticCode::InvalidAssignment,
            range,
            &from_text,
            &to_text,
            rhs_row.range.offset,
        );
    }

    /// The stashed-signature function-subtype verdict for an assignment
    /// pair (#2834): a side whose row names a declaration carrying a
    /// written function-type annotation (a formal, field, or top-level
    /// variable — [`Engine::fn_var_sigs`]) upgrades to the stashed
    /// signature, the only face that carries the annotation's own type
    /// parameters WITH bounds (the interned `Ty::Function` keeps just
    /// their count). Both sides must be function-shaped and a stash
    /// must serve at least one; everything else keeps the ordinary
    /// verdict ([`Self::assignment_verdict`]) — pairs the shape faces
    /// already decide are unchanged. The deciding signatures ride the
    /// answer (the byte-exact message renders them — the interned
    /// shapes they may upgrade carry no parameter bounds, #2849).
    fn stashed_fn_pair(
        &self,
        from_row: &Row,
        to_row: Option<&Row>,
        from: Ty,
        to: Ty,
    ) -> Option<(bool, Box<FnVarSig>, Box<FnVarSig>)> {
        let from_stash = self.stash_sig_of(from_row);
        let to_stash = to_row.and_then(|row| self.stash_sig_of(row));
        if from_stash.is_none() && to_stash.is_none() {
            return None;
        }
        let from_sig = from_stash.or_else(|| self.function_shape_sig(from))?;
        let to_sig = to_stash.or_else(|| self.function_shape_sig(to))?;
        let verdict = self.fn_sig_subtype(&from_sig, &to_sig)?;
        Some((verdict, from_sig, to_sig))
    }

    /// The stashed written signature a bare, unqualified row's
    /// declaration carries, when it carries one at all.
    fn stash_sig_of(&self, row: &Row) -> Option<Box<FnVarSig>> {
        if row.qualified || !matches!(row.shape, RowShape::Bare(_)) {
            return None;
        }
        self.fn_var_sigs.get(&row.name).cloned()
    }

    /// The alias-instantiated written-signature claim (#2849): a formal
    /// whose written annotation is a parameterized function-RHS alias use
    /// (`F<int>` over `typedef F<X> = void Function<Y extends X>()`)
    /// joins [`Engine::fn_var_sigs`] with the INSTANTIATED signature —
    /// the same entry the written `void Function<Y extends int>()`
    /// spelling binds. The element model keeps the annotation's resolved
    /// type: a use with arguments instantiates the alias's right-hand
    /// type, the arguments substituted into it bounds included (upstream
    /// Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
    /// named_type_resolver.dart `_instantiateElement` →
    /// `TypeAliasElementImpl.instantiateImpl`). The stash is the only
    /// face carrying parameters WITH bounds (#2834's junction), so the
    /// assignment pair's `relateTypeParameters` equality rule decides
    /// over real bounds where the interned count-only fold stayed
    /// silent. A bare-alias use, a non-alias annotation, and a capture
    /// miss (a foreign head, or a typedef the parse has not registered
    /// yet — the capture walks in declaration order, the same posture
    /// [`Engine::alias_fn_sig`] documents) bind nothing: the count-only
    /// under-report posture stays.
    pub(crate) fn stash_alias_instantiated_formal(&mut self, formal: StringId) {
        let Some(Ty::Named { name, args, .. }) = self.formal_written else {
            return;
        };
        if args == ArgListId::EMPTY {
            return;
        }
        if let Some(sig) = self.instantiated_alias_sig(name, args) {
            self.fn_var_sigs.insert(formal, sig);
        }
    }

    /// The return check (`rtv.dart:209-249` sync matrix, `:258-289`
    /// async): the verdict is [`Engine::return_verdict`], the frame's
    /// latched async modifier picking the face; a failing one stages — the
    /// teardown may still clean it against the expression's promoted read.
    pub(crate) fn check_return(&mut self, from: Ty, range: SourceRange, from_literal: bool) {
        // The innermost executable frame owns the return. A frame with no
        // latched return (`ret: None` — a function literal or closure whose
        // return type is context-inferred) no longer stops the walk flat:
        // its body's returns check against the literal's context return
        // type ([`Engine::check_closure_return`], the oracle's
        // `isClosure` arm, rtv.dart:160-165) — still never the enclosing
        // method's declared return (`firstDepleted`'s `Future<InventoryEvent?>`
        // must not adjudicate `onTimeout: () => null`).
        let found = self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Executable {
                ret,
                is_async,
                generator,
                ..
            } => Some(match ret {
                Some(ret) => Ok((
                    ret.ty,
                    ret.host,
                    ret.name,
                    ret.display,
                    ret.ctor,
                    *is_async,
                    *generator,
                )),
                None => Err((*is_async, *generator)),
            }),
            _ => None,
        });
        let Some(found) = found else {
            return;
        };
        let (to, host, name, display, ctor, is_async, generator) = match found {
            Ok(latched) => latched,
            Err((is_async, generator)) => {
                self.check_closure_return(from, range, from_literal, is_async, generator);
                return;
            }
        };
        // A bare `return;` carries no expression — the listener feeds the
        // `;` token's own span with a `void` source, so the byte at the
        // range's head is `;` exactly when no expression stood (a `return
        // void_fn();` spells the callee's identifier instead). The three
        // upstream early-returns below all key on it
        // (return_type_verifier.dart `verifyReturnStatement`:54-76).
        let bare = from == Ty::Void
            && self
                .source
                .as_bytes()
                .get(self.byte_of(range.offset))
                .is_some_and(|b| *b == b';');
        // A generative constructor's `return e;` reports its own code and
        // skips the assignability matrix entirely (:57-63 —
        // `isGenerativeConstructor`; the factory-latched `ctor` flag marks
        // the other face, a factory checks the pair like a function). The
        // bare `return;` falls through silent.
        if host == ExecKind::Constructor && !ctor {
            if !bare {
                self.emit_compile_time(
                    "return_in_generative_constructor",
                    "RETURN_IN_GENERATIVE_CONSTRUCTOR",
                    self.staged_expression_span(range),
                    "Constructors can't return values.",
                    Some(
                        "Try removing the return statement or using a factory constructor.",
                    ),
                );
            }
            return;
        }
        // A generator body's `return e;`/`=> e` is the parser's
        // `generatorReturnsValue` row (`returnInGenerator`,
        // parser_impl.dart:6628/:6855 — the `=>` body anchors at the arrow
        // token, the statement at the `return` keyword); the verifier then
        // early-returns (:66-68) so no assignability question is ever
        // asked. The bare `return;` reports nothing.
        if generator {
            if !bare {
                let anchor = self.generator_return_anchor(range);
                self.emit_compile_time(
                    "return_in_generator",
                    "RETURN_IN_GENERATOR",
                    anchor,
                    "Can't return a value from a generator function that uses the 'async*' or 'sync*' modifier.",
                    Some(
                        "Try replacing 'return' with 'yield', using a block function body, or changing the method body modifier.",
                    ),
                );
            }
            return;
        }
        // The bare `return;` gate (:292-308 `_checkReturnWithoutValue`):
        // the ok-set is `void`/`dynamic`/`Null`/`InvalidType` over `T`
        // (`_isVoidDynamicOrNull`, :341-346) for a synchronous body, or over
        // `futureValueType(T)` for `async` (:299-301). The anchor is the
        // `return` keyword — the `;` the listener handed is the
        // statement's end, not the keyword.
        if bare {
            let t_eff = if is_async {
                future_value_ty(self.strings, &self.boundary.pool(), to)
            } else {
                to
            };
            let ok = matches!(t_eff, Ty::Void | Ty::Dynamic | Ty::Null | Ty::Unknown)
                || matches!(t_eff, Ty::Named { name: head, .. } if !self.named_type_is_type(head));
            if !ok {
                let keyword = self.return_keyword_range(range);
                self.emit_compile_time(
                    "return_without_value",
                    "RETURN_WITHOUT_VALUE",
                    keyword,
                    "The return value is missing after 'return'.",
                    None,
                );
            }
            return;
        }
        // The `await e` operand's row anchors at `e`, but the oracle's
        // `.at(expression)` is the AwaitExpression — the `await` keyword
        // through the operand's own end (`reportTypeError`,
        // rtv.dart:155-205). `await_operand_extents` carries the
        // `(keyword end, end-token start)` of every await that closed
        // inside this frame: an operand row that starts inside the extent
        // and closes over its end token is that operand, so the staged
        // span re-anchors at the widest enclosing await (a nested
        // `await await e` takes the outermost keyword's start — the
        // official's outermost AwaitExpression node). The verdict's own
        // lookups keep the operand's span — the untyped-literal and
        // cast-rewrite maps key the row, not the wrapper.
        let operand_range = range;
        let mut range = range;
        let mut widest: Option<(u32, u32)> = None;
        for &(operand_start, end_start) in &self.await_operand_extents {
            // The row is the await's operand when it sits strictly inside
            // the recorded extent — the head row's span need not reach the
            // operand's own end (`await s()`'s `s` row is one column, so
            // the row's end is the operand's head, not its `)`). A nested
            // `await await e` matches every enclosing extent, and the
            // minimum start lands on the outermost keyword — the
            // official's outermost AwaitExpression; the matching extent's
            // own end stands in for the row's (the whole expression end).
            if operand_start <= range.offset && range.end() <= end_start {
                widest = Some(match widest {
                    Some((w, e)) if w <= operand_start => (w, e),
                    _ => (operand_start, end_start),
                });
            }
        }
        if let Some((operand_start, end_start)) = widest {
            range = SourceRange {
                offset: operand_start.saturating_sub(5), // `await` keyword
                length: end_start - operand_start.saturating_sub(5),
            };
        }
        // An unannotated collection literal's row carries no type of its
        // own: the host's declared return is the context that resolves it
        // (`typed_literal_resolver.dart` `_fromContextType` — the context
        // decides list/set/map, an empty `{}` adopts a Map/Set context and
        // defaults to a map without one).
        let from = match self.untyped_literals.get(&operand_range.offset) {
            Some(&kind) => self.literal_return_ty(kind, to),
            None => from,
        };
        // The declared host's own NamedType resolution gates the whole check
        // (`named_type_resolver.dart` `_resolveToElement`: a head that misses
        // or names a non-type element leaves the annotation `InvalidType`,
        // and the return verifier asks no assignability question of an
        // invalid host). `Null` into any non-nullable named target is
        // decidable-false from nullability alone, so an unresolved head must
        // gate before the matrix — the gated-off `augment`-typed getters
        // emit no `return_of_invalid_type` (#1445).
        if let Ty::Named { name: head, .. } = to
            && !self.named_type_is_type(head)
        {
            return;
        }
        // An `as` expression's static type is its resolved target — the
        // same cast-face substitution the teardown applies
        // (`filter_staged`'s `cast_rewrites` join): `return null as T`
        // inside a `void` host would otherwise ask the `Null → void`
        // question (always true, never staged) and miss the `T → void`
        // verdict the oracle's `.at(expression)` reports. The parse-time
        // operand type is the cast operand's face; the expression's own
        // face is the rewritten target, so it substitutes BEFORE the
        // invalid-source gate and the verdict below.
        let from = self
            .cast_rewrites
            .iter()
            .find(|(cast, _)| {
                cast.offset <= operand_range.offset && operand_range.end() <= cast.end()
            })
            .map(|(_, ty)| *ty)
            .unwrap_or(from);
        // The invalid-source twin: a written value type whose head resolves
        // to no type is `InvalidType` — assignable to everything, so the
        // return matrix never asks (probed 3.47.1: `return parent;` with
        // `AstNode? parent` against an `int` host stays silent).
        if self.written_ty_invalid(from) {
            return;
        }
        // The constructor face closes the nominal blind spot: a factory's
        // return check reads the declared container faces (`int` from a
        // `C`-typed factory, one class from another), so a pair the written
        // matrix suppresses re-decides through the declared-subtype
        // negative.
        let mut verdict = self.return_verdict(from, to, from_literal, is_async);
        if verdict.is_none() && ctor {
            verdict = self.declared_subtype_negative(&from, &to);
        }
        if verdict == Some(false) {
            self.stage(
                StagedKind::Return {
                    host,
                    name,
                    display,
                    ctor,
                    is_async,
                },
                range,
                from,
                to,
                from_literal,
            );
        }
    }

    /// The closure body's return check (`rtv.dart:160-165`'s `isClosure`
    /// arm): the literal's return type is context-inferred, never
    /// declared, so the verdict asks the context return type the model
    /// knows ([`Engine::closure_context_return`]) and stages under the
    /// closure host — the teardown's promotion join and re-decision matrix
    /// are the declared path's, the message face is the closure's
    /// (`return_of_invalid_type_from_closure`). No decidable context stays
    /// the under-report posture (the oracle's remaining inference channels
    /// — an argument formal's signature, a generic method's solved `T` —
    /// are not this layer's knowledge).
    fn check_closure_return(
        &mut self,
        from: Ty,
        range: SourceRange,
        from_literal: bool,
        is_async: bool,
        generator: bool,
    ) {
        // The generator/bare early-returns mirror the declared path above
        // (`verifyReturnStatement`:66-73 applies to every executable, the
        // closure's `return e;` included — `() sync* { return 3; }` is the
        // parser's `generatorReturnsValue` row, and a generator closure's
        // bare `return;` reports nothing).
        let bare = from == Ty::Void
            && self
                .source
                .as_bytes()
                .get(self.byte_of(range.offset))
                .is_some_and(|b| *b == b';');
        if generator {
            if !bare {
                let anchor = self.generator_return_anchor(range);
                self.emit_compile_time(
                    "return_in_generator",
                    "RETURN_IN_GENERATOR",
                    anchor,
                    "Can't return a value from a generator function that uses the 'async*' or 'sync*' modifier.",
                    Some(
                        "Try replacing 'return' with 'yield', using a block function body, or changing the method body modifier.",
                    ),
                );
            }
            return;
        }
        let Some(to) = self.closure_context_return() else {
            return;
        };
        if bare {
            let t_eff = if is_async {
                future_value_ty(self.strings, &self.boundary.pool(), to)
            } else {
                to
            };
            let ok = matches!(t_eff, Ty::Void | Ty::Dynamic | Ty::Null | Ty::Unknown)
                || matches!(t_eff, Ty::Named { name: head, .. } if !self.named_type_is_type(head));
            if !ok {
                let keyword = self.return_keyword_range(range);
                self.emit_compile_time(
                    "return_without_value",
                    "RETURN_WITHOUT_VALUE",
                    keyword,
                    "The return value is missing after 'return'.",
                    None,
                );
            }
            return;
        }
        // The declared path's untyped-literal arm (`_fromContextType`): a
        // collection literal body resolves against the context return
        // before the assignability ask, so `[3]` under `List<String>`
        // reads `List<String>` and the verdict stays clean — the element
        // check itself reports the leaf.
        let from = match self.untyped_literals.get(&range.offset) {
            Some(&kind) => self.literal_return_ty(kind, to),
            None => from,
        };
        // The declared path's context gate: an unresolved written head
        // (`named_type_resolver.dart`'s InvalidType annotation) asks no
        // assignability question.
        if let Ty::Named { name: head, .. } = to
            && !self.named_type_is_type(head)
        {
            return;
        }
        if self.return_verdict(from, to, from_literal, is_async) == Some(false) {
            self.stage(
                StagedKind::Return {
                    host: ExecKind::Closure,
                    // The closure template has no name slot; the staging's
                    // third placeholder renders the interned empty string.
                    name: self.intern(""),
                    display: None,
                    ctor: false,
                    is_async,
                },
                range,
                from,
                to,
                from_literal,
            );
        }
    }

    /// The compile-time-error emit for the return family's own codes —
    /// rows the [`SemanticCode`] table does not carry
    /// (`return_without_value`, `return_in_generator`,
    /// `return_in_generative_constructor` are compile-time-error rows of
    /// the pinned registry, `analyzer-codes.tsv`), pushed with their
    /// verbatim oracle message and correction, if any.
    fn emit_compile_time(
        &mut self,
        name: &'static str,
        machine: &'static str,
        range: SourceRange,
        message: &'static str,
        correction: Option<&'static str>,
    ) {
        self.report.diagnostics.push(Diagnostic::new(
            Severity::Error,
            CodeInfo::new(name, machine, DiagnosticType::CompileTimeError, true),
            message,
            self.file,
            range,
            correction.map(str::to_string),
        ));
    }

    /// The `return` keyword's own span behind a bare `return;` (the
    /// `returnWithoutValue` anchor — `.at(statement.returnKeyword)`,
    /// return_type_verifier.dart:305-307): the listener hands the `;`'s
    /// span, so the keyword is the six `return` bytes ahead of it after
    /// skipping whitespace. Anything unexpected keeps the handed span.
    fn return_keyword_range(&self, range: SourceRange) -> SourceRange {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(range.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor >= 6 && &bytes[cursor - 6..cursor] == b"return" {
            SourceRange {
                offset: self.utf16_of(cursor - 6),
                length: 6,
            }
        } else {
            range
        }
    }

    /// The generator value-return's anchor (the parser's
    /// `handleInvalidStatement(begin, generatorReturnsValue)` —
    /// parser_impl.dart:6628/:6855): `begin` is the `=>` FUNCTION token
    /// for an arrow body, the `return` keyword for a statement — both sit
    /// ahead of the expression's span after whitespace. Neither spelled
    /// keeps the expression's own span.
    fn generator_return_anchor(&self, range: SourceRange) -> SourceRange {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(range.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor >= 2 && &bytes[cursor - 2..cursor] == b"=>" {
            return SourceRange {
                offset: self.utf16_of(cursor - 2),
                length: 2,
            };
        }
        if cursor >= 6 && &bytes[cursor - 6..cursor] == b"return" {
            return SourceRange {
                offset: self.utf16_of(cursor - 6),
                length: 6,
            };
        }
        range
    }

    /// The downward tear-off instantiation behind a generic source
    /// function type assigned to a non-generic target
    /// (`_isFunctionSubtypeOf` never reaches the pair — its
    /// `relateTypeParameters` count gate, type_system.dart:1585-1590):
    /// solve the source's own type parameters from the target's
    /// structurally-paired types, then run the subtype checks over the
    /// substituted signature. `Some(bool)` decides; `None` leaves the
    /// ordinary signature verdict in place (the under-report direction —
    /// an unsolved nested shape the name-level model cannot bind is not a
    /// definite rejection).
    ///
    /// The candidate parameter spellings are the signature leaves that
    /// appear only on the source side — the stash's named entries when it
    /// captured them, else the bare `Named` heads present in the source
    /// signature but absent from every target type. A candidate appearing
    /// at no pairable position proves nothing (`List<T>` under an
    /// unrelated target head stays unbound) and keeps `None`.
    fn instantiated_fn_sig_subtype(&self, from: &FnVarSig, to: &FnVarSig) -> Option<bool> {
        // The arity shape the instantiation cannot change: subtype needs
        // every target parameter covered — `from.positional.len() >=
        // to.positional.len()`, `from.required_positional <=
        // to.required_positional`, and every target named formal present
        // on the source (subtype.dart:419-431 — an instantiation only
        // rewrites the type variables).
        if from.positional.len() < to.positional.len()
            || from.required_positional > to.required_positional
            || to
                .named
                .iter()
                .any(|(name, _, _)| !from.named.iter().any(|(n, _, _)| n == name))
        {
            return Some(false);
        }
        // The candidate type-parameter spellings: the stash's named
        // entries when they exist (the pooled `Ty::Function` shape keeps
        // only a count, `StringId::NONE`), otherwise every bare `Named`
        // head the source signature spells that no target type spells —
        // a name on both sides is a shared declared type, never a
        // parameter.
        let mut candidates: Vec<StringId> = from
            .params
            .iter()
            .filter_map(|(name, _)| (*name != StringId::NONE).then_some(*name))
            .collect();
        if candidates.is_empty() {
            let mut to_heads: FxHashSet<StringId> = FxHashSet::default();
            for ty in to
                .positional
                .iter()
                .chain(to.named.iter().map(|(_, t, _)| t))
                .chain(std::iter::once(&to.return_ty))
            {
                self.collect_named_heads(*ty, &mut to_heads);
            }
            let mut from_heads: FxHashSet<StringId> = FxHashSet::default();
            for ty in from
                .positional
                .iter()
                .chain(from.named.iter().map(|(_, t, _)| t))
                .chain(std::iter::once(&from.return_ty))
            {
                self.collect_named_heads(*ty, &mut from_heads);
            }
            candidates = from_heads
                .into_iter()
                .filter(|head| !to_heads.contains(head))
                // A name that resolves to a real type — a core primitive,
                // a declared container, an in-scope type parameter — is
                // never the signature's own type variable: it just happens
                // to appear on the source side only, and binding it would
                // accept instantiations the oracle's inference rejects.
                .filter(|head| {
                    !self.members.primitive_ids.contains(head)
                        && !self.type_params.contains(head)
                        && !self.pending_type_params.contains(head)
                        && resolve_container(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            self.file,
                            *head,
                        )
                        .is_none()
                })
                .collect();
        }
        if candidates.is_empty() {
            return None;
        }
        let candidate_set: FxHashSet<StringId> = candidates.iter().copied().collect();
        // Bind each candidate from its structurally-paired target type: a
        // bare `Named` leaf binds the whole target type, a shared head
        // recurses into its arguments (`List<T>` against `List<int>` binds
        // `T := int`); anything else cannot bind through this model and
        // leaves the candidate unbound. A conflicting re-bind fails the
        // instantiation (`T Function<T>(T, T)` against `int
        // Function(int, String)` wants `T` to be two types at once).
        let mut bindings: FxHashMap<StringId, Ty> = FxHashMap::default();
        let mut failed = false;
        for (from_ty, to_ty) in from.positional.iter().zip(to.positional.iter()) {
            self.bind_sig_leaves(*from_ty, *to_ty, &candidate_set, &mut bindings, &mut failed);
        }
        for (name, to_ty, _) in &to.named {
            if let Some((_, from_ty, _)) = from.named.iter().find(|(n, _, _)| n == name) {
                self.bind_sig_leaves(*from_ty, *to_ty, &candidate_set, &mut bindings, &mut failed);
            }
        }
        self.bind_sig_leaves(
            from.return_ty,
            to.return_ty,
            &candidate_set,
            &mut bindings,
            &mut failed,
        );
        if failed || bindings.is_empty() || candidates.iter().any(|c| !bindings.contains_key(c)) {
            return None;
        }
        // A bound-violating solve is no instantiation the oracle accepts:
        // `String Function(String) f = C()` over `T call<T extends num>(T)`
        // would bind `T := String`, which `extends num` refuses — the
        // tear-off falls back to `instantiateTypeFormalsToBounds` (`num
        // Function(num)` is the reported face). `None` keeps that
        // fallback open; `Some(false)` would wrongly hard-reject a pair
        // the bounds face still assigns.
        for (param, bound) in from.params.iter() {
            if let (Some(bound_ty), Some(solved)) = (*bound, bindings.get(param)) {
                match self.assignable(*solved, bound_ty, false) {
                    Some(false) => return None,
                    Some(true) | None => {}
                }
            }
        }
        // The substituted signature re-runs the subtype checks: return
        // covariance (a `void` target is Right Top — skipped like
        // [`Engine::fn_sig_subtype`]'s own guard), the target's
        // positional/named contravariance. One undecidable slot
        // suppresses the whole (`None`).
        if to.return_ty != Ty::Void {
            match self.slot_subtype(self.subst_ty(from.return_ty, &bindings), to.return_ty) {
                Some(false) => return Some(false),
                Some(true) => {}
                None => return None,
            }
        }
        for (index, to_ty) in to.positional.iter().enumerate() {
            let from_ty = self.subst_ty(from.positional[index], &bindings);
            match self.slot_subtype(*to_ty, from_ty) {
                Some(false) => return Some(false),
                Some(true) => {}
                None => return None,
            }
        }
        for (name, to_ty, _) in &to.named {
            let Some((_, from_ty, _)) = from.named.iter().find(|(n, _, _)| n == name) else {
                return Some(false);
            };
            let from_ty = self.subst_ty(*from_ty, &bindings);
            match self.slot_subtype(*to_ty, from_ty) {
                Some(false) => return Some(false),
                Some(true) => {}
                None => return None,
            }
        }
        Some(true)
    }

    /// The bare `Named` heads inside a signature leaf — the candidates the
    /// instantiation walk scans.
    fn collect_named_heads(&self, ty: Ty, out: &mut FxHashSet<StringId>) {
        match ty {
            Ty::Named { name, args, .. } => {
                out.insert(name);
                let args = {
                    let pool = self.boundary.pool();
                    pool.get(args).to_vec()
                };
                for arg in args {
                    self.collect_named_heads(arg, out);
                }
            }
            Ty::Record { shape, .. } => {
                let shape = {
                    let pool = self.boundary.pool();
                    pool.record_shape(shape).clone()
                };
                for field in shape
                    .positional
                    .iter()
                    .chain(shape.named.iter().map(|(_, t)| t))
                {
                    self.collect_named_heads(*field, out);
                }
            }
            Ty::Function { shape, .. } => {
                let (return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                self.collect_named_heads(return_ty, out);
                for ty in positional.iter().chain(named.iter().map(|(_, t)| t)) {
                    self.collect_named_heads(*ty, out);
                }
            }
            _ => {}
        }
    }

    /// The pairwise binding walk: `from_ty`'s bare candidate `Named` leaf
    /// takes `to_ty` wholesale; a shared head recurses into its arguments
    /// pairwise. A re-bind to a different type marks the instantiation
    /// unsolvable.
    fn bind_sig_leaves(
        &self,
        from_ty: Ty,
        to_ty: Ty,
        candidates: &FxHashSet<StringId>,
        bindings: &mut FxHashMap<StringId, Ty>,
        failed: &mut bool,
    ) {
        if *failed {
            return;
        }
        match (from_ty, to_ty) {
            (
                Ty::Named {
                    name,
                    args: ArgListId::EMPTY,
                    ..
                },
                _,
            ) if candidates.contains(&name) => {
                if let Some(bound) = bindings.get(&name) {
                    if *bound != to_ty {
                        *failed = true;
                    }
                } else {
                    bindings.insert(name, to_ty);
                }
            }
            (
                Ty::Named {
                    name: from_name,
                    args: from_args,
                    ..
                },
                Ty::Named {
                    name: to_name,
                    args: to_args,
                    ..
                },
            ) if from_name == to_name => {
                // The pairwise argument walk borrows the pool twice in a
                // row — own the vectors before recursing so no `RefCell`
                // guard crosses a nested `intern_list` borrow.
                let (from_args, to_args) = {
                    let pool = self.boundary.pool();
                    (pool.get(from_args).to_vec(), pool.get(to_args).to_vec())
                };
                for (from_arg, to_arg) in from_args.iter().zip(to_args.iter()) {
                    self.bind_sig_leaves(*from_arg, *to_arg, candidates, bindings, failed);
                }
            }
            (
                Ty::Record {
                    shape: from_shape, ..
                },
                Ty::Record {
                    shape: to_shape, ..
                },
            ) => {
                let (from_shape, to_shape) = {
                    let pool = self.boundary.pool();
                    (
                        pool.record_shape(from_shape).clone(),
                        pool.record_shape(to_shape).clone(),
                    )
                };
                for (from_field, to_field) in
                    from_shape.positional.iter().zip(to_shape.positional.iter())
                {
                    self.bind_sig_leaves(*from_field, *to_field, candidates, bindings, failed);
                }
                for ((from_name, from_field), (to_name, to_field)) in
                    from_shape.named.iter().zip(to_shape.named.iter())
                {
                    if from_name != to_name {
                        return;
                    }
                    self.bind_sig_leaves(*from_field, *to_field, candidates, bindings, failed);
                }
            }
            _ => {}
        }
    }

    /// The context return type a function literal's body checks against:
    /// the innermost open capture's matched context `K` — latched at
    /// `begin_function_expression` from whichever channel supplied it
    /// (the declaration's folded written type, the assignment target's
    /// declared type, the argument slot's formal type, or the enclosing
    /// return position's function shape — [`Engine::closure_context_ty`]),
    /// gated on the literal's own type-parameter count matching `K`'s
    /// (the upstream `typeContext is FunctionType` equality arm). Its
    /// return is the body's `R`. Every unmatched context — dynamic, bare
    /// `Function`, a count-mismatched generic, an argument formal the
    /// callee tables cannot see — answers `None`, the under-report
    /// posture.
    pub(crate) fn closure_context_return(&self) -> Option<Ty> {
        let capture = self.closure_caps.last()?;
        let ctx = capture.ctx?;
        if !self.closure_ctx_matches(ctx, capture.type_params) {
            return None;
        }
        self.function_shape_return(ctx)
    }

    /// An unannotated collection literal's static type in a return
    /// position (`typed_literal_resolver.dart`): a context shaped like the
    /// literal's own family donates its type wholesale (an Iterable-not-Map
    /// context resolves `[…]` and `{...}` set shapes to itself, a Map
    /// context resolves map shapes; the greatest-closure shortcut the
    /// resolver documents), everything else falls to the no-context
    /// defaults — `List<dynamic>`, `Set<dynamic>`, `Map<dynamic,
    /// dynamic>`.
    pub(crate) fn literal_return_ty(&self, kind: UntypedLiteral, to: Ty) -> Ty {
        if let Ty::Named { name, .. } = to {
            let heads = &self.collection_heads;
            let (list, set, map, iterable) = (heads[0], heads[1], heads[2], heads[3]);
            let adopts = match kind {
                UntypedLiteral::List => name == list || name == set || name == iterable,
                UntypedLiteral::SetElements => name == set || name == iterable,
                UntypedLiteral::MapEntries => name == map,
                // An empty `{}` resolves to the context's shape whichever
                // collection head it names.
                UntypedLiteral::EmptyBraces => name == map || name == set || name == iterable,
            };
            if adopts {
                return to;
            }
        }
        let (list, set, map) = (
            self.collection_heads[0],
            self.collection_heads[1],
            self.collection_heads[2],
        );
        let dynamic = Ty::Dynamic;
        match kind {
            UntypedLiteral::List => Ty::Named {
                name: list,
                nullable: false,
                args: self.boundary.intern_list(&[dynamic]),
            },
            UntypedLiteral::SetElements => Ty::Named {
                name: set,
                nullable: false,
                args: self.boundary.intern_list(&[dynamic]),
            },
            UntypedLiteral::MapEntries | UntypedLiteral::EmptyBraces => Ty::Named {
                name: map,
                nullable: false,
                args: self.boundary.intern_list(&[dynamic, dynamic]),
            },
        }
    }

    /// Whether a declared named-type head resolves to an actual type — the
    /// `_resolveToElement` face (named_type_resolver.dart:586-600): a head
    /// that misses (`undefined_class`) or names a non-type element
    /// (`notAType`) leaves the annotation `InvalidType`. A namespaced name
    /// with no captured declaration (the SDK projection's names-only stubs)
    /// conservatively counts as a type — the under-report direction; the
    /// primitives and the in-scope type parameters are always types. The
    /// enclosing container's own members are never types
    /// (`InstanceScope.lookup`, the same chain [`Engine::type_resolution`]
    /// walks). The
    /// teardown re-decision of an already-staged check re-runs
    /// [`Engine::return_verdict`] directly: this gate lives at the staging
    /// site, where the frame's type parameters are still in scope.
    pub(crate) fn named_type_is_type(&self, name: StringId) -> bool {
        if self.members.primitive_ids.contains(&name)
            || self.type_params.contains(&name)
            || self.pending_type_params.contains(&name)
        {
            return true;
        }
        if let Some((container, _)) = self.container_context()
            && self
                .index
                .get(self.file)
                .and_then(|f| f.container(container))
                .is_some_and(|c| c.members.get(&name).is_some())
        {
            return false;
        }
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => Self::decl_is_type(decl.kind),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => self
                .index
                .get(file)
                .and_then(|f| f.decl(name))
                .map_or(true, |decl| Self::decl_is_type(decl.kind)),
            // A bare prefix is not a type; an ambiguous set resolves to the
            // oracle's `MultiplyDefinedElement` — `InvalidType`.
            ScopedHit::Found(NamespaceEntry::Prefix) | ScopedHit::Ambiguous(_) => false,
            ScopedHit::NotFound => false,
        }
    }

    /// [`Self::named_type_is_type`] resolved in a FOREIGN file's scope: the
    /// extension-credit gate asks whether an extension's `on` head resolves
    /// where the extension was DECLARED (the spelling need not be visible to
    /// the importing file — `on XmlNode` in ancestors.dart applies in files
    /// that never import node.dart). The current container's member-shadow
    /// check is the diagnosing scope's own; a foreign asking scope keeps the
    /// declaration-table and namespace probes alone.
    pub(crate) fn named_type_is_type_in(&self, file: FileId, name: StringId) -> bool {
        if self.members.primitive_ids.contains(&name) {
            return true;
        }
        let lib = self.graph.library_of_file(file).unwrap_or(self.lib);
        match lookup_scoped(self.graph, self.namespaces, self.index, lib, file, name) {
            ScopedHit::Local(decl) => Self::decl_is_type(decl.kind),
            ScopedHit::Found(NamespaceEntry::Decl(owner)) => self
                .index
                .get(owner)
                .and_then(|f| f.decl(name))
                .map_or(true, |decl| Self::decl_is_type(decl.kind)),
            ScopedHit::Found(NamespaceEntry::Prefix) | ScopedHit::Ambiguous(_) => false,
            ScopedHit::NotFound => false,
        }
    }

    /// The declaration kinds a named type can name.
    pub(crate) fn decl_is_type(kind: MicroKind) -> bool {
        matches!(
            kind,
            MicroKind::Class
                | MicroKind::Enum
                | MicroKind::Mixin
                | MicroKind::Extension
                | MicroKind::TypeAlias
                | MicroKind::TypeParam
        )
    }

    /// The return verdict matrix, shared by the parse-time stage and the
    /// teardown re-decision. A synchronous body (`rtv.dart:209-249`):
    /// `void` hosts accept `void`/`dynamic`/`Null`; `void`/`dynamic`
    /// expressions only fit `void`/`dynamic` hosts; everything else defers
    /// to the assignability decision (the return expression is the
    /// literal's context, so `from_literal` feeds the conversion —
    /// `double f() => 1;` is clean). Bare `return;` is a `void`
    /// expression. An `async` body never reaches these branches upstream
    /// (`isSynchronous` early-returns) — it re-decides through
    /// [`Engine::async_return_verdict`].
    pub(crate) fn return_verdict(
        &self,
        from: Ty,
        to: Ty,
        from_literal: bool,
        is_async: bool,
    ) -> Option<bool> {
        let verdict = if is_async {
            self.async_return_verdict(from, to, from_literal)
        } else {
            match to {
                Ty::Void => match from {
                    // `Never <: void` holds (Right Bottom — `return throw e;`).
                    Ty::Void | Ty::Dynamic | Ty::Null | Ty::Never => Some(true),
                    Ty::Unknown => None,
                    Ty::Named { .. } | Ty::Record { .. } | Ty::Function { .. } => Some(false),
                },
                Ty::Unknown => None,
                // A declared return that spells a type parameter is a
                // genuine `TypeParameterType` target (subtype.dart's Left
                // Variable arm — `S <: T` is `false` for every interface
                // source): `return T` asks `Type <: T`, `return 3` asks
                // `int <: T`, and both reject. The file's decl table
                // decides param-ness — pass 1 sees the live frame's sets;
                // the teardown re-decision runs with every frame popped,
                // so only the file-scoped `MicroKind::TypeParam` decl
                // survives. This arm is safe to read the decl table BECAUSE
                // `to` is always a declared return here — never a solved
                // formal or a bound-substituted alias slot, which is why
                // [`Engine::assignable`] restricts its own param detection
                // to the live sets.
                Ty::Named { name: to_name, .. } if self.return_target_is_type_param(to_name) => {
                    self.return_param_source_verdict(from, to)
                }
                _ => match from {
                    // A `void` return expression fits only the
                    // `void`/`dynamic` hosts (rtv.dart:222-226 — the
                    // official reports whenever `T` is not
                    // `_isVoidDynamic`).
                    Ty::Void => Some(matches!(to, Ty::Void | Ty::Dynamic)),
                    // A `dynamic` return expression is a subtype only of the
                    // top-type hosts; the strict-casts mode rejects the rest
                    // (the same overlay [`Engine::assignable`] applies — the
                    // oracle's return verifier consults the identical
                    // `isAssignableTo` gate).
                    Ty::Dynamic => Some(matches!(to, Ty::Dynamic | Ty::Void) || !self.strict.casts),
                    _ => self.assignable(from, to, from_literal),
                },
            }
        };
        // The declared-subtype negative closes the nominal blind spot for
        // EVERY return host, not only constructors (rtv.dart asks the one
        // `isAssignableTo` over the resolved pair whatever the declaring
        // element): two declared container heads with no supertype path
        // between them — a `Future<int>` expression against an `int` host
        // — is the oracle's report, and the matrix alone suppresses the
        // distinct-head pair. The negative decides only on a complete
        // hierarchy walk (a primitive source against a declared target,
        // or a closed closure that never names the target), so the
        // under-report direction survives on every open question.
        verdict.or_else(|| self.declared_subtype_negative(&from, &to))
    }

    /// Whether a declared return's head names a type parameter — the live
    /// sets at pass 1, the file's decl table once the frames are popped
    /// (the teardown re-decision). Reading the decl table is safe only on
    /// this declared-return path: `to` is provably the executable's own
    /// return type, so a `MicroKind::TypeParam` hit is a real parameter
    /// target, never a foreign callee's solved formal or a typedef's
    /// bound slot.
    fn return_target_is_type_param(&self, to_name: StringId) -> bool {
        self.declared_head_is_type_param(to_name)
    }

    /// The declared-head param-ness predicate shared by the declared-return
    /// and write-target faces: the live sets at pass 1, the file's decl
    /// table once the frames are popped (the teardown re-decision). The
    /// decl-table leg is safe on both faces that consult it — a declared
    /// return's `to` is provably the executable's own return type, and an
    /// assignment's `to` is the slot's own `writeType` — so a
    /// `MicroKind::TypeParam` hit is a real parameter of the enclosing
    /// declaration, never a foreign callee's solved formal or a typedef's
    /// bound slot.
    fn declared_head_is_type_param(&self, name: StringId) -> bool {
        self.type_params.contains(&name)
            || self.pending_type_params.contains(&name)
            || self
                .index
                .get(self.file)
                .and_then(|f| f.decl(name))
                .is_some_and(|decl| decl.kind == MicroKind::TypeParam)
    }

    /// The `S <: T` verdict over a parameter target (subtype.dart's Left
    /// Variable arm): an interface, record, or function source never
    /// subtypes a bare parameter — `Some(false)`. A parameter SOURCE
    /// (`S` also a parameter — the promoted-bound or reflexivity arm)
    /// stays undecidable on the name-level lattice (`None`). `Null`,
    /// `Never`, `dynamic`, `void`, and `Unknown` sources keep the generic
    /// matrix's own answers — `Null <: T` rejects on the non-nullable
    /// param, `Never` and the top faces ride their own rules — so they
    /// defer to [`Engine::assignable`] over the real target rather than
    /// take the interface refusal.
    fn return_param_source_verdict(&self, from: Ty, to: Ty) -> Option<bool> {
        match from {
            Ty::Named {
                name: from_name, ..
            } if self.return_target_is_type_param(from_name) => None,
            Ty::Named { .. } | Ty::Record { .. } | Ty::Function { .. } => Some(false),
            _ => self.assignable(from, to, false),
        }
    }

    /// The `async` body's return verdict matrix (`rtv.dart:258-289`): the
    /// declared `T` reads through [`future_value_ty`] (`T_v`), the
    /// expression's `S` through the recursive `flatten` (type_system.dart
    /// :425-471, [`flatten_ty`]) — the two projections are deliberately
    /// different shapes. (a) a `void` `T_v` accepts `void`/`dynamic`/`Null`
    /// `flatten_S` (:258-264); (b) a `void` `flatten_S` accepts
    /// `void`/`dynamic` `T_v` (:266-272); (c) otherwise the report needs
    /// BOTH questions to fail — `S` unassignable to `T_v` AND `flatten_S`
    /// no subtype of `T_v` (:274-289; `Future<int> f() async => DB();` is
    /// clean through the subtype leg even though `DB` never fits the
    /// declared `Future<DB>` head). The recovered `Unknown` face counts as
    /// accepted on both ok-sets (`_isVoidDynamic`/`_isVoidDynamicOrNull`,
    /// :337-344, treat `InvalidType` the same) and stays an undecidable
    /// `None` in the assignability question — under-report, the sync
    /// matrix's own convention. The message placeholders keep the raw `S`
    /// and the declared `T` (`reportTypeError`, :155-205) — the
    /// projections are verdict-side only.
    pub(crate) fn async_return_verdict(
        &self,
        from: Ty,
        to: Ty,
        from_literal: bool,
    ) -> Option<bool> {
        // Right Bottom: a `throw` expression's `Never` assigns into every
        // declared shape — `Future<void> f() async => throw 'x';` is clean
        // through the subtype leg before the flatten projections ask their
        // own questions (the flatten ok-sets name no `Never` arm).
        if from == Ty::Never {
            return Some(true);
        }
        let t_v = future_value_ty(self.strings, &self.boundary.pool(), to);
        let flatten_s = flatten_ty(self.strings, &self.boundary.pool(), from);
        if t_v == Ty::Void {
            return Some(matches!(
                flatten_s,
                Ty::Void | Ty::Dynamic | Ty::Null | Ty::Unknown
            ));
        }
        if flatten_s == Ty::Void {
            return Some(matches!(t_v, Ty::Void | Ty::Dynamic | Ty::Unknown));
        }
        let verdict = self.assignable(from, t_v, from_literal);
        if verdict == Some(true) || self.name_subtype(flatten_s, t_v) {
            return Some(true);
        }
        // An undecidable direct ask still reports when the declared
        // hierarchy proves `S` fits no `T_v` face (rtv.dart:274-289 asks
        // the element-level `isAssignableTo` — `Future<String>` into a
        // `Future<int>` host's `int` is a closed-world false the
        // written-shape matrix suppresses).
        verdict.or_else(|| self.declared_subtype_negative(&from, &t_v))
    }

    /// An expression's type since `mark`, for the body/return checks, with
    /// the result row's primitive-literal flag (the conversion's
    /// source-side fact); `None` when the window is empty. The window's
    /// result row is its tail — the fold discipline pushes the outermost
    /// expression's row last. A folded creation reads the created
    /// container (`C()`'s row is the fold's sentinel, its container names
    /// the static type — the factory return face `factory D.new() => C();`
    /// asks the nominal question).
    pub(crate) fn single_row_since(&self, mark: usize) -> Option<(Ty, bool, SourceRange)> {
        // `TypeUse` rows are type-annotation residue, not value rows of the
        // expression (`operand_row` already skips them from the tail): a
        // recovered `is` operand's reparsed type (`<synthetic> is (x-1)`)
        // leaves them BELOW the folded `bool` row, and the enclosing check
        // asks the expression's own type — exactly the non-TypeUse row.
        // Interpolation reads are the same residue class (#1823): a
        // string literal's `$name` child rows join the window below the
        // literal's completion row, and the oracle's expression is the
        // `StringInterpolation` node — its type is the literal's, so the
        // child reads never answer the enclosing check.
        // The window's result row is its tail — the fold discipline pushes
        // the outermost expression's row last (a member merge leaves the
        // member row, a cascade its target-typed value stand-in, a
        // parenthesized group its own row), so a multi-row window answers
        // the expression's static type through the same single-row arms
        // applied to that tail (`return B().x` asks the member face,
        // `return a ?? b` the join's right operand — the under-report
        // direction keeps the composite's own type when the arms cannot
        // spell it).
        let row = self
            .rows
            .iter()
            .skip(mark)
            .filter(|row| !matches!(row.shape, RowShape::TypeUse(..)))
            .filter(|row| !self.interpolation_child_rows.contains(&row.range.offset))
            .next_back()
            .cloned()?;
        let ty = if row.ty == Ty::Never {
            // The throw retype (`handle_throw_expression`): the
            // expression's static type is `Never` regardless of the
            // operand's shape — the oracle's bottom type decides every
            // downstream assignability ask (`Never f() => throw Object();`
            // stays silent), so the Created-name read below must not
            // resurrect the operand's type.
            Ty::Never
        } else if let Some(result) = self.invoked_callee_result(&row) {
            // The send's own static type — the `expression_row_ty` mirror
            // for the body/return window: an invocation row keeps the
            // callee's DECLARED type (or nothing), never the send's
            // result, so `int f() { return g(); }` asks `String` against
            // `int` exactly like the initializer position does
            // (send_result.rs's own contract).
            result
        // A creation send (`C()`, `C<int>()`) keeps the receiver face: its
        // static type is the instantiated head, which the shape match below
        // resolves through `creation_row_ty`. The callee walk would answer
        // the constructor declaration's absent `return_ty` (`Ty::UNKNOWN`)
        // and silence every return check on a created value.
        } else if row.intent == Intent::Invoke
            && row.ty == Ty::UNKNOWN
            && !matches!(row.shape, RowShape::Receiver(Receiver::Created { .. }))
        {
            self.callee_return_ty(&row)
        } else if row.ty == Ty::UNKNOWN
            && let Some(sig) = self.tear_off_sig(&row)
        {
            // A bare function-name expression is the tear-off: the
            // return/arrow-body ask reads the declaration's own
            // `FunctionType` ([`Self::tear_off_sig`]'s element face).
            self.fn_sig_ty(&sig)
        } else {
            match &row.shape {
                RowShape::Receiver(Receiver::Created { name, kind, .. })
                    if *kind != ContainerKind::Extension =>
                {
                    // The expression's static type is the instantiated head:
                    // `Box<int>()` is `Box<int>`, not the bare fold-sentinel
                    // `Box` — the args-aware assignability matrix decides the
                    // difference (#2838's raw-type arm reads a raw head as
                    // its to-bounds defaults, which reports a written
                    // creation against its own instantiation).
                    self.creation_row_ty(*name, false, row.range.offset)
                        .filter(|ty| *ty != Ty::UNKNOWN)
                        .unwrap_or_else(|| self.instantiated_head_ty(Ty::named(*name)))
                }
                _ => row.ty,
            }
        };
        // A `?.`-merged expression reads the short-circuit's nullable
        // result (`return a?.b` asks `B?` against the declared return —
        // the same member-offset latch the `var`-inference arm re-arms in
        // [`Engine::initializer_static_ty`]). An operator-merged row keeps
        // its operator result, and a row inside a statically-`bool`
        // composite keeps the composite's `bool` — `x?.m == true`'s
        // expression type is the `==`'s `bool`, never `bool?`.
        let ty = if self.null_aware_members.contains(&row.range.offset)
            && !matches!(row.intent, Intent::Operator(..))
            && !self.bool_decided_covers(&row)
        {
            Self::nullable_form(ty)
        } else {
            ty
        };
        Some((
            ty,
            Self::primitive_literal(&row, &self.members.primitive_ids),
            row.range,
        ))
    }

    /// The for-in loop variable's staged element type (#2506): the first
    /// written argument of a sequence-implementing iterable head —
    /// `dynamic` for a raw `Iterable`/`Stream` — spelled as a staging
    /// entry (the flow lattice's own shape; the listener interns the
    /// lexeme in the flow name table). The gate mirrors
    /// `check_for_in_iterable`'s own ask (the sequence-face `implements`
    /// walk must answer `Some(true)`), so only iterables the oracle
    /// would type through the element face stage here; everything else
    /// (unresolvable, undecidable, non-sequence, argument-less) keeps
    /// the flow row's own inference.
    pub(crate) fn for_in_element_staging(
        &self,
        mark: usize,
        await_: bool,
    ) -> Option<(Ty, dartq_flow::ty::StagedArg)> {
        let Some((ty, _, _)) = self.single_row_since(mark) else {
            return None;
        };
        // A function-flavored element (`List<C Function(String)>`): the
        // element model poisons the annotation into the plain `Named`
        // return head (rule 0c), so staging it would retag the loop
        // variable a non-function class and the invocation face would
        // mis-fire. The formal's declaration consumed the completed
        // signature into `fn_var_sigs` — the iterable's spelling answers
        // the question here (the under-report direction keeps the fold's
        // own silence).
        let iterable_name = self
            .rows
            .iter()
            .skip(mark)
            .find(|row| !matches!(row.shape, RowShape::TypeUse(..)))
            .map(|row| row.name);
        if iterable_name.is_some_and(|n| self.fn_var_sigs.contains_key(&n)) {
            return None;
        }
        let Ty::Named {
            name,
            nullable: false,
            args,
        } = ty
        else {
            return None;
        };
        let head = if await_ {
            self.intern("Stream")
        } else {
            self.collection_heads[3]
        };
        // The acceptance the oracle's own element inference implies: a
        // static type assignable to `Iterable<dynamic>` has an element.
        // The spelling set answers the core heads directly (the
        // `implements` walk only matches through *unresolved* edges —
        // `List implements Iterable` resolves on an indexed graph and
        // never compares), and the walk stays for the deep chains
        // (typed data and user subtypes).
        let core_spelling = name == head
            || (!await_
                && (name == self.collection_heads[0]
                    || name == self.collection_heads[1]
                    || name == self.collection_heads[3]));
        // The typed-data families: fixed-width views whose element face is
        // a core primitive by declaration (upstream Dart SDK source,
        // sdk/lib/typed_data/typed_data.dart — `Uint8List implements
        // _TypedIntList` → `TypedDataList<int>` → `List<int>`), pinned as
        // a curated fact because the container model carries no clause
        // type arguments for the substitution walk.
        const TYPED_DATA_ELEMENTS: &[(&str, &str)] = &[
            ("Int8List", "int"),
            ("Uint8List", "int"),
            ("Uint8ClampedList", "int"),
            ("Int16List", "int"),
            ("Uint16List", "int"),
            ("Int32List", "int"),
            ("Uint32List", "int"),
            ("Int64List", "int"),
            ("Uint64List", "int"),
            ("Float32List", "double"),
            ("Float64List", "double"),
        ];
        let typed_data_element = (!await_)
            .then(|| {
                TYPED_DATA_ELEMENTS
                    .iter()
                    .find(|&(spelling, _)| self.strings.resolve(name) == *spelling)
                    .map(|(_, element)| Ty::named(self.intern(element)))
            })
            .flatten();
        if let Some(element) = typed_data_element {
            let staged_args = super::listener::staged_arg_tree(
                &self.boundary.args.borrow(),
                self.strings,
                ArgListId::EMPTY,
            )
            .unwrap_or_default();
            let lexeme = match &element {
                Ty::Named { name, .. } => self.strings.resolve(*name).to_string(),
                _ => String::new(),
            };
            return Some((
                element,
                dartq_flow::ty::StagedArg::Named {
                    lexeme,
                    nullable: false,
                    args: staged_args,
                },
            ));
        }
        let accepted = core_spelling
            || implements_interface(
                self.graph,
                self.namespaces,
                self.index,
                self.strings,
                self.lib,
                self.file,
                name,
                head,
                false,
            ) == Some(true);
        if !accepted {
            return None;
        }
        // A bare sequence head's element is `dynamic` (the spec's
        // `Iterable<dynamic>` instantiation — the same face the raw
        // `Iterable` iterable stages); a written instantiation takes its
        // first type argument.
        if args == ArgListId::EMPTY {
            return Some((Ty::Dynamic, dartq_flow::ty::StagedArg::Dynamic));
        }
        let element = self.boundary.args.borrow().get(args).first().copied()?;
        match &element {
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                let staged_args = super::listener::staged_arg_tree(
                    &self.boundary.args.borrow(),
                    self.strings,
                    *args,
                )
                .unwrap_or_default();
                Some((
                    element,
                    dartq_flow::ty::StagedArg::Named {
                        lexeme: self.strings.resolve(*name).to_string(),
                        nullable: *nullable,
                        args: staged_args,
                    },
                ))
            }
            // A raw `Iterable`'s element is `dynamic` (the spec folds the
            // iterable onto `Iterable<dynamic>` — the variable holds that
            // instantiation's element face): the flow read row spells
            // `dynamic`, never the `Unknown` the InvalidType-scrutinee
            // posture keys on.
            Ty::Dynamic => Some((element, dartq_flow::ty::StagedArg::Dynamic)),
            _ => None,
        }
    }

    /// The for-in iterable face (`for_in_of_invalid_type`, the M1
    /// `_checkForEachParts` arm — error_verifier.dart:4890-4958): the
    /// iterable's static type must be assignable to `Iterable<dynamic>`,
    /// or `Stream<dynamic>` under an `await` keyword. The oracle's
    /// suppression order, mapped onto [`Ty`]:
    ///
    /// - `void` rides `checkForUseOfVoidResult`'s own row
    ///   (`use_of_void_result`) — this face never names it;
    /// - `dynamic` reports only under `strict-casts` (otherwise the
    ///   `isNullable` arm absorbs it);
    /// - a nullable source (`T?`, `Null`, `dynamic`) defers to the
    ///   nullable-dereference check — this face stays silent;
    /// - a top type (`dynamic`, `void`, `Object?`) is folded onto the
    ///   sequence type before the assignability ask — always clean;
    /// - [`Ty::Unknown`] (a broken iterable's `InvalidType`) returns
    ///   `None` from `assignable` — suppression.
    ///
    /// Only a definite `Some(false)` emits; an undecidable `None` (an
    /// unindexed edge, a type parameter whose bound the pass can't
    /// resolve) keeps the under-report direction.
    pub(crate) fn check_for_in_iterable(&mut self, mark: usize, await_: bool) {
        let Some((ty, literal, range)) = self.single_row_since(mark) else {
            return;
        };
        // `use_of_void_result`'s row (upstream `_checkForEachParts` →
        // `checkForUseOfVoidResult(node.iterable)` at error_verifier.dart:
        // 4442 — the void use reports and short-circuits the iterable
        // assignability ask). The anchor is the iterable's use-site span
        // (`for (x in f())` reports the whole send).
        if ty == Ty::Void {
            let range = self
                .rows
                .iter()
                .rev()
                .find(|row| row.range == range)
                .map(|row| self.use_site_range(row))
                .unwrap_or(range);
            self.emit(SemanticCode::UseOfVoidResult, range, "", "");
            return;
        }
        // The strict-casts dynamic arm reports ahead of the nullable arm.
        if ty == Ty::Dynamic && self.strict.casts {
            let sequence = if await_ { "Stream" } else { "Iterable" };
            let a0 = self.ty_display(ty);
            self.emit(SemanticCode::ForInOfInvalidType, range, &a0, sequence);
            return;
        }
        // `isNullable` — the nullable-dereference face owns it.
        let nullable = match ty {
            Ty::Null | Ty::Dynamic => true,
            Ty::Named { nullable, .. }
            | Ty::Record { nullable, .. }
            | Ty::Function { nullable, .. } => nullable,
            _ => false,
        };
        if nullable {
            return;
        }
        // `isTop` folds the source onto the sequence type — never a row.
        if matches!(ty, Ty::Void | Ty::Dynamic) {
            return;
        }
        let head = if await_ {
            self.intern("Stream")
        } else {
            self.collection_heads[3]
        };
        let target = Ty::Named {
            name: head,
            nullable: false,
            args: self.boundary.intern_list(&[Ty::Dynamic]),
        };
        // A rejection stages: the teardown join re-decides it against the
        // iterable read's promoted type ([`StagedKind::ForIn`]).
        let sequence = Ty::named(head);
        match self.assignable(ty, target, literal) {
            Some(false) => self.stage(StagedKind::ForIn, range, ty, sequence, literal),
            // The undecidable source side re-asks the core-face interface
            // membership (`implements` semantics, never the entity ask —
            // the sequence head is the core interface itself): a clean
            // supertype chain reaching it stays silent, while a chain with
            // a dead hop (the oracle's InvalidType supertype — the
            // broken-import corpus's `XmlNodeList extends DelegatingList`)
            // never reaches it and reports. An edge resolving behind an
            // unmodeled SDK face keeps the open verdict — the under-report
            // direction.
            None => {
                let Ty::Named {
                    name,
                    nullable: false,
                    ..
                } = ty
                else {
                    return;
                };
                if implements_interface(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.strings,
                    self.lib,
                    self.file,
                    name,
                    head,
                    false,
                ) == Some(false)
                {
                    self.stage(StagedKind::ForIn, range, ty, sequence, literal);
                }
            }
            Some(true) => {}
        }
    }

    /// The declared-container negative for a constructor's return matrix:
    /// both heads are written types and `to` is absent from `from`'s
    /// resolved supertype closure — the same declared-edge evidence the
    /// sealed-supertype contract consumes ([`Engine::closure`]). A
    /// primitive source subtypes no user container (the core types are
    /// final); a container source subtypes a primitive target only through
    /// a written edge the closure carries. `Object` is the universal
    /// supertype and stays out of the question. Everything unresolved
    /// suppresses (the under-report direction).
    /// The assignment face's verdict: the matrix, closed by the
    /// declared-subtype negative for the nominal pairs the matrix cannot
    /// ask. A non-null NAMED source against a distinct-head NAMED target
    /// is a hierarchy question — the nullable-target Right Top arm
    /// answers `true` unconditionally and the non-null pair suppresses —
    /// but `S <: T?` iff `S <: T` (`Null` rides the Right Top leg
    /// separately, so a nullable SOURCE stays true), so the question
    /// projects the target's `?` away and consults the declared closure:
    /// a provably-unrelated pair answers `false` (subtype.dart's nominal
    /// rule — the oracle's `invalid_assignment`), every other pair keeps
    /// the matrix verdict. Scoped to the assignment faces only: the
    /// return/argument/override faces keep their own pinned postures over
    /// synthetic hierarchies. An int-literal source sits out — its
    /// implicit `double` conversion (`double d = 1;`, oracle-legal) is
    /// decided by the matrix's later literal-refinement arms, which a
    /// negative evaluated before them would preempt.
    pub(crate) fn assignment_verdict(&self, from: Ty, to: Ty, from_literal: bool) -> Option<bool> {
        // A type-parameter write target (the slot's `writeType` is `T`/`T?`
        // itself — upstream decides against that parameter type,
        // assignment_expression_resolver.dart `_checkForInvalidAssignment`):
        // subtype.dart's Left Variable arm refuses every interface, record,
        // or function source (`static5_test_06`'s `final U x = const A()`,
        // `test_typeParameter`'s write over `T?`) — no interface subtypes a
        // parameter, whatever its bound (probed 3.13.1: `T extends num` takes
        // `t = 3`, `T extends A` takes `t = a` and even the bound's own
        // subtype `t = b` — the bound rescues nothing). A `Null` source meets
        // only the nullable conjunct (`Null <: T?` holds, `Null <: T` is the
        // report). A parameter SOURCE re-enters [`Self::assignable`], whose
        // same-head matrix decides the reflexive and nullable spellings
        // (`t = t` silent, `T?` into `T` reporting); a bound-chained
        // parameter source (`S extends T`, probed silent) answers through its
        // declared bound (the Left Type Variable Bound arm below), so the
        // bound chain no longer rides the interface refusal. Everything else
        // (`dynamic`, `Never`, `Unknown`, `void`) keeps the generic matrix.
        // The decl-table predicate decides param-ness in BOTH passes — the
        // staged row and the teardown re-decision agree, which the live
        // frame sets alone cannot (empty once the frames pop).
        if let Ty::Named {
            name: to_name,
            nullable: to_nullable,
            ..
        } = to
            && self.declared_head_is_type_param(to_name)
        {
            return match from {
                Ty::Null => Some(to_nullable),
                Ty::Named {
                    name: from_name, ..
                } if from_name == to_name => self.assignable(from, to, false),
                // A distinct parameter source answers through its declared
                // bound (subtype.dart:336-350's Left Type Variable Bound —
                // `S <: T` iff `bound(S) <: T`, and `bound(S) == T` is the
                // same-head reflexive arm), so `S extends T` no longer rides
                // the interface refusal (the over-report this ticket lifts).
                Ty::Named { name: from_name, .. } if self.is_scope_param(from_name) => {
                    match self.param_bound(from_name) {
                        Some(bound) if bound != Ty::Unknown => {
                            self.assignable(bound, to, false)
                        }
                        // No written bound captured: the interface refusal
                        // stands (the under-report-safe side).
                        _ => Some(false),
                    }
                }
                Ty::Named { .. } | Ty::Record { .. } | Ty::Function { .. } => Some(false),
                _ => self.assignable(from, to, false),
            };
        }
        let verdict = self.assignable(from, to, from_literal);
        if verdict == Some(false) || from_literal {
            return verdict;
        }
        // A subtype PROOF against a non-nullable target is final (#2873):
        // the declared-closure negative exists for the nullability Right-Top
        // true (every value assigns into `T?` — the base-type question the
        // projection below re-asks) and for the undecided pairs, never to
        // overturn a proven relation — the `FutureOr<T>` union's value leg
        // proves `Future<Object?> <: FutureOr<Object>` through `Object`, a
        // supertype question the Future→FutureOr declared-head closure
        // cannot see.
        let target_nullable = matches!(
            to,
            Ty::Named { nullable: true, .. }
                | Ty::Record { nullable: true, .. }
                | Ty::Function { nullable: true, .. }
                | Ty::Dynamic
                | Ty::Void
        );
        if verdict == Some(true) && !target_nullable {
            return verdict;
        }
        let negative = match (from, to) {
            (
                Ty::Named {
                    name: from_name,
                    nullable: false,
                    ..
                },
                Ty::Named {
                    name: to_name,
                    args: to_args,
                    ..
                },
            ) if from_name != to_name
                && to_name != self.intern("Object")
                // A `Never` source is Left Bottom (subtype.dart:74-77):
                // `Never <: T1` is proven outright, and the declared walk
                // below — which reads `Never` as an unrelated head with an
                // empty supertype closure — must not overturn the proof
                // (`int? x = neverVal;` is the oracle's clean row).
                && !self.is_never_ty(from)
                // An in-scope type parameter's relation to the target is
                // decided only at the instantiation — the parse-time `T`
                // capture stays under-report (`operator []=`'s `T v`
                // against `int? prop`).
                && !self.type_params.contains(&from_name)
                && !self.pending_type_params.contains(&from_name) =>
            {
                let to_base = Ty::Named {
                    name: to_name,
                    nullable: false,
                    args: to_args,
                };
                self.declared_subtype_negative(&from, &to_base)
            }
            _ => None,
        };
        if negative == Some(false) {
            Some(false)
        } else {
            verdict
        }
    }

    pub(crate) fn declared_subtype_negative(&self, from: &Ty, to: &Ty) -> Option<bool> {
        // The record source arm (`subtype.dart`'s record block): a record
        // type's only interface supertypes are `Object` and `Record`
        // itself — every other named target is a decidable negative
        // (`(int, int) <: int`, `(int, int) <: num`, `(a: 1) <: String`).
        // `FutureOr` is a union, not a head a record cannot meet
        // (`(int, int) <: FutureOr<Object>` holds through the value leg),
        // so it stays open; a target that resolves to no container and is
        // no core primitive could still be an alias or an unseen edge —
        // the under-report direction keeps it undecidable.
        if let Ty::Record { nullable: false, .. } = from
            && let Ty::Named {
                name: to_name,
                nullable: false,
                ..
            } = to
            && *to_name != self.intern("Object")
            && *to_name != self.intern("Record")
            && *to_name != self.intern("FutureOr")
            && (self.members.primitive_ids.contains(to_name)
                || resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    *to_name,
                )
                .is_some())
        {
            return Some(false);
        }
        let (
            Ty::Named {
                name: from_name,
                nullable: false,
                ..
            },
            Ty::Named {
                name: to_name,
                nullable: false,
                ..
            },
        ) = (from, to)
        else {
            return None;
        };
        if from_name == to_name || *to_name == self.intern("Object") {
            return None;
        }
        let from_container = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            *from_name,
        );
        if from_container.is_none() {
            // A primitive (or unmodeled) source subtypes no user container —
            // the core types are final.
            let to_container = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                *to_name,
            );
            return to_container.map(|_| false);
        }
        let to_container = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            *to_name,
        );
        if to_container.is_none() {
            return None;
        }
        let alias_hop =
            |name: StringId, file: FileId, lib: LibraryId| self.alias_clause_hop(name, file, lib);
        let hierarchy = FoldHierarchy {
            graph: self.graph,
            namespaces: self.namespaces,
            index: self.index,
            file: self.file,
            lib: self.lib,
            class_modifiers: self.class_modifiers,
            memo: RefCell::default(),
            in_progress: RefCell::default(),
            alias_head: Some(&alias_hop),
        };
        let (_, from_file) = from_container.unwrap();
        let found = hierarchy
            .closure((from_file, *from_name))
            .iter()
            .any(|(_, name)| *name == *to_name);
        (!found).then_some(false)
    }

    /// Enters an enum constant: its constructor type arguments check against
    /// the enclosing enum's declared type-parameter count.
    pub(crate) fn enter_enum_element(&mut self) {
        self.in_enum_element = self
            .container_context()
            .filter(|(_, kind)| *kind == ContainerKind::Enum)
            .and_then(|(name, _)| self.declared_type_params_of_container(name));
    }

    pub(crate) fn leave_enum_element(&mut self) {
        self.in_enum_element = None;
    }

    // —— pattern/switch machinery (M2 face) ————————————————————————

    /// Appends a leaf to the innermost open pattern frame; outside any
    /// pattern (pattern-assignment RHS, if-case roots) the leaf is dropped
    /// — only switch exhaustiveness consumes case spaces.
    pub(crate) fn pattern_leaf(&mut self, leaf: Restriction) {
        self.switch.pattern_leaf(leaf);
    }

    /// The coverage half of a cast pattern's close (`p as X`, official
    /// `createCastSpace`, upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/exhaustiveness/shared.dart:704-780).
    /// The cast adds no coverage of its own: the SUB-pattern's space is
    /// dispatched over the matched type (already pushed — the last leaf of
    /// the open frame), then
    ///   - a cast type wholly inside that space lets the cast match every
    ///     value (the throw path covers the rest), so the case covers the
    ///     whole matched type;
    ///   - a non-nullable cast type joins the `null` space (the cast throws
    ///     for `null`, so `null` never reaches the sub-pattern).
    /// `case B() as C:` over a sealed `A` therefore covers `B|Null` — the
    /// gap witnesses `C()`, not the cast-left `B()`.
    pub(crate) fn finish_cast_pattern(&mut self, cast_ty: Ty) {
        let world = self.name_world();
        let context = NameTy::from_semantic(
            self.switch
                .switches
                .last()
                .map_or(Ty::Unknown, |c| c.scrutinee),
        );
        let cast = NameTy::from_semantic(cast_ty);
        let Some(frame) = self.switch.pattern_stack.last_mut() else {
            return;
        };
        // No leaf at all is the type-free sub-pattern (`_ as X`, `var x as
        // X`): its coverage is the matched type whole (`createVariableSpace`
        // / the wildcard space).
        let sub = frame.leaves.pop().unwrap_or(Restriction::NamedContext);
        let space = if world.contained_in(cast, &sub, context) {
            Restriction::NamedContext
        } else if NameWorld::is_potentially_nullable(&cast) {
            sub
        } else {
            Restriction::Union(vec![sub, Restriction::Null])
        };
        frame.leaves.push(space);
    }

    /// Closes the case-header pattern window and drains its root: the
    /// topmost restriction is the header pattern's coverage (pushed between
    /// the window's open and here), everything below is debris from earlier
    /// bodies. `None` — a legacy or recovered header with no pattern —
    /// falls back to the unknown space.
    pub(crate) fn close_case_sink(&mut self) -> Option<Restriction> {
        self.switch.close_case_sink()
    }

    /// Substitutes a captured root's type-free coverage with the
    /// scrutinee's context type. A top-typed scrutinee (`dynamic`, an
    /// unknown or `void` face) is spelled `Object?` — the type-free row
    /// covers the whole space there exactly as it covers a named context,
    /// where the tracker's name-only substitution would leave the row
    /// unknown and the remaining `Object` half falsely uncovered.
    pub(crate) fn substitute_root(&self, root: Option<Restriction>) -> Restriction {
        if root == Some(Restriction::NamedContext) {
            let scrutinee = self
                .switch
                .switches
                .last()
                .map_or(Ty::Unknown, |ctx| ctx.scrutinee);
            match NameTy::from_semantic(scrutinee) {
                NameTy::TopNullable => return Restriction::Named(self.intern("Object"), true),
                NameTy::TopNonNullable => {
                    return Restriction::Named(self.intern("Object"), false);
                }
                _ => {}
            }
        }
        self.switch.substitute_root(root)
    }

    /// The name-level exhaustiveness world over this unit's element model.
    /// `'a` (not `'_`): the world borrows only the engine's long-lived
    /// index/string-table references, so it can outlive `&mut self` uses.
    pub(crate) fn name_world(&self) -> NameWorld<'a> {
        NameWorld {
            index: self.index,
            file: self.file,
            strings: self.strings,
            scope: Some(NameScope {
                graph: self.graph,
                namespaces: self.namespaces,
                lib: self.lib,
            }),
            resolved: std::cell::RefCell::default(),
            bool_id: self.intern("bool"),
            object_id: self.intern("Object"),
            enum_id: self.intern("Enum"),
            primitive_ids: self.members.primitive_ids,
            core_supertypes: self.members.core_supertypes,
            unknowns: std::cell::Cell::new(0),
        }
    }

    /// Closes an object pattern (the parser fires `handle_object_pattern`
    /// *after* the rest — fields and boundaries included): the row lowers
    /// to the bare head type exactly when every field's drained restriction
    /// covers its member's whole column ([`NameWorld::property_type`], the
    /// member-table walk, superclass chain included). Any residual narrows
    /// the row below the name level's expressible spaces, so the row is
    /// the unknown space (the established under-report). `head` is `None`
    /// for the unmodeled `Type.field` shorthand shapes.
    pub(crate) fn close_object_fields(&mut self, head: Option<StringId>, fields: &[ObjectField]) {
        let world = self.name_world();
        self.switch.close_object_fields(&world, head, fields);
    }

    /// Opens a switch context; the scrutinee's push-time row is the last
    /// one on the row stack when the block opens. No row — the shape a
    /// complex scrutinee leaves — latches `Ty::Unknown`, and
    /// [`Self::report_switch`] skips the entire check for that (the
    /// oracle's `InvalidType` gate), so the latch gap suppresses instead
    /// of mis-answering.
    pub(crate) fn open_switch(&mut self, is_expression: bool, kw: SourceRange) {
        let scrutinee_row = self.rows.last().filter(|row| !self.is_index_key_row(row));
        let scrutinee = scrutinee_row.map_or(Ty::Unknown, |row| row.ty);
        // The void-operand use (upstream `visitSwitchExpression`/
        // `visitSwitchStatement` → `checkForUseOfVoidResult(node.
        // expression)` at error_verifier.dart:2024/:2038): `switch (v())`
        // reports `use_of_void_result` at the scrutinee's span. The
        // operand's static type resolves the invoke's return — a `v()`
        // call's row spells `Unknown` until `operand_static_ty` reads the
        // callee's `void` return.
        if let Some(row) = scrutinee_row.cloned() {
            let ty = self.operand_static_ty(&row);
            if ty == Ty::Void {
                let range = self.use_site_range(&row);
                self.emit(SemanticCode::UseOfVoidResult, range, "", "");
            }
        }
        self.switch.open_switch(scrutinee, is_expression, kw);
    }

    /// Computes the exhaustiveness verdict and emits the switch
    /// diagnostics (per-case reachability, then the covering gap).
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    pub(crate) fn report_switch(&mut self) {
        // A pre-patterns library takes the legacy statement face instead
        // (constant_verifier.dart:487-507: `visitSwitchStatement` runs
        // `_validateSwitchExhaustiveness` only under `Feature.patterns`, and
        // a switch expression cannot exist before 3.0), so no reachability,
        // no covering gap, no dead member bodies — and nothing retained for
        // the teardown re-decision. The pre-patterns verifier's own rows
        // (primitive equality, the type-mismatch gate, the enum-constant
        // coverage walk) run off the same closed context
        // ([`Self::report_legacy_switch`]).
        if !self.patterns_enabled {
            self.report_legacy_switch();
            return;
        }
        let Some(ctx) = self.switch.switches.pop() else {
            return;
        };
        // An unresolvable scrutinee is the oracle's `InvalidType`
        // (constant_verifier.dart:1035): the entire exhaustiveness pass —
        // reachability and covering alike — gates off behind
        // `hasInvalidType` (:1095). Mapping it to the nullable top would
        // drop every case row and false-positive the covering gap. (The
        // push-time latch leaves `Unknown` exactly here: no row, or a row
        // whose declared type never resolved — the "complex scrutinee"
        // blind spot, now a clean whole-check suppression.)
        if ctx.scrutinee == Ty::Unknown || ctx.has_invalid_type {
            return;
        }
        // A record- or function-typed scrutinee joins the name-level
        // world as a shape-keyed unknown sentinel (`NameTy::from_semantic`
        // — no name-level decomposition exists for the shape columns),
        // and the official's unknown-scrutinee face applies to it alike:
        // `createUnknownSpace`'s matched value "is matched by nothing"
        // (shared.dart:310-320), so no row set can prove or refute — the
        // whole-check suppression. Without it every case row failed the
        // `ty <: single` filter and the switch reported a spurious
        // covering gap (the nullable-function-typed field scrutinee of
        // `switch (where) { null => .., final where => .. }`).
        if matches!(
            NameTy::from_semantic(ctx.scrutinee),
            NameTy::Unknown(key) if key >= (1 << 41)
        ) {
            return;
        }
        let world = self.name_world();
        // An unsolved `T extends Enum` scrutinee (#2456): the oracle solved
        // the generic call that produced it, so its switch is judged over a
        // real enum. The case set witnesses that enum
        // ([`crate::switch_tracker::enum_bound_scrutinee`]); judging the
        // verdict, the row text, and the deferred re-decision over it is the
        // solved face. Any other scrutinee keeps its own type.
        let scrutinee =
            crate::switch_tracker::enum_bound_scrutinee(&world, &ctx).unwrap_or(ctx.scrutinee);
        let value = NameTy::from_semantic(scrutinee);
        for (i, c) in ctx.cases.iter().enumerate() {}
        let cases: Vec<Case<NameTy>> = ctx
            .cases
            .iter()
            .map(|c| Case {
                space: case_space(&world, &c.space, value),
                guarded: c.guarded,
            })
            .collect();
        let report = compute(&world, &value, &cases);
        let unreachable: Vec<SourceRange> = report
            .unreachable
            .iter()
            .filter_map(|index| ctx.cases.get(*index).map(|c| c.anchor))
            .collect();
        let (ty_text, unmatched, suggested) = match &report.exhaustiveness {
            Exhaustiveness::NonExhaustive(witnesses) => {
                let first = witnesses.first();
                (
                    self.ty_display(scrutinee),
                    first.map_or(String::new(), |w: &EngineWitness<NameTy>| {
                        w.as_witness(&world)
                    }),
                    first.map_or(String::new(), |w| w.as_correction(&world)),
                )
            }
            Exhaustiveness::Exhaustive => (String::new(), String::new(), String::new()),
        };
        // The oracle reports reachability per case first
        // (constant_verifier.dart:1080-1088), then the covering gap
        // (:1122-1140) at the `switch` keyword.
        for anchor in unreachable {
            self.emit(SemanticCode::UnreachableSwitchCase, anchor, "", "");
        }
        // The dead member faces (statement switches only — a switch
        // expression's covered arms carry `unreachable_switch_case` at the
        // arrow alone). The total-match chain — an unguarded member whose
        // space covers the whole scrutinee (`case _`, a variable pattern
        // over the same type) driving every later member head and body
        // dead — is the flow layer's fact (`analyzeSwitchStatement`'s dead
        // `unmatched` state, translated at the diagnose `DeadCode` arm);
        // staging it here too doubled every row. This walk keeps the face
        // the flow layer defers here: a member whose own space is
        // nullability-disjoint from the scrutinee never matches, so only
        // its body dies (the official's `_EqualityCheckHasKnownResult`
        // known-false arm). Exhaustiveness-only coverage (a duplicate
        // constant over a fully covered scrutinee) is NOT flow-dead — the
        // oracle pins that to `unreachable_switch_case` alone.
        if !ctx.is_expression && !cases.is_empty() {
            let total = |index: usize| {
                matches!(
                    compute(&world, &value, std::slice::from_ref(&cases[index])).exhaustiveness,
                    Exhaustiveness::Exhaustive
                )
            };
            // The never-match WARNING face — the case's every single type
            // can never be a subtype of the matched value's type — is the
            // disjointness gate
            // ([`crate::switch_tracker::restriction_disjoint_from`])
            // over constant roots ([`restriction_is_constant`]); it
            // decides the `constant_pattern_never_matches_value_type` row
            // at the constant (the pattern-checking faces) and never
            // kills a body. The flow's body-death rides ONLY the
            // nullability-disjoint face: `_equalityCheck`'s known-false
            // arm is `_typesAreDisjointDueToNullability` (upstream Dart
            // SDK source, pkg/_fe_analyzer_shared/lib/src/flow_analysis/
            // flow_analysis.dart:9496-9501, fed into
            // `_handleEqualityCheckPattern`'s known-false arm :8930-8948)
            // — one operand the `Null` type, the other non-nullable. A
            // merely value-disjoint constant (`case true` over an `int`
            // scrutinee, another enum's constant over an enum) falls to
            // `_NoEqualityInformation`: the warning reports but the body
            // stays reachable (oracle 3.13.1, probe2444 pa/pb/ph). Two
            // dead faces:
            //   - every leaf `null` against a non-nullable matched type
            //     (`case null` over `int` — a union needs all leaves null,
            //     [`crate::switch_tracker::restriction_is_null_only`]);
            //   - a non-null constant root against a `Null`-typed
            //     scrutinee (`case true` over `Null` — probe2444 pf; a
            //     `null` leaf there is the known-TRUE equality and its
            //     body stays live).
            // Witness-set equality alone never answers here: the space
            // model drops a row it cannot express ("the `Enum` root
            // minus one constant") and regenerates the bare scrutinee
            // witness, which would mark a case whose constant may overlap
            // the matched type (`AstNodeTag.a` over an `Enum`-typed
            // scrutinee) dead. A guarded case never qualifies: `compute`
            // keeps its row out of the covering set (the guard may fail),
            // so its body stays live.
            let body_dies = |index: usize| {
                let space = &ctx.cases[index].space;
                if crate::switch_tracker::restriction_is_null_only(space) {
                    !world.can_be_subtype_of(&NameTy::Null, &value)
                } else {
                    matches!(value, NameTy::Null)
                        && restriction_is_constant(space)
                        && crate::switch_tracker::restriction_disjoint_from(&world, space, value)
                }
            };
            // The total-match handoff line: from the first unguarded
            // total member on, every later head and body is chain-dead
            // and the flow fact stages those rows — `body_dies` below
            // applies only to the members before it (staging them too
            // doubled the rows).
            let chain_start = (0..ctx.cases.len())
                .find(|&index| !ctx.cases[index].guarded && total(index))
                .map_or(ctx.cases.len(), |index| index + 1);
            for (index, case) in ctx.cases.iter().enumerate().take(chain_start) {
                if !case.guarded
                    && let Some(body) = case.body
                    && body_dies(index)
                {
                    self.emit(SemanticCode::DeadCode, body, "", "");
                }
            }
        }
        // A scrutinee name the world cannot resolve (neither this unit nor
        // the library graph declares it — [`NameWorld::name_resolvable`])
        // is opaque: its sealedness and subclass set are unknowable, so
        // the covering verdict may be falsely negative and the expression
        // face — always must-be-exhaustive — suppresses (under-report
        // direction). The statement face is already gated on sealed facts.
        // An unmodelable scrutinee (record/function shapes fold to one
        // fixed `Unknown` — no field columns, so even a `_` row cannot be
        // proven to cover it) never decides the face either: the verdict
        // would be the world's blind spot, not the program's (#2139 — a
        // record alias now folds to its shape, surfacing the face).
        let expression_face_decides = match value {
            NameTy::Named(name, _) | NameTy::NamedArgs(name, _, _) => world.name_resolvable(name),
            NameTy::Unknown(u64::MAX) => false,
            _ => true,
        };
        // The private-enum variant (constant_verifier.dart:1082-1109): an
        // enum declared in another library whose name is private, or whose
        // every witness names a private constant (`._`), reports the SAME
        // shared code under the private template (`messages.yaml`
        // `nonExhaustiveSwitchExpressionPrivate` — one diagnostic name, its
        // own problem/correction text). The row reports either way; only the
        // message differs.
        let private_foreign_enum = match value {
            NameTy::Named(name, _) | NameTy::NamedArgs(name, _, _)
                if world.is_enum_name(name) && world.is_foreign_library(name) =>
            {
                self.strings.resolve(name).starts_with('_')
                    || match &report.exhaustiveness {
                        Exhaustiveness::NonExhaustive(witnesses) => witnesses
                            .iter()
                            .all(|w| w.as_witness(&world).contains("._")),
                        Exhaustiveness::Exhaustive => false,
                    }
            }
            _ => false,
        };
        let must_be_exhaustive = (ctx.is_expression && expression_face_decides)
            || is_always_exhaustive(&world, value);
        if let Exhaustiveness::NonExhaustive(_) = report.exhaustiveness {
            if must_be_exhaustive && ctx.default_kw.is_none() {
                let code = if ctx.is_expression {
                    SemanticCode::NonExhaustiveSwitchExpression
                } else {
                    SemanticCode::NonExhaustiveSwitchStatement
                };
                let row = self.covering_row(
                    code,
                    ctx.switch_kw,
                    &ty_text,
                    &unmatched,
                    &suggested,
                    private_foreign_enum,
                );
                // A nullable scrutinee defers (#2538): the flow layer's
                // per-clause matched type may promote the scrutinee
                // (`if (n == null) return;` strips the null witness the
                // parse-time row reports), and that fact drains only at
                // the teardown join — stage the row for the re-decision
                // there instead of reporting now. A NAMED scrutinee defers
                // for the same reason: an enclosing `is`-guard promotes a
                // bare read past its parse-time face (`if (e is Literal)
                // switch (e) {…}` — the subclass cases cover the promoted
                // `Literal`, never the unrefined `Object`), and the
                // teardown keeps the unrefined verdict verbatim when no
                // refinement stands. Anything else (a clause-less switch,
                // an unnameable scrutinee) keeps today's immediate face.
                if (Self::is_nullable_ty(scrutinee) || matches!(scrutinee, Ty::Named { .. }))
                    && let Some(first) = ctx.cases.first()
                {
                    self.pending_switch_coverings.push(PendingSwitchCovering {
                        code,
                        kw: ctx.switch_kw,
                        first_anchor: first.flow_anchor,
                        scrutinee,
                        ty_text,
                        unmatched,
                        suggested,
                        private: private_foreign_enum,
                        spaces: ctx.cases.iter().map(|c| c.space.clone()).collect(),
                        guarded: ctx.cases.iter().map(|c| c.guarded).collect(),
                    });
                } else {
                    self.report.diagnostics.push(row);
                }
            }
        } else if must_be_exhaustive && let Some(default_at) = ctx.default_kw {
            // The cases already cover the scrutinee, so the `default`
            // clause can never match (`nonExhaustiveness == null &&
            // defaultNode != null && mustBeExhaustive`,
            // constant_verifier.dart:1141-1148).
            self.emit(SemanticCode::UnreachableSwitchDefault, default_at, "", "");
        }
    }

    /// The covering row both faces build — the immediate emit in
    /// [`Self::report_switch`] and the deferred re-decision in
    /// [`Self::teardown_covering`]. The plain template carries the witness
    /// texts (the `{1}` witness and the `{2}` correction, diagnostic.g.dart
    /// :13018/:13079); the private-enum variant carries the scrutinee type
    /// alone (upstream `pkg/analyzer/messages.yaml`
    /// `nonExhaustiveSwitchExpressionPrivate`).
    pub(crate) fn covering_row(
        &self,
        code: SemanticCode,
        kw: SourceRange,
        ty_text: &str,
        unmatched: &str,
        suggested: &str,
        private: bool,
    ) -> Diagnostic {
        let (message, correction) = if private {
            (code.message_private(ty_text), code.correction_private())
        } else {
            (
                code.message3(ty_text, unmatched, suggested),
                code.correction_message(suggested),
            )
        };
        Diagnostic::new(
            code.severity(),
            code.code_info(),
            message,
            self.file,
            kw,
            correction,
        )
    }

    /// The deferred covering verdict's teardown half (#2538): the staged
    /// row re-decides against the flow's matched type. `matched` is the
    /// first case clause's `FlowFact::MatchedType` row (`None` — a
    /// complex scrutinee, no bare promotion key): the fallback keeps the
    /// parse-time verdict verbatim. A refinement that decides the
    /// covering gap away drops the row; anything else re-renders against
    /// the refined value's witness.
    pub(crate) fn teardown_covering(
        &self,
        pending: &PendingSwitchCovering,
        matched: Option<Ty>,
    ) -> Option<Diagnostic> {
        let world = self.name_world();
        // The flow's matched type refines the verdict only when it is a face
        // the world can decide: a promoted read reaches a declared type, so a
        // matched spelling no declaration backs (#2456's unsolved parameter,
        // re-read by the flow from the same unresolved row) refines nothing —
        // the parse-time verdict stands, rendered against the type it was
        // decided over. An UNRESOLVED row (`Ty::Unknown` — the flow's own
        // blind spot for a complex scrutinee) refines nothing either: it
        // folds to the nullable top, which would both re-render the type as
        // the empty text and answer over a space the program never had.
        let matched_decides = matched.is_some_and(|ty| {
            ty != Ty::Unknown
                && ty != pending.scrutinee
                && match NameTy::from_semantic(ty) {
                    NameTy::Named(name, _) | NameTy::NamedArgs(name, _, _) => {
                        world.is_known_head(name)
                    }
                    NameTy::Unknown(_) => false,
                    _ => true,
                }
        });
        let refined = match matched.filter(|_| matched_decides) {
            Some(ty) => NameTy::from_semantic(ty),
            None => {
                return Some(self.covering_row(
                    pending.code,
                    pending.kw,
                    &pending.ty_text,
                    &pending.unmatched,
                    &pending.suggested,
                    pending.private,
                ));
            }
        };
        let cases: Vec<Case<NameTy>> = pending
            .spaces
            .iter()
            .zip(pending.guarded.iter())
            .map(|(space, guarded)| Case {
                space: case_space(&world, space, refined),
                guarded: *guarded,
            })
            .collect();
        match compute(&world, &refined, &cases).exhaustiveness {
            Exhaustiveness::Exhaustive => None,
            Exhaustiveness::NonExhaustive(witnesses) => {
                let first = witnesses.first();
                let ty_text = self.ty_display(
                    matched
                        .filter(|_| matched_decides)
                        .unwrap_or(pending.scrutinee),
                );
                let unmatched = first.map_or(String::new(), |w: &EngineWitness<NameTy>| {
                    w.as_witness(&world)
                });
                let suggested = first.map_or(String::new(), |w| w.as_correction(&world));
                Some(self.covering_row(
                    pending.code,
                    pending.kw,
                    &ty_text,
                    &unmatched,
                    &suggested,
                    pending.private,
                ))
            }
        }
    }

    /// Renders `ty` in the official display form through [`Ty::render`] —
    /// the single named-type text source (the model owns the
    /// `Name<Arg, ...>?` face; the diagnoser only supplies the tables).
    /// Every slot this pass can hold resolves through the boundary's seeded
    /// pool — local slots verbatim, foreign ones projected.
    pub(crate) fn render_ty(&self, ty: Ty) -> String {
        ty.render(self.strings, &self.boundary.pool())
    }

    /// The diagnostic-message face of `ty` — the oracle's
    /// `DartType.getDisplayString`, whose resolved `staticType` for a bare
    /// parameterized head is always the instantiate-to-bounds
    /// instantiation (`G` renders `G<A<dynamic>>`, never `G`; upstream
    /// Dart SDK source, `type.dart` `instantiateTypeFormalsToBounds`) —
    /// at EVERY nesting depth, because the resolution instantiates each
    /// written annotation: a `List<G>` argument displays `List<G<num>>`,
    /// a function formal `G` displays `G<num>`, a record field likewise.
    /// Every message slot this pass renders (the type-argument bounds
    /// face's argument and bound, the assignment faces, the covering
    /// rows) resolves through this one function.
    /// The bind sites (`bind_declared_type`/`bind_parameter_type`) already
    /// store the instantiated face, so this fill is the catch-all belt for
    /// a raw `Named{_, EMPTY}` arriving through a channel no bind site
    /// owns — display-only: the judgment `Ty` the verdict compared keeps
    /// its own face.
    pub(crate) fn ty_display(&self, ty: Ty) -> String {
        self.render_ty(self.display_filled_ty(ty))
    }

    /// `ty` with every raw parameterized head replaced by its
    /// instantiate-to-bounds face — the head itself plus every head its
    /// argument list, record fields, or function shape spells
    /// ([`Self::INSTANTIATE_DEPTH_CAP`] bounds the descent: a
    /// self-referential bound (`class G<T extends G>`) has no finite
    /// instantiation, and the raw head it would keep re-enters the same
    /// fill). A spine that changed re-interns; an already-filled type —
    /// every judgment-side slot — walks to its leaves and allocates
    /// nothing.
    fn display_filled_ty(&self, ty: Ty) -> Ty {
        self.display_filled_ty_at(ty, 0)
    }

    /// The nesting cap of [`Self::display_filled_ty`]: a written type
    /// deeper than this keeps the raw head at that depth (the same
    /// under-report direction the rest of the display faces take), and the
    /// self-referential-bound cycle terminates there.
    const INSTANTIATE_DEPTH_CAP: u32 = 16;

    fn display_filled_ty_at(&self, ty: Ty, depth: u32) -> Ty {
        if depth >= Self::INSTANTIATE_DEPTH_CAP {
            return ty;
        }
        let depth = depth + 1;
        let ty = self.instantiated_head_ty(ty);
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                if args == ArgListId::EMPTY {
                    return ty;
                }
                let written = self.boundary.pool().get(args).to_vec();
                let filled: Vec<Ty> = written
                    .iter()
                    .map(|arg| self.display_filled_ty_at(*arg, depth))
                    .collect();
                if filled == written {
                    return ty;
                }
                self.boundary
                    .args
                    .borrow_mut()
                    .named(name, nullable, &filled)
            }
            Ty::Record { shape, nullable } => {
                // The pool guard drops before the recursive walk: the fill
                // re-enters `instantiated_head_ty`, whose own `borrow_mut`
                // would panic the cell.
                let (positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.record_shape(shape);
                    (shape.positional.to_vec(), shape.named.to_vec())
                };
                let filled_positional: Vec<Ty> = positional
                    .iter()
                    .map(|field| self.display_filled_ty_at(*field, depth))
                    .collect();
                let filled_named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|(name, field)| (*name, self.display_filled_ty_at(*field, depth)))
                    .collect();
                if filled_positional == positional && filled_named == named {
                    return ty;
                }
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&filled_positional, &filled_named, nullable)
            }
            Ty::Function { shape, nullable } => {
                let (type_params, type_param_count, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params_named.to_vec(),
                        shape.type_params,
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                let filled_params: Vec<(StringId, Option<Ty>)> = type_params
                    .iter()
                    .map(|(param, bound)| {
                        (*param, bound.map(|b| self.display_filled_ty_at(b, depth)))
                    })
                    .collect();
                let filled_return = self.display_filled_ty_at(return_ty, depth);
                let filled_positional: Vec<Ty> = positional
                    .iter()
                    .map(|formal| self.display_filled_ty_at(*formal, depth))
                    .collect();
                let filled_named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|(name, formal)| (*name, self.display_filled_ty_at(*formal, depth)))
                    .collect();
                if filled_return == return_ty
                    && filled_positional == positional
                    && filled_named == named
                    && filled_params == type_params
                {
                    return ty;
                }
                let mut pool = self.boundary.args.borrow_mut();
                if type_params.is_empty() {
                    // The count-only shape (`ArgPool::function`'s producer)
                    // keeps its count: the group has no names to display,
                    // but the re-interned slot must not merge with the
                    // count-less degenerate face.
                    pool.function(
                        type_param_count,
                        filled_return,
                        &filled_positional,
                        &filled_named,
                        nullable,
                    )
                } else {
                    pool.function_generic(
                        &filled_params,
                        filled_return,
                        &filled_positional,
                        &filled_named,
                        nullable,
                    )
                }
            }
            _ => ty,
        }
    }

    /// The function-subtype decision (`_isFunctionSubtypeOf`,
    /// upstream Dart SDK source, pkg/analyzer/lib/src/dart/element/
    /// subtype.dart): return covariance with the void Right-Top, formal
    /// contravariance over the target's whole positional prefix, the two
    /// positional-count rules (leftover target formals, leftover required
    /// positionals), and the named coverage with the required-presence
    /// rule (a `required` named formal in `from` must be `required` in
    /// `to` — its callers always pass it). Counts and names are structural
    /// facts; every type question routes through [`Engine::assignable`],
    /// and one undecidable slot suppresses the whole (`None` — the
    /// under-report direction).
    /// The strict subtype ask the function-shape and covariant-interface
    /// rules run on every slot (`subtype.dart`'s function and interface
    /// cases): real subtyping, never the assignability face. The
    /// downcast leniencies the assignable face grants (`dynamic → C`,
    /// `Null → C?`) are expression-position casts — inside a type's shape
    /// (`XPathNode Function(XmlNode)` against
    /// `XPathItem Function(dynamic)`) the contravariant leg asks
    /// `dynamic <: XmlNode`, which is false, and the generic-argument
    /// covariance asks `dynamic <: ValueT` the same way.
    pub(crate) fn slot_subtype(&self, sub: Ty, sup: Ty) -> Option<bool> {
        if sub == Ty::Unknown || sup == Ty::Unknown {
            return None;
        }
        if sub == Ty::Never {
            return Some(true);
        }
        if sub == Ty::Dynamic || sub == Ty::Void {
            // A top source subtypes only the top faces (the nullable
            // projections included) — never a concrete named head.
            return Some(matches!(
                sup,
                Ty::Dynamic
                    | Ty::Void
                    | Ty::Named { nullable: true, .. }
                    | Ty::Record { nullable: true, .. }
                    | Ty::Function { nullable: true, .. }
            ));
        }
        if sub == Ty::Null {
            return Some(matches!(
                sup,
                Ty::Null
                    | Ty::Dynamic
                    | Ty::Void
                    | Ty::Named { nullable: true, .. }
                    | Ty::Record { nullable: true, .. }
                    | Ty::Function { nullable: true, .. }
            ));
        }
        if sup == Ty::Null {
            return Some(false);
        }
        // Right Nullable (subtype.dart 453-454): `S <: T?` needs `S <: T`
        // (the `Null` leg decided above), and `S? <: T?` strips to
        // `S <: T` (347-348). The assignable face's into-`T?` Right-Top
        // leniency — every source "assigns" into a nullable target — is
        // the assignment posture, not this relation, and a shape slot
        // must not read it (`double? <: int?` is false: the
        // f1-misfit tear-off pair stays informative, the oracle's
        // silence). The stripped pair re-enters the ordinary ask; the
        // nullable source against a non-nullable target keeps its
        // rejection there.
        let (sub, sup) = if matches!(
            sup,
            Ty::Named { nullable: true, .. }
                | Ty::Record { nullable: true, .. }
                | Ty::Function { nullable: true, .. }
        ) {
            (Self::non_nullable_ty(sub), Self::non_nullable_ty(sup))
        } else {
            (sub, sup)
        };
        self.assignable(sub, sup, false)
    }

    pub(crate) fn fn_sig_subtype(&self, from: &FnVarSig, to: &FnVarSig) -> Option<bool> {
        // The generic-function-type bounds face — `relateTypeParameters`
        // (upstream Dart SDK source, pkg/analyzer/lib/src/dart/element/
        // type_system.dart:1581-1635), which `_isFunctionSubtypeOf` runs
        // ahead of its return/formal checks (subtype.dart:338-350): the
        // signatures' own type-parameter counts must match, and each
        // parameter's written bound must be EQUAL on both sides —
        // `isEqualTo`, a mutual-subtype question over the pair, with the
        // sides' own parameters unified by position (upstream
        // instantiates both signatures with one shared fresh set, so the
        // source's `T` in a bound compares against the target's
        // parameter of the same index, whatever its name). A bound
        // written on neither side needs no ask; one written on exactly
        // one side reads `dynamic` against the other
        // (`bound1 ??= DynamicTypeImpl.instance`). This is an equality,
        // not a bounds direction: `void Function<Y extends num>()` is a
        // subtype of `void Function<Y extends int>()` in NEITHER
        // direction. A count-only entry (no name — the interned shape
        // records just the `<...>` group's size) has an unknown bound
        // and never decides (`None`, the under-report direction).
        if from.params.len() != to.params.len() {
            return Some(false);
        }
        if !from.params.is_empty() {
            let unify: FxHashMap<StringId, Ty> = from
                .params
                .iter()
                .zip(to.params.iter())
                .filter(|((from_name, _), (to_name, _))| {
                    *from_name != StringId::NONE && *to_name != StringId::NONE
                })
                .map(|((from_name, _), (to_name, _))| (*from_name, Ty::named(*to_name)))
                .collect();
            for ((from_name, from_bound), (to_name, to_bound)) in
                from.params.iter().zip(to.params.iter())
            {
                let (bound_from, bound_to) = (*from_bound, *to_bound);
                if bound_from.is_none() && bound_to.is_none() {
                    // Both unwritten: the implicit bounds coincide and no
                    // ask is needed.
                    continue;
                }
                if *from_name == StringId::NONE || *to_name == StringId::NONE {
                    // A count-only entry's bound is unknown (never
                    // `dynamic`) — no ask is honest.
                    return None;
                }
                let from_bound = self.subst_ty(bound_from.unwrap_or(Ty::Dynamic), &unify);
                let to_bound = bound_to.unwrap_or(Ty::Dynamic);
                match (
                    self.slot_subtype(from_bound, to_bound),
                    self.slot_subtype(to_bound, from_bound),
                ) {
                    (Some(true), Some(true)) => {}
                    (Some(false), _) | (_, Some(false)) => return Some(false),
                    _ => return None,
                }
            }
        }
        // The shared-parameter instantiation `relateTypeParameters` ran:
        // the source's own type parameters spell the TARGET's parameters
        // by position inside its return and formal faces — `T
        // Function<T>(T)` against `U Function<U>(U)` compares `U` to `U`
        // (reflexive), never the two unrelated heads the name lattice
        // would reject. Empty when either side is non-generic (the count
        // arm already decided the mixed shape).
        let param_unify: FxHashMap<StringId, Ty> = from
            .params
            .iter()
            .zip(to.params.iter())
            .filter(|((from_name, _), (to_name, _))| {
                *from_name != StringId::NONE && *to_name != StringId::NONE
            })
            .map(|((from_name, _), (to_name, _))| (*from_name, Ty::named(*to_name)))
            .collect();
        let from_return = self.subst_ty(from.return_ty, &param_unify);
        // A target return face naming a type this file cannot spell (a
        // generic callee's own inference parameter — `then<R>`'s
        // `FutureOr<R> Function(T)`) is an inference-site constraint, not
        // a checkable face: the official solves the parameter from the
        // argument at the call (a `void Function(T)` callback infers `R`
        // and type-checks, subtype.dart's left-top `void` leg); the
        // name-level model cannot solve it, and the return-covariance
        // check stays undecided — the under-report direction.
        if to.return_ty != Ty::Void && !self.return_face_undecidable(to.return_ty) {
            match self.slot_subtype(from_return, to.return_ty) {
                Some(false) => return Some(false),
                Some(true) => {}
                None => return None,
            }
        }
        if from.positional.len() < to.positional.len() {
            return Some(false);
        }
        if from.required_positional > to.required_positional {
            return Some(false);
        }
        for (index, to_ty) in to.positional.iter().enumerate() {
            match self.slot_subtype(*to_ty, self.subst_ty(from.positional[index], &param_unify)) {
                Some(false) => return Some(false),
                Some(true) => {}
                None => return None,
            }
        }
        // Named parameters (subtype.dart:132-179): every named parameter
        // the target accepts must exist on the source (contravariant type,
        // no stronger `required`), and the source's extra named parameters
        // must all be optional — a target call never passes them.
        for (name, to_ty, to_required) in &to.named {
            let Some((_, from_ty, from_required)) = from.named.iter().find(|(n, _, _)| n == name)
            else {
                return Some(false);
            };
            if *from_required && !to_required {
                return Some(false);
            }
            match self.slot_subtype(*to_ty, self.subst_ty(*from_ty, &param_unify)) {
                Some(false) => return Some(false),
                Some(true) => {}
                None => return None,
            }
        }
        if from
            .named
            .iter()
            .any(|(name, _, required)| *required && !to.named.iter().any(|(n, _, _)| n == name))
        {
            return Some(false);
        }
        Some(true)
    }

    /// Whether a target face is the generic-callback inference shape —
    /// `FutureOr<R>` over a name this file cannot spell (the callee's own
    /// inference parameter, `then<R>`'s callback formal): the official
    /// solves `R` from the argument at the call, and the name-level model
    /// cannot, so the return-covariance check stays undecided. Any other
    /// face — including an unspellable head the oracle resolves through
    /// the import graph (`Future`) — keeps the ordinary check.
    fn return_face_undecidable(&self, ty: Ty) -> bool {
        let Ty::Named { name, args, .. } = ty else {
            return false;
        };
        if name != self.intern("FutureOr") {
            return false;
        }
        let list = self.boundary.pool().get(args).to_vec();
        list.first().is_some_and(|arg| match arg {
            Ty::Named { name: arg_name, .. } => !self.named_type_is_type(*arg_name),
            _ => false,
        })
    }

    /// The written function-type display for the message slots: the
    /// return type, `Function`, the signature's own type parameters
    /// (`void Function<Y extends num>()`), the positional list with the
    /// optional tail bracketed, and the named section braced with its
    /// `required` keywords (`void Function({required int x})`).
    /// Count-only parameter entries carry no names and render nothing —
    /// the interned shape never recorded them.
    pub(crate) fn fn_sig_display(&self, sig: &FnVarSig) -> String {
        let mut text = self.ty_display(sig.return_ty);
        text.push_str(" Function");
        let params: Vec<&(StringId, Option<Ty>)> = sig
            .params
            .iter()
            .filter(|(name, _)| *name != StringId::NONE)
            .collect();
        if !params.is_empty() {
            text.push('<');
            for (index, (name, bound)) in params.iter().enumerate() {
                if index > 0 {
                    text.push_str(", ");
                }
                text.push_str(self.strings.resolve(*name));
                if let Some(bound) = bound {
                    text.push_str(" extends ");
                    text.push_str(&self.ty_display(*bound));
                }
            }
            text.push('>');
        }
        text.push('(');
        let required = usize::from(sig.required_positional);
        for (index, ty) in sig.positional.iter().enumerate() {
            if index > 0 {
                text.push_str(", ");
            }
            if index == required {
                text.push('[');
            }
            text.push_str(&self.ty_display(*ty));
        }
        if sig.positional.len() > required {
            text.push(']');
        }
        if !sig.named.is_empty() {
            if !sig.positional.is_empty() {
                text.push_str(", ");
            }
            text.push('{');
            for (index, (name, ty, is_required)) in sig.named.iter().enumerate() {
                if index > 0 {
                    text.push_str(", ");
                }
                if *is_required {
                    text.push_str("required ");
                }
                text.push_str(&self.ty_display(*ty));
                text.push(' ');
                text.push_str(self.strings.resolve(*name));
            }
            text.push('}');
        }
        text.push(')');
        text
    }

    /// The verdict-side alias fold, memoized per written `Ty`: subtype
    /// questions re-ask the same destination face across a file's staged
    /// checks (and the covariant element walk re-enters per argument),
    /// while [`Engine::fold_alias_ty`]'s miss path — the scoped lookup,
    /// the declaring-libraries probe, the to-bounds fill on a surviving
    /// bare head — is the cost to amortize. Non-named shapes return
    /// untouched (their inner faces never carry an alias head the verdict
    /// surface sees).
    pub(crate) fn fold_alias_verdict(&self, ty: Ty) -> Ty {
        if !matches!(ty, Ty::Named { .. }) {
            return ty;
        }
        if let Some(folded) = self.alias_fold_memo.borrow().get(&ty) {
            return *folded;
        }
        let mut folded = self.fold_alias_ty(ty);
        if folded == ty {
            // A parameterized-alias use (`StrMap<int>` over
            // `typedef StrMap<V> = Map<String, V>`): the chain fold's
            // hop lands on the substituted RHS head and misreads it as
            // another alias to instantiate — the substituted RHS IS the
            // resolved type the oracle carries (named_type_resolver.dart
            // `_instantiateElement`). Substitute the captured RHS
            // directly, chasing while the rewritten head stays a
            // parameterized alias (`B<int>` over `typedef B<X> = A<X>`
            // still lands on `A<int>`).
            for _ in 0..8 {
                let Ty::Named {
                    name,
                    nullable,
                    args,
                } = folded
                else {
                    break;
                };
                if args == ArgListId::EMPTY {
                    break;
                }
                let Some(substituted) = self.parameterized_alias_fold(name, args) else {
                    break;
                };
                folded = Self::join_nullable(substituted, nullable);
            }
        }
        self.alias_fold_memo.borrow_mut().insert(ty, folded);
        folded
    }

    /// A parameterized-alias use's substituted right-hand type
    /// (`StrMap<int>` reads `Map<String, int>` — the oracle's
    /// `_instantiateElement` face, named_type_resolver.dart): the
    /// declaration's parameter spellings zip against the written
    /// arguments and substitute through the captured RHS. `None` when
    /// the head resolves to no parameterized alias, the declaration
    /// records no parameter spellings, or the written arity disagrees —
    /// the written use stands (the under-report direction).
    fn parameterized_alias_fold(&self, name: StringId, args: ArgListId) -> Option<Ty> {
        let param_names: Vec<StringId> = match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => {
                if decl.kind != MicroKind::TypeAlias {
                    return None;
                }
                decl.type_param_names.to_vec()
            }
            ScopedHit::Found(NamespaceEntry::Decl(owner)) => {
                let decl = self.index.get(owner)?.decl(name)?;
                if decl.kind != MicroKind::TypeAlias {
                    return None;
                }
                decl.type_param_names.to_vec()
            }
            _ => return None,
        };
        if param_names.is_empty() {
            return None;
        }
        let written: Vec<Ty> = self.boundary.pool().get(args).to_vec();
        if written.len() != param_names.len() {
            return None;
        }
        let rhs = self.alias_rhs_any(name)?;
        let map: FxHashMap<StringId, Ty> = param_names.iter().copied().zip(written).collect();
        Some(self.subst_ty(rhs, &map))
    }

    /// The M1 assignability decision ([`Ty::assignable_from`] plus the
    /// final-primitive table): a value typed by one final primitive never
    /// fits another — the types cannot be extended, implemented, or mixed
    /// in, so no subclass relation can close the gap, whatever declaration
    /// the names resolve to. `int`/`double` fit `num` (the one supertype
    /// edge). `int` → `double` is the literal conversion
    /// (`_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart`,
    /// `analyzeIntLiteral` → `convertToDouble`: an integer literal whose
    /// context schema `int` does not satisfy but `double` does is typed
    /// `double`), so `from_literal` — whether the source row is a
    /// primitive literal — decides: `double d = 1;` is clean, `double d =
    /// intVar;` reports (a non-literal expression keeps type `int`; the
    /// conversion keys on the literal, never the value). Same-head pairs
    /// with differing written arguments run the covariant element rule
    /// through the boundary pool ([`Engine::covariant_args`]). `None`
    /// suppresses (under-report).
    /// The function-rule verdict over two function shapes — the arm
    /// [`Engine::assignable`] runs on both the written pair and the folded
    /// pair. The generic-instantiation arm: a generic function-typed
    /// expression in a context position is not the un-instantiated
    /// signature — the resolver's downward tear-off inference instantiates
    /// it toward the context type before the assignability ask, so
    /// `return g;` with `g: T Function<T>(T)` inside an `int Function(int)`
    /// host reads `S = int Function(int)`, a clean pair
    /// (`return_type_verifier.dart`'s `expression.staticType` is the
    /// post-inference type). `_isFunctionSubtypeOf` itself refuses the
    /// pair — its `relateTypeParameters` count gate (subtype.dart:338-345,
    /// type_system.dart:1585-1590) never relates a generic source to a
    /// non-generic target — so this arm answers the instantiation question
    /// `fn_sig_subtype` cannot: bind each source-only signature leaf (the
    /// parameter spellings) from the target's structurally-matching type,
    /// then re-run the subtype checks over the substituted signature. An
    /// arity the instantiation can never bridge (`U Function<U>(U, int)`
    /// against `int Function(int)` — two required positionals where one is
    /// wanted) reports through the same shape rules.
    fn fn_shape_pair_verdict(&self, from_fn: &FnVarSig, to_fn: &FnVarSig) -> Option<bool> {
        if from_fn.params.len() > to_fn.params.len() && to_fn.params.is_empty()
            && let Some(verdict) = self.instantiated_fn_sig_subtype(from_fn, to_fn)
        {
            return Some(verdict);
        }
        self.fn_sig_subtype(from_fn, to_fn)
    }

    pub(crate) fn assignable(&self, from: Ty, to: Ty, from_literal: bool) -> Option<bool> {
        // The structural-descent budget (#3333): a self-referential alias
        // pair — `typedef F = void Function(List<G>); typedef G = void
        // Function(List<F>);` — bounds the alias NAME chain
        // ([`Self::fold_alias_ty`]'s hop set) but not the structural
        // descent: `assignable` → [`Self::slot_subtype`] →
        // [`Self::covariant_args`] → [`Self::fn_sig_subtype`] →
        // `assignable` re-derives the same `(from, to)` pair at every
        // covariance level, unbounded, and a single ask crossing the pair
        // overflows the stack. Every edge of that cycle re-enters this
        // function, so bounding the verdict's own frame depth bounds the
        // cycle: the guard below refuses the 257th nested frame and the
        // verdict answers `None` (unknown — the under-report direction),
        // never a fabricated rejection. 256 frames is far above any
        // written type's nesting, so a legitimate ask never reaches it.
        let Some(_depth) = AssignDepth::enter() else {
            return None;
        };

        // The function-shape read runs on the WRITTEN pair first
        // (`subtype.dart` unfolds the alias before `_isFunctionSubtypeOf`,
        // but the alias faces keep the parameterized alias's signature
        // capture — `instantiated_alias_sig`'s use-site bounds — which the
        // pooled `Ty::Function` shape of a full fold drops). Both sides
        // function-shaped decides by the function rule and never reaches
        // the nominal fold below.
        let mut from_fn = self.function_shape_sig(from);
        let mut to_fn = self.function_shape_sig(to);
        if let (Some(from_fn), Some(to_fn)) = (from_fn.as_ref(), to_fn.as_ref()) {
            return self.fn_shape_pair_verdict(from_fn, to_fn);
        }
        // The alias unfold: the element model never carries a type-alias
        // head into a type question — a `TypeAliasElement` use resolves
        // through its aliased type at annotation resolution (upstream
        // Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
        // named_type_resolver.dart `_instantiateElement` →
        // `TypeAliasElementImpl.instantiateImpl`), so `subtype.dart`
        // decides over the expanded pair. The written `Ty` keeps the
        // alias spelling, so the verdict folds both sides first:
        // `List<dynamic>` against `IntList` asks `List<dynamic>` against
        // `List<int>`, exactly the oracle's resolved `staticType` pair —
        // while the diagnostic's display keeps the written `check.to`.
        // The bare `Never` head fold (#2921 B2) lands on the expanded
        // face: an alias to `Never` reads the canonical bottom, and a
        // written `Never` annotation still meets it (the same face
        // [`Engine::is_never_ty`] answers — [`Engine::normalized_never`]
        // folds deep; the entry fold here covers the verdict surface's
        // own pair, and the deep legs re-enter `assignable` per argument
        // and meet the same shallow face).
        let shallow_never = |ty: Ty| if self.is_never_ty(ty) { Ty::Never } else { ty };
        let from = shallow_never(self.fold_alias_verdict(from));
        let to = shallow_never(self.fold_alias_verdict(to));
        // The folded-pair function read (#3368's foreign-alias face): the
        // written-pair probe above only reaches a same-file alias capture
        // ([`Engine::function_shape_sig`]'s Named arm answers through the
        // parse-time `BoundsIndex` — file-local). A FOREIGN function-RHS
        // typedef — `co19/Utils/lang_feature_check.dart`'s `typedef
        // F<X> = void Function<Y extends X>();` — carries no capture here,
        // but [`Engine::fold_alias_verdict`] resolves it through the
        // element model (`instantiate_alias`'s `decl.var_ty` projection)
        // and the folded face IS `Ty::Function`, `type_params_named`
        // bounds substituted (`substitute_alias_rhs` rewrites inside the
        // shape). Read the folded shapes the written probe missed, then
        // decide by the same function rule the written pair takes —
        // upstream runs the same order (`subtype.dart` unfolds the alias
        // before `_isFunctionSubtypeOf`). A written-side capture keeps its
        // signature (the parameterized-alias use-site bound spellings the
        // pooled shape demotes); the fold adds only what the probe missed.
        if from_fn.is_none() {
            from_fn = self.function_shape_sig(from);
        }
        if to_fn.is_none() {
            to_fn = self.function_shape_sig(to);
        }
        if let (Some(from_fn), Some(to_fn)) = (from_fn.as_ref(), to_fn.as_ref()) {
            return self.fn_shape_pair_verdict(from_fn, to_fn);
        }
        // Right Bottom: no subtyping rule ever holds INTO `Never` but
        // reflexivity (upstream Dart SDK source, pkg/analyzer/lib/src/
        // dart/element/subtype.dart:74-77 answers the `Never` SOURCE; a
        // `Never` target simply falls through every arm to the trailing
        // `false`), so `T0 <: Never` is decidable for every concrete
        // `T0`. The `isAssignableTo` relaxations survive
        // (type_system.dart:860-891): `InvalidType` accepts (the
        // `Unknown` sentinel keeps its `None`), and a `dynamic` source
        // passes only in the non-strict default (`return fromType is
        // DynamicType`, :930) — `strict-casts` keeps the subtype
        // refusal. A `void` source stays the `use_of_void_result`
        // face's own business (`None`).
        if to == Ty::Never {
            return match from {
                Ty::Never => Some(true),
                Ty::Unknown | Ty::Void => None,
                Ty::Dynamic if !self.strict.casts => Some(true),
                _ => Some(false),
            };
        }
        // The type-variable SOURCE arm (upstream Dart SDK source,
        // pkg/analyzer/lib/src/dart/element/subtype.dart's Left Type
        // Variable Bound — the left-side decomposition runs ahead of the
        // target's own FutureOr/`?` decompositions): a bare, non-nullable
        // in-scope parameter `T` subtypes a target `T1` exactly when its
        // declared bound does. `T <: Object?` is `B <: Object?` (an
        // unbounded `T` carries the implicit `Object?`, so `Object? <:
        // Object?` accepts), `T <: Object` is `B <: Object` (fails on the
        // same implicit bound), and `T extends num` subtypes neither
        // `String` nor `String?`. A parameter TARGET is the Left Variable
        // arm's own business (the `to_is_param` block below decides it), so
        // this arm defers on one. Running ahead of the `?` decomposition
        // matters: decomposing `T <: S?` into `T <: S` / `T <: Null` loses
        // the bound's own nullability — `B <: S` rejects where `B <: S?`
        // accepts (`Object?` bound against `Object?` — #3582's
        // `T => Object?` extra family).
        //
        // The bound read is the WRITTEN capture ([`Engine::param_bound`]); an
        // UNWRITTEN bound is the implicit `Object?` (upstream's type
        // variable's nullability-independent upper bound,
        // subtype.dart:336-350), so an unbounded in-scope `T` decides the
        // pair exactly like an `Object?` source — `T` fits `Object?`/
        // `dynamic`/`void` and rejects `Object` and every concrete
        // non-nullable head. Abstaining there was the #3339 divergence:
        // upstream's `isAssignableTo` answers false for `int x = t`, so
        // silence was an under-report on the corpus's largest live
        // `argument_type_not_assignable` mass. A bound that mentions the
        // parameter itself (`X extends FutureOr<X>`) still declines —
        // recursing into it would cycle — and a promoted read's narrowing is
        // not modeled here (the rows carry the declared parameter type; the
        // flow layers re-derive at the read).
        if let Ty::Named {
            name: from_name,
            nullable: false,
            args: ArgListId::EMPTY,
        } = from
            && self.is_scope_param(from_name)
            && !matches!(to, Ty::Named { name: to_name, .. } if self.is_scope_param(to_name))
        {
            let bound = match self.param_bound(from_name) {
                Some(bound) if bound != Ty::Unknown => bound,
                // No written bound (or a capture miss on a parameter this
                // scope provably declares): the implicit `Object?`.
                _ => Ty::Named {
                    name: self.intern("Object"),
                    nullable: true,
                    args: ArgListId::EMPTY,
                },
            };
            if !self.bound_mentions_param(bound, from_name, 0) {
                return self.assignable(bound, to, from_literal);
            }
        }
        // Left FutureOr (`subtype.dart:371-385`, ahead of Right FutureOr):
        // `FutureOr<S0> <: T1` iff `S0 <: T1` and `Future<S0> <: T1`; a
        // `FutureOr<S0>?` source is Left Nullable over it (`Null <: T1` as
        // well). Without this arm a FutureOr source only ever met the
        // same-head matrix, so `FutureOr<S<int>>` failed against itself.
        if let Ty::Named {
            name,
            nullable,
            args,
        } = from
            && name == self.intern("FutureOr")
            && !matches!(to, Ty::Unknown)
        {
            let value_ty = self
                .boundary
                .pool()
                .get(args)
                .first()
                .copied()
                .unwrap_or(Ty::Dynamic);
            if value_ty == Ty::Unknown {
                return None;
            }
            let future_ty = Ty::Named {
                name: self.intern("Future"),
                nullable: false,
                args,
            };
            let null_leg = if nullable {
                self.assignable(Ty::Null, to, false)
            } else {
                Some(true)
            };
            return match (
                self.assignable(value_ty, to, from_literal),
                self.assignable(future_ty, to, from_literal),
                null_leg,
            ) {
                (Some(true), Some(true), Some(true)) => Some(true),
                (Some(false), _, _) | (_, Some(false), _) | (_, _, Some(false)) => Some(false),
                _ => None,
            };
        }
        // The `FutureOr<T>` union rule (`subtype.dart`'s FutureOr case):
        // `S <: FutureOr<T>` iff `S <: Future<T>` or `S <: T` — a nominal
        // head question the matrix cannot ask (Future and FutureOr are
        // unrelated heads). `Null <: FutureOr<ResponsePayload?>` rides the
        // value leg (`Null <: ResponsePayload?`); `Future<X>` rides the
        // future leg.
        if let Ty::Named {
            name,
            nullable: false,
            args,
        } = to
            && name == self.intern("FutureOr")
        {
            // The written-argument-less `FutureOr` face is
            // `FutureOr<dynamic>` (`futureValueType`'s raw-head arm) — the
            // value leg is `dynamic` and accepts everything.
            let value_ty = self
                .boundary
                .pool()
                .get(args)
                .first()
                .copied()
                .unwrap_or(Ty::Dynamic);
            if from == Ty::Unknown || value_ty == Ty::Unknown {
                return None;
            }
            let future_ty = Ty::Named {
                name: self.intern("Future"),
                nullable: false,
                args,
            };
            return match (
                self.assignable(from, future_ty, from_literal),
                self.assignable(from, value_ty, from_literal),
            ) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            };
        }
        // Right Nullable (subtype.dart:224-246): `T0 <: S1?` iff
        // `T0 <: S1` or `T0 <: Null` — a `?` suffix is not a top type
        // (only the top bases `Object?`/`dynamic` accept everything, and
        // they reach the same answer through their own non-null legs),
        // so `int? x = "s"` is the oracle's report, not a silent pass.
        // Left Nullable (subtype.dart:389-392) runs ahead of it: a `S0?`
        // source decomposes on the source side first — `int? <: int?`
        // asks `int <: int?` (this arm's base leg) and `Null <: int?`,
        // never the matrix's own left-nullable refusal (which answers
        // only non-`?` targets).
        let nullable_base = |ty: Ty| match ty {
            Ty::Named {
                name,
                nullable: true,
                args,
            } => Some(Ty::Named {
                name,
                nullable: false,
                args,
            }),
            Ty::Record {
                shape,
                nullable: true,
            } => Some(Ty::Record {
                shape,
                nullable: false,
            }),
            Ty::Function {
                shape,
                nullable: true,
            } => Some(Ty::Function {
                shape,
                nullable: false,
            }),
            _ => None,
        };
        if let Some(base) = nullable_base(to) {
            if let Some(s0) = nullable_base(from) {
                return match (
                    self.assignable(s0, to, from_literal),
                    self.assignable(Ty::Null, to, from_literal),
                ) {
                    (Some(true), Some(true)) => Some(true),
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    _ => None,
                };
            }
            return match (
                self.assignable(from, base, from_literal),
                self.assignable(from, Ty::Null, from_literal),
            ) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            };
        }
        // A `Null` source against an in-scope parameter target (`Null => T`
        // — #3582's extra family): the pool-less matrix's Left Null arm
        // rejects every non-top target, but upstream's `Null <: T` holds
        // whenever `T` is POTENTIALLY NULLABLE — the parameter's declared
        // bound (`Null <: B`: an unbounded `T` reads `Object?` and accepts;
        // `T extends num` keeps the rejection). The same Left-Variable
        // decomposition the source arm runs, mirrored on the target.
        if from == Ty::Null
            && let Ty::Named {
                name: to_name,
                nullable: false,
                args: ArgListId::EMPTY,
            } = to
            && self.is_scope_param(to_name)
        {
            let bound = match self.param_bound(to_name) {
                Some(bound) if bound != Ty::Unknown => bound,
                _ => Ty::Named {
                    name: self.intern("Object"),
                    nullable: true,
                    args: ArgListId::EMPTY,
                },
            };
            if !self.bound_mentions_param(bound, to_name, 0) {
                return self.assignable(Ty::Null, bound, false);
            }
        }
        // The cross-family arms below read `from_fn`/`to_fn` computed at
        // entry (the written pair — the alias faces' signature capture).
        if let Some(verdict) = to.assignable_from(from) {
            // The strict-casts overlay (`type_system.dart:891-930`: with
            // `strict-casts: true`, "the option disables all downcasts,
            // including casts from `dynamic`"). A `dynamic` source is a
            // subtype only of the top types — into any named target (the
            // `?` projection included) or `Null` the assignment is a
            // rejected downcast, reporting the ordinary assignability code.
            if self.strict.casts && from == Ty::Dynamic && !matches!(to, Ty::Dynamic | Ty::Void) {
                return Some(false);
            }
            return Some(verdict);
        }
        if let (
            Ty::Record {
                shape: from_shape,
                nullable: from_nullable,
            },
            Ty::Record {
                shape: to_shape,
                nullable: to_nullable,
            },
        ) = (from, to)
        {
            // Nullability compat already ran inside `assignable_from`'s
            // Left-Nullable arm; a surviving pair has a non-nullable source
            // or a nullable target. The shape identity decides the rest
            // (upstream `subtype.dart`'s record rule: equal arity, pairwise
            // field assignability, equal name set).
            if from_nullable && !to_nullable {
                return Some(false);
            }
            // Own the shapes before recursing: the pool guard borrows the
            // args RefCell, and the field checks below recurse into
            // `assignable`, whose alias chase re-enters the pool through
            // `intern_list`'s `borrow_mut` — a guard held across the
            // recursion panics the cell (#2594's live-repro follow-on).
            let (from, to) = {
                let pool = self.boundary.pool();
                (
                    pool.record_shape(from_shape).clone(),
                    pool.record_shape(to_shape).clone(),
                )
            };
            if from.positional.len() != to.positional.len() || from.named.len() != to.named.len() {
                return Some(false);
            }
            let mut unknown = false;
            // An `InvalidType` field poisons the whole-record question, and
            // the poison outranks every field verdict: a record literal's
            // inference is decided as a unit, so a recovered-comma-hole
            // field (`(FeatureSet?, InvalidType)` against
            // `(FeatureSet, Version)`) leaves no assignable ask at all —
            // the oracle's splice stays silent on the other fields'
            // nullable-vs-non-nullable mismatches too. The scan runs over
            // the whole shape before any field decides.
            if from
                .positional
                .iter()
                .chain(from.named.iter().map(|(_, f)| f))
                .any(|f| *f == Ty::Unknown)
                || to
                    .positional
                    .iter()
                    .chain(to.named.iter().map(|(_, t)| t))
                    .any(|t| *t == Ty::Unknown)
            {
                return None;
            }
            for (&f, &t) in from.positional.iter().zip(to.positional.iter()) {
                match self.assignable(f, t, from_literal) {
                    Some(false) => return Some(false),
                    Some(true) => {}
                    None => unknown = true,
                }
            }
            // The name set must coincide — canonical interning sorts the
            // pairs by name, so a pairwise `(name, ty)` walk decides both.
            for ((fn_, f), (tn, t)) in from.named.iter().zip(to.named.iter()) {
                if fn_ != tn {
                    return Some(false);
                }
                match self.assignable(*f, *t, from_literal) {
                    Some(false) => return Some(false),
                    Some(true) => {}
                    None => unknown = true,
                }
            }
            return (!unknown).then_some(true);
        }
        // The cross-family function face (upstream `isAssignableTo`,
        // type_system.dart:891-930 — the ask behind
        // `argument_type_not_assignable`): with a function shape on one
        // side the pair is decisive, not a blind spot. A function-shaped
        // source subtypes exactly the `Object` and `Function` heads among
        // interface targets (subtype.dart's function block, then the
        // unconditional `return false`); an interface source facing a
        // function-shaped target decides only on two-sided facts — the
        // final primitives reject outright and the 'call' tearoff arm
        // (type_system.dart:906-916) decides from the method's own
        // signature (the oracle accepts a callable class instance where a
        // function type is expected, and rejects an incompatible one).
        // Every other interface source keeps the under-report `None`: the
        // slot also receives derived asks the oracle never makes (the
        // covariant element walk, the staged-mark channel the
        // closure-return face pins clean), and a bare rejection there
        // would report inference residue instead of the oracle's
        // silence. The bare `Function`-head context stays with the
        // ordinary named path below.
        if let Some(to_fn) = to_fn.as_ref() {
            return match from {
                Ty::Named {
                    name,
                    nullable: false,
                    args,
                } => self.call_method_assignable(name, args, to_fn),
                // A record source subtypes a function target in no case
                // (subtype.dart's record block falls to the same
                // `return false`).
                Ty::Record { .. } => Some(false),
                _ => None,
            };
        }
        if from_fn.is_some() {
            return match to {
                Ty::Named {
                    name,
                    nullable: false,
                    ..
                } => {
                    if name == self.intern("Object") || name == self.intern("Function") {
                        Some(true)
                    } else if self.members.primitive_ids.contains(&name)
                        || resolve_container(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            self.file,
                            name,
                        )
                        .is_some()
                    {
                        Some(false)
                    } else {
                        None
                    }
                }
                Ty::Record {
                    nullable: false, ..
                } => Some(false),
                _ => None,
            };
        }
        // The bare `Function` supertype's implicit-call face (#2921 B5 —
        // `isAssignableTo`'s callable-class acceptance,
        // type_system.dart:875-884 + `acceptsFunctionType`,
        // type_system.dart:121-126): the `Function` head accepts every
        // function type (`subtype.dart`'s `T1.isDartCoreFunction` arm
        // answers the tear-off), so a non-nullable interface source with a
        // `call` method assigns through the member's own type — and
        // `isAssignableTo(callMethodType, Function)` is unconditional
        // (`call` is a function-typed member, `Function` its supertype):
        // a resolvable `call` means the pair accepts outright, whatever
        // its signature. Both spellings — the bare head and a written
        // instantiation — run the SAME proof, [`Self::call_tear_off_verdict`]
        // (#3512: an args-conditioned second path is how the instantiated
        // head drifted into a false `invalid_assignment`).
        if let Ty::Named {
            name,
            nullable: false,
            args: ArgListId::EMPTY,
        } = to
            && name == self.intern("Function")
            && let Ty::Named {
                nullable: false, ..
            } = from
        {
            return self.call_tear_off_verdict(from);
        }
        let (
            Ty::Named {
                name: from_name,
                nullable: false,
                args: from_args,
            },
            Ty::Named {
                name: to_name,
                nullable: false,
                args: to_args,
            },
        ) = (from, to)
        else {
            return None;
        };
        if from_name == to_name {
            // Same head, differing written arguments: the covariant
            // interface rule decides through the boundary pool.
            return self.covariant_args(from_name, from_args, to_args);
        }
        // A type-parameter target (subtype.dart's Left Variable arm —
        // `T0 <: T1` where `T1` is a `TypeParameterType` is decided ONLY by
        // the promoted-bound/reflexivity arms: an interface, record, or
        // function source never subtypes a bare parameter, so the question
        // is decidable-false the moment the target spells one. The
        // source-side twin (a param source against a param target —
        // `S <: T`) stays undecidable: the bound chain is the M1 model's
        // blind spot. Param-ness reads the LIVE sets only — a name in the
        // file's decl table is NOT enough: a type alias's parameter or a
        // foreign callee's solved formal both spell a `MicroKind::TypeParam`
        // name into this slot without being a target the Left Variable arm
        // decides. The declared-return teardown carries its own param
        // detection (the staged `Return` re-decision — `filter_staged`),
        // where the target is provably the executable's declared return.
        let to_is_param =
            self.type_params.contains(&to_name) || self.pending_type_params.contains(&to_name);
        if to_is_param {
            let from_is_param = self.type_params.contains(&from_name)
                || self.pending_type_params.contains(&from_name);
            return if from_is_param { None } else { Some(false) };
        }
        // Distinct non-primitive heads: the supertype-graph walk decides.
        // A definite miss is the reportable assignability face
        // (`return_of_invalid_type`, `argument_type_not_assignable`,
        // `invalid_assignment`); an open direction (an unindexed edge the
        // oracle's full world could relate) stays the under-report `None`.
        // The top-type target short-circuits first — every class is a
        // subtype of `Object` whether or not the index carries its
        // declaration.
        if !self.members.primitive_ids.contains(&from_name)
            && !self.members.primitive_ids.contains(&to_name)
        {
            if to_name == self.intern("Object") {
                return Some(true);
            }
            return implements_interface(
                self.graph,
                self.namespaces,
                self.index,
                self.strings,
                self.lib,
                self.file,
                from_name,
                to_name,
                true,
            );
        }
        // dart:core's collection heads are final classes disjoint from the
        // final primitives: no subclass of one can ever be the other, so a
        // collection-against-primitive pair decides negatively (a
        // `List<dynamic>` source against an `int` host — `int f() => []` —
        // is the return check's decidable rejection, not a blind spot).
        // The async heads carry the same disjointness (`Stream`/`Future`
        // are SDK-declared interfaces, so [`Engine::declared_outside_core`]
        // leaves them open — but `int`/`String`/`bool` are final and never
        // a `Stream` or `Future`; the yield-each raw face
        // `isAssignableTo('String', 'Stream<int>')` is the oracle's
        // definite rejection).
        let from_primitive = self.members.primitive_ids.contains(&from_name);
        let to_primitive = self.members.primitive_ids.contains(&to_name);
        let collections = &self.collection_heads;
        let async_heads = [self.intern("Stream"), self.intern("Future")];
        if (collections.contains(&from_name) && to_primitive)
            || (from_primitive
                && (collections.contains(&to_name) || async_heads.contains(&to_name)))
        {
            return Some(false);
        }
        if !from_primitive || !to_primitive {
            // One side a core primitive, the other a declared container:
            // the primitives' closed lattice decides the negative
            // ([`Self::declared_outside_core`]); anything else stays
            // undecided.
            let other = if from_primitive { to_name } else { from_name };
            return self.declared_outside_core(other).then_some(false);
        }
        let (int, double, num) = (
            self.members.primitive_ids[0],
            self.members.primitive_ids[1],
            self.members.primitive_ids[2],
        );
        if (from_name == int || from_name == double) && to_name == num {
            return Some(true);
        }
        if from_name == int && to_name == double {
            // `analyzeIntLiteral`'s `convertToDouble`: the literal converts,
            // the value does not.
            return Some(from_literal);
        }
        Some(false)
    }

    /// Whether a parameter's declared bound mentions the parameter itself
    /// anywhere in its shape — the `X extends FutureOr<X>` / `X extends A<X>`
    /// spellings. The type-variable source arm refuses such a bound: asking
    /// `B <: T1` would meet the same `X` again and re-enter the arm forever.
    /// A function- or record-shaped bound declines outright (arbitrarily
    /// nested, and the shape pools make an exact walk dear for a face this
    /// rare); the depth cap is the belt-and-braces guard for a self-referential
    /// argument list.
    fn bound_mentions_param(&self, ty: Ty, name: StringId, depth: u32) -> bool {
        if depth > 8 {
            return true;
        }
        match ty {
            Ty::Named {
                name: head, args, ..
            } => {
                if head == name {
                    return true;
                }
                // Own the list before recursing: the pool guard borrows the
                // args cell, and the walk re-enters it (`assignable`'s own
                // invariant, #2594).
                let args = self.boundary.pool().get(args).to_vec();
                args.iter()
                    .any(|arg| self.bound_mentions_param(*arg, name, depth + 1))
            }
            Ty::Function { .. } | Ty::Record { .. } => true,
            _ => false,
        }
    }

    /// The `call` tear-off verdict for a non-nullable interface source
    /// facing the bare `Function` head — `isAssignableTo`'s callable
    /// acceptance (type_system.dart:875-884 + `acceptsFunctionType`
    /// :121-126). ONE proof serves the bare head and a written
    /// instantiation alike.
    ///
    /// Upstream asks one fact — is `getCallMethodType` non-null
    /// (type_system.dart:559-566: `call` looked up through the inheritance
    /// chain and, being a member of the head, instantiated by the
    /// receiver's own type arguments)? The follow-up
    /// `isAssignableTo(callMethodType, Function)` is unconditional — `call`
    /// is a function-typed member and `Function` the supertype of every
    /// function type — so the signature this arm would have to substitute
    /// never decides anything. The existence fact is likewise independent
    /// of the written instantiation: `Function a = C0()` and
    /// `Function b = C1<int>()` prove the same face. An args-conditioned
    /// second path is exactly how the instantiated head drifted into a
    /// false `invalid_assignment` (#3512), so both spellings come through
    /// here.
    ///
    /// - `Some(true)`: a non-static `call` **method** resolves on the
    ///   head's member face (alias spellings fold to the aliased head
    ///   first). A getter there is no dispatch method upstream
    ///   (`lookUpMethod` answers `InternalMethodElement?`,
    ///   type.dart:1011-1040) and a static member is no instance face
    ///   (`getMember3`) — `language/function/type_call_getter2_test` pins
    ///   the rejection.
    /// - `Some(false)`: the head resolves to a container whose member face
    ///   provably carries no usable `call` — the final primitives included
    ///   (their member surface is compiler-known and carries none).
    /// - `None`: undecidable — an unresolvable head (a foreign spelling
    ///   the index does not carry, or the derived-ask residue this slot
    ///   also receives), or a broken supertype chain
    ///   ([`Walk::ChainBroken`]: the member may live past the hop we
    ///   cannot resolve). A bare rejection there would report a legal
    ///   program, the same under-report posture
    ///   [`Self::call_method_assignable`] documents for the shaped face.
    fn call_tear_off_verdict(&self, from: Ty) -> Option<bool> {
        let Ty::Named {
            name,
            nullable: false,
            ..
        } = self.fold_alias_ty(from)
        else {
            return None;
        };
        if self.members.primitive_ids.contains(&name) {
            return Some(false);
        }
        match self.walk_members(name, self.intern("call")) {
            Walk::Found(entry) if entry.kind == MemberKind::Method && !entry.is_static => {
                Some(true)
            }
            Walk::Found(_) | Walk::Missing => resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                name,
            )
            .is_some()
            .then_some(false),
            Walk::ChainBroken => None,
        }
    }

    /// The `call` member signature of a callable-class head, instantiated
    /// by the written use's type arguments — `implicit_call_face`'s
    /// EMPTY-args face plus the `getCallMethodType` substitution upstream
    /// runs unconditionally (type_system.dart:876-885 reads
    /// `callMethodType` with the receiver's own substitution; there is no
    /// `args == EMPTY` gate in `acceptsFunctionType`). An EMPTY source
    /// delegates to [`Engine::implicit_call_face`] unchanged. A written
    /// instantiation (`C<int>`) resolves the head's declaration parameter
    /// spellings, zips them against the written arguments, and substitutes
    /// through the walked member's formals and return — `T call(T)` over
    /// `class C<T>` reads `int call(int)` at the `C<int>` use. `None` when
    /// the member lookup finds no `call` (the caller's resolvable-or-not
    /// split decides) or when a signature leaf cannot take the
    /// substitution ([`Engine::subst_params`]'s `None` — a nullable
    /// parameter-lift the projection cannot spell, or a parameter the
    /// declaring container's spellings never named, e.g. an inherited
    /// `call` whose own container's parameters differ): the under-report
    /// direction.
    fn instantiated_call_face(
        &self,
        name: StringId,
        args: ArgListId,
    ) -> Option<(Formals, Ty, Box<[(StringId, Option<Ty>)]>)> {
        if args == ArgListId::EMPTY {
            return self.implicit_call_face(Ty::Named {
                name,
                nullable: false,
                args,
            });
        }
        let written: Vec<Ty> = self.boundary.pool().get(args).to_vec();
        let (container, _owner) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )?;
        if container.type_param_names.len() != written.len() {
            return None;
        }
        let call = self.intern("call");
        let Walk::Found(entry) = self.walk_members(name, call) else {
            return None;
        };
        if entry.kind != MemberKind::Method || entry.is_static {
            return None;
        }
        // The container's own parameters bind the written use's
        // arguments — minus any name the `call` method itself declares
        // (`T call<T>(T)` shadows the class `T` inside its own
        // signature; the class map must not touch it,
        // `getCallMethodType`'s substitution keeps the method formals'
        // own parameters intact).
        let map: Vec<(StringId, Ty)> = container
            .type_param_names
            .iter()
            .copied()
            .zip(written)
            .filter(|(param, _)| !entry.type_param_names.contains(param))
            .collect();
        let (formals, return_ty, params) = match &entry {
            Projected::Own(e) => (
                e.formals.as_deref()?.clone(),
                e.return_ty,
                e.type_param_names
                    .iter()
                    .enumerate()
                    .map(|(index, param)| {
                        (
                            *param,
                            e.type_param_bounds
                                .get(index)
                                .copied()
                                .filter(|bound| *bound != Ty::Dynamic),
                        )
                    })
                    .collect(),
            ),
            Projected::Foreign(e) => (
                e.formals.as_deref()?.to_owned(),
                e.return_ty,
                e.type_param_names
                    .iter()
                    .enumerate()
                    .map(|(index, param)| {
                        (
                            *param,
                            e.type_param_bounds
                                .get(index)
                                .copied()
                                .filter(|bound| *bound != Ty::Dynamic),
                        )
                    })
                    .collect(),
            ),
        };
        let positional: Option<Box<[Formal]>> = formals
            .positional
            .iter()
            .map(|formal| {
                Some(Formal {
                    name: formal.name,
                    ty: self.subst_params(formal.ty, &map)?,
                    required: formal.required,
                    covariant: formal.covariant,
                    default_value: formal.default_value,
                })
            })
            .collect();
        let named: Option<Box<[Formal]>> = formals
            .named
            .iter()
            .map(|formal| {
                Some(Formal {
                    name: formal.name,
                    ty: self.subst_params(formal.ty, &map)?,
                    required: formal.required,
                    covariant: formal.covariant,
                    default_value: formal.default_value,
                })
            })
            .collect();
        let params: Box<[(StringId, Option<Ty>)]> = params;
        Some((
            Formals {
                positional: positional?,
                named: named?,
            },
            self.subst_params(return_ty, &map)?,
            params,
        ))
    }

    /// The 'call' tearoff arm of the cross-family function face
    /// (`isAssignableTo`'s callable-class acceptance,
    /// type_system.dart:906-916): the non-nullable interface head `name`
    /// is assigned into the function shape `to_fn`. The rejection fires
    /// only where the facts are two-sided: the final primitives (whose
    /// member surface is compiler-known — no `call`, never a subtype of a
    /// function type) reject outright, and a written `call` method's own
    /// signature decides (compatible assigns, incompatible reports).
    /// Every other interface source keeps the under-report `None`: a
    /// resolvable class without `call` decides `false` upstream, but this
    /// slot also receives derived asks the oracle never makes (the
    /// covariant element walk's slot pairs, the staged-mark channel the
    /// closure-return face pins clean), and a bare rejection there would
    /// report inference residue instead of the oracle's silence. A
    /// generic instantiation needs the call method's own instantiation,
    /// and a generic `call` the inference the declared facts do not
    /// carry — both under-report.
    fn call_method_assignable(
        &self,
        name: StringId,
        args: ArgListId,
        to_fn: &FnVarSig,
    ) -> Option<bool> {
        if self.members.primitive_ids.contains(&name) {
            return Some(false);
        }
        let Some((formals, return_ty, params)) = self.instantiated_call_face(name, args) else {
            // A resolvable container with provably no `call` member
            // rejects the written ask: upstream's `callMethodType` is
            // null there, so `isAssignableTo` falls through to `false`
            // (type_system.dart:910-915 — `String Function(String) f =
            // C()` over `class C {}` reports). An UNRESOLVABLE head keeps
            // the under-report `None` (the derived-ask residue this slot
            // also receives), as does an instantiated source whose
            // member lookup cannot prove the face — a broken
            // supertype chain, a `call` carried by an unindexed
            // declaration, or a signature leaf [`Engine::subst_params`]
            // cannot spell is not the no-`call` fact.
            if args != ArgListId::EMPTY {
                let Some(_) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    name,
                ) else {
                    return None;
                };
                let call = self.intern("call");
                return match self.walk_members(name, call) {
                    // A clean miss — or a `call` that is no method
                    // (a field, a getter) — is the null `callMethodType`
                    // upstream falls to `false` on.
                    Walk::Missing => Some(false),
                    Walk::Found(entry) => {
                        (entry.kind != MemberKind::Method || entry.is_static).then_some(false)
                    }
                    Walk::ChainBroken => None,
                };
            }
            return resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                name,
            )
            .is_some()
            .then_some(false);
        };
        // A generic `call`'s type parameters ride the signature —
        // `acceptsFunctionType`'s `callMethodType` keeps them, and
        // `isFunctionSubtypeOf`'s `relateTypeParameters` arm (the
        // parameter-count + bound-equality ask [`Self::fn_sig_subtype`]
        // runs first) decides the pair exactly like two generic function
        // types. A target written without parameters rejects a generic
        // source outright (the count arm).
        let call_sig = FnVarSig {
            params,
            required_positional: u16::try_from(
                formals.positional.iter().filter(|f| f.required).count(),
            )
            .unwrap_or(u16::MAX),
            positional: formals.positional.iter().map(|f| f.ty).collect(),
            named: formals
                .named
                .iter()
                .map(|f| (f.name, f.ty, f.required))
                .collect(),
            return_ty,
        };
        // A generic `call` in a non-generic context instantiates before the
        // subtype ask (`getCallMethodType`, type_system.dart:528-535 —
        // `instantiateTypeFormalsToBounds` fills each method type parameter
        // with its written bound, `dynamic` unwritten). The first try is
        // the same downward tear-off inference the `assignable` entry runs
        // over two function-shaped written types — the context signature's
        // structurally-paired types solve the parameters
        // (`instantiated_fn_sig_subtype`): `int Function(int) f = C()` over
        // `T call<T>(T)` solves `T := int` and accepts, which the
        // `relateTypeParameters` count gate below would refuse outright.
        // When the context cannot solve — an arity the instantiation can
        // never bridge, an unbound parameter, a solution that contradicts
        // the declared bound — the oracle's `callMethodType` falls back to
        // the bounds face: `T call<T extends num>(T)` reads `num
        // Function(num)` (the `String Function(String)` target's `T :=
        // String` solve violates `extends num`, and the reported face is
        // the bound `num`), `void Function() f = C()` reads `dynamic
        // Function(dynamic)` (no positional pairs the context).
        if !call_sig.params.is_empty() && to_fn.params.is_empty()
            && let Some(verdict) = self.instantiated_fn_sig_subtype(&call_sig, to_fn)
        {
            return Some(verdict);
        }
        let call_sig = if call_sig.params.is_empty() {
            call_sig
        } else {
            let bounds: FxHashMap<StringId, Ty> = call_sig
                .params
                .iter()
                .map(|(name, bound)| (*name, bound.unwrap_or(Ty::Dynamic)))
                .collect();
            FnVarSig {
                params: Box::default(),
                required_positional: call_sig.required_positional,
                positional: call_sig
                    .positional
                    .iter()
                    .map(|ty| self.subst_ty(*ty, &bounds))
                    .collect(),
                named: call_sig
                    .named
                    .iter()
                    .map(|(name, ty, required)| {
                        (*name, self.subst_ty(*ty, &bounds), *required)
                    })
                    .collect(),
                return_ty: self.subst_ty(call_sig.return_ty, &bounds),
            }
        };
        self.fn_sig_subtype(&call_sig, to_fn)
    }

    /// The covariant interface rule for one generic head —
    /// `<C<S…>> C <: <C<T…>> C` needs every `S` to fit its `T`
    /// (`subtype.dart`'s interface case): `List<int>` fits `List<num>`,
    /// `List<num>` does not fit `List<int>`. The two slots reached here
    /// through one boundary pool (own types intern in the seeded pool,
    /// foreign ones project), so unequal ids mean genuinely differing
    /// written arguments, and each element verdict re-enters
    /// [`Engine::assignable`] — nested heads recurse, the primitive edges
    /// and the strict-casts overlay apply to elements unchanged. One
    /// decidable mismatch breaks the pair; one undecidable element (a
    /// distinct-name pair an unseen subclass edge could still relate)
    /// suppresses the whole — the under-report direction. An arity
    /// mismatch between two WRITTEN instantiations is the `wrong_number`
    /// family's written residue; the assignability face stays silent on
    /// it. A raw head against a written instantiation is different
    /// residue: the raw side reads as its instantiate-to-bounds defaults
    /// (`instantiateToBounds`, type_system.dart:719-731 — the oracle's
    /// resolved types never disagree in arity, #2838), filled through the
    /// bounds snapshot's fixpoint and decided by the same element rule.
    pub(crate) fn covariant_args(
        &self,
        head: StringId,
        from: ArgListId,
        to: ArgListId,
    ) -> Option<bool> {
        // The narrow borrow ([`Engine::function_shape_sig`]'s pattern): the
        // slots copy out and the pool guard drops before the element
        // verdicts — each re-enters [`Engine::assignable`] and the chain
        // below it interns into the same pool (`intern_list`'s
        // `borrow_mut`), which a live `Ref` here would panic against.
        let (from, to) = {
            let pool = self.boundary.pool();
            (pool.get(from).to_vec(), pool.get(to).to_vec())
        };
        let (from, to) = if from.len() != to.len() {
            // The raw-type arm (#2838): exactly one side EMPTY. A
            // written-vs-written mismatch stays the `wrong_number`
            // family's, and a fill that misses the written count (a
            // foreign head, a poisoned bound) keeps the silence.
            let (raw_first, written): (bool, Vec<Ty>) = match (from.is_empty(), to.is_empty()) {
                (true, false) => (true, to),
                (false, true) => (false, from),
                _ => return None,
            };
            let defaults = self.bounds.itb_default_args(self.strings, head)?;
            if defaults.len() != written.len() {
                return None;
            }
            let filled: Vec<Ty> = defaults.iter().map(|tree| self.tree_ty(tree)).collect();
            if raw_first {
                (filled, written)
            } else {
                (written, filled)
            }
        } else {
            (from, to)
        };
        if from.len() != to.len() {
            return None;
        }
        // The head's declared-variance facts (this file's capture; a
        // foreign head reads all-covariant — the legacy default,
        // element.dart:11560-11562). The interface-argument relation runs
        // per parameter (subtype.dart:44-83 `_interfaceArguments`): a
        // legacy/`out` parameter asks left→right, an `in` parameter
        // right→left (`C<A>` <: `C<B>` iff `B <: A`), an `inout` parameter
        // both ways. The `in` inversion composes with the function-shape
        // face's own formal contravariance — the two flips multiply into
        // the oracle's override posture (an `in`-parameter formal accepts
        // the argument whose head argument sits BELOW the target's).
        let variances: Vec<Option<Variance>> = self
            .var_containers
            .iter()
            .find(|container| container.name == head)
            .map_or_else(Vec::new, |container| {
                container
                    .params
                    .iter()
                    .map(|(_, declared, _)| *declared)
                    .collect()
            });
        let mut unknown = false;
        for (index, (&f, &t)) in from.iter().zip(to.iter()).enumerate() {
            // The generic-method inference face: a `dynamic` argument into
            // an unsolved formal's own type-variable slot never reaches the
            // subtype question — the call-site inference binds the variable
            // from the argument's static type first
            // (`node.accept(visitor)` over `accept<E>(AstVisitor<E>)` with
            // an `AstVisitor<dynamic>` argument solves `E = dynamic`, and
            // the written pair `AstVisitor<dynamic>`/`AstVisitor<E>` the
            // matrix sees is inference residue, not a mismatch;
            // type_analyzer.dart's `InferenceContext` grounds the formal
            // before any assignability ask). Only the exact unsolved
            // residue qualifies: a non-primitive, non-collection head that
            // resolves to no container AND is no in-scope type parameter —
            // an in-scope parameter is a real type variable, not residue:
            // the enclosing class's own `E` instantiates independently of
            // this send's solve, and the oracle keeps the mismatch
            // (`StateController<dynamic>` into `StateController<ValueT>`,
            // #2687 — `dynamic` is not a subtype of the variable). A
            // WRITTEN `Never` slot is no residue either: `Never` answers
            // `unbound_formal_head` (not primitive, not a collection head,
            // resolves to no container) but it is a real bottom-type
            // argument — `A<dynamic>` into `A<Never>` refuses because
            // `dynamic <: Never` is false (#3589's co19 instantiate-to-
            // bounds family: the I2B `Never` default lands in a written
            // arg shape and the oracle reports `invalid_assignment`).
            if f == Ty::Dynamic
                && matches!(
                    t,
                    Ty::Named {
                        nullable: false,
                        args: ArgListId::EMPTY,
                        ..
                    }
                )
                && let Ty::Named { name, .. } = t
                && !self.type_params.contains(&name)
                && !self.is_never_ty(t)
                && self.unbound_formal_head(name)
            {
                continue;
            }
            // The covariant-interface element rule is the subtype question
            // (`Ctrl<dynamic>` does not fit `Ctrl<int>`: `dynamic <: int`
            // is false — the expression-position downcast leniency never
            // enters a type's argument shape), asked in the parameter's
            // declared direction.
            let declared = variances.get(index).copied().flatten();
            let verdict = match declared {
                Some(Variance::Contravariant) => self.slot_subtype(t, f),
                Some(Variance::Invariant) => {
                    match (self.slot_subtype(f, t), self.slot_subtype(t, f)) {
                        (Some(false), _) | (_, Some(false)) => Some(false),
                        (Some(true), Some(true)) => Some(true),
                        _ => None,
                    }
                }
                _ => self.slot_subtype(f, t),
            };
            match verdict {
                Some(false) => return Some(false),
                Some(true) => {}
                None => unknown = true,
            }
        }
        (!unknown).then_some(true)
    }

    /// The return type of a function-shaped written type — the
    /// [`Engine::function_shape_sig`] answer's own return. `None` for
    /// every non-function shape.
    pub(crate) fn function_shape_return(&self, ty: Ty) -> Option<Ty> {
        Some(self.function_shape_sig(ty)?.return_ty)
    }

    /// The written function shape of a type, aliases unfolded: a
    /// `Ty::Function` reads its pool shape, a named head answers through
    /// the alias faces — the use-site-instantiated parameterized alias
    /// ([`Engine::instantiated_alias_sig`]) or the written-shape faces
    /// (`alias_fn_sig`: raw heads, co-alias chains, the SDK
    /// `Comparator`). `None` for every other shape — the caller keeps its
    /// ordinary path. The alias gates read the parse-time capture first,
    /// so the non-alias matrix pairs pay two hash probes, not a
    /// resolution walk.
    fn function_shape_sig(&self, ty: Ty) -> Option<Box<FnVarSig>> {
        match ty {
            Ty::Function {
                shape,
                nullable: false,
            } => {
                let (type_params, type_params_named, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.type_params_named.clone(),
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                // The interned shape's own `<T extends B>` group rides the
                // signature when the producer recorded it (#2878's
                // `type_params_named` column — tear-offs and the folded
                // parameterized-alias faces carry names and written
                // bounds, which `fn_sig_subtype`'s `relateTypeParameters`
                // equality leg decides over; a count-only shape keeps the
                // anonymous entries — no name, no bound, never deciding.
                let params: Vec<(StringId, Option<Ty>)> = if type_params_named.is_empty() {
                    (0..usize::from(type_params))
                        .map(|_| (StringId::NONE, None))
                        .collect()
                } else {
                    type_params_named.into()
                };
                Some(Box::new(FnVarSig {
                    params: params.into(),
                    required_positional: u16::try_from(positional.len()).unwrap_or(u16::MAX),
                    positional: positional.into(),
                    named: named.iter().map(|(name, ty)| (*name, *ty, false)).collect(),
                    return_ty,
                }))
            }
            Ty::Named {
                name,
                nullable: false,
                args,
            } => {
                // A capture answers a function-RHS alias directly; a
                // named-RHS one joins the co-alias chain the signature
                // face chases (`typedef B = A<int>` over
                // `A<X> = X Function()`). No capture — a use before this
                // file's declaration, a foreign head, or no alias at all —
                // keeps the ordinary path (the under-report direction the
                // pre-face verdicts already had).
                let capture = self.bounds.alias_fn_shape(name).is_some()
                    || self.bounds.alias_named_shape(name).is_some();
                if !capture {
                    return None;
                }
                if args == ArgListId::EMPTY {
                    self.alias_fn_sig(name)
                } else {
                    self.instantiated_alias_sig(name, args)
                }
            }
            _ => None,
        }
    }

    /// A use-site-instantiated parameterized function-type alias's shape
    /// (`Pred2<String>` over `typedef Pred2<T> = bool Function(T)`): the
    /// written right-hand shape with the alias's own type parameters
    /// substituted by the written arguments — the instantiation the
    /// written-shape face cannot carry (its leaves stay the parameter
    /// names). The RHS's own type variables ride the instantiated
    /// signature (#2838: `F<int>` over `typedef F<X> = void Function<Y
    /// extends X>()` reads `void Function<Y extends int>()` — the count
    /// and equality rules of [`Engine::fn_sig_subtype`] decide over the
    /// real populated params), each bound substituted like every other
    /// RHS piece; a poisoned bound reads unwritten (`None` — the
    /// `dynamic` arm). `None` on an arity residue (the `wrong_number`
    /// family's business), when the written capture failed, or when the
    /// head is no function-RHS alias.
    pub(crate) fn instantiated_alias_sig(
        &self,
        name: StringId,
        args: ArgListId,
    ) -> Option<Box<FnVarSig>> {
        let (params, rhs_vars): (Box<[StringId]>, Box<[(StringId, Option<TypeTree>, bool)]>) =
            match self.bounds.alias_fn_shape(name) {
                Some((params, rhs_vars, ..)) => (
                    params.iter().map(|(param, ..)| *param).collect(),
                    rhs_vars.iter().cloned().collect(),
                ),
                None => return None,
            };
        let written: Vec<Ty> = {
            let pool = self.boundary.pool();
            pool.get(args).to_vec()
        };
        if written.len() != params.len() {
            return None;
        }
        let map: FxHashMap<StringId, Ty> = params.iter().copied().zip(written).collect();
        let sig = self.alias_fn_sig(name)?;
        Some(Box::new(FnVarSig {
            params: rhs_vars
                .iter()
                .map(|(var, bound, poisoned)| {
                    let bound = if *poisoned {
                        None
                    } else {
                        bound
                            .as_ref()
                            .map(|tree| self.subst_ty(self.tree_ty(tree), &map))
                    };
                    (*var, bound)
                })
                .collect(),
            positional: sig
                .positional
                .iter()
                .map(|ty| self.subst_ty(*ty, &map))
                .collect(),
            required_positional: sig.required_positional,
            named: sig
                .named
                .iter()
                .map(|(name, ty, required)| (*name, self.subst_ty(*ty, &map), *required))
                .collect(),
            return_ty: self.subst_ty(sig.return_ty, &map),
        }))
    }

    /// Substitutes one alias instantiation map through a written type:
    /// parameter leaves become their arguments (the written `?` joins the
    /// argument's own nullability), named/function/record nodes rebuild
    /// with substituted children — one re-intern each, the canonical
    /// pools dedupe. A map miss passes through (a miss-spelled leaf is no
    /// parameter of this alias). Pool reads scope before the re-intern
    /// borrows (one `RefCell`, no overlap).
    fn subst_ty(&self, ty: Ty, map: &FxHashMap<StringId, Ty>) -> Ty {
        if let Ty::Named {
            name,
            nullable,
            args: ArgListId::EMPTY,
        } = ty
            && let Some(replacement) = map.get(&name)
        {
            return Self::join_nullable(*replacement, nullable);
        }
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } if args != ArgListId::EMPTY => {
                let written: Vec<Ty> = {
                    let pool = self.boundary.pool();
                    pool.get(args).to_vec()
                };
                let substituted: Vec<Ty> =
                    written.iter().map(|arg| self.subst_ty(*arg, map)).collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .named(name, nullable, &substituted)
            }
            Ty::Function { shape, nullable } => {
                let (type_params, type_param_names, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.type_params_named.clone(),
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                let positional: Vec<Ty> = positional
                    .iter()
                    .map(|ty| self.subst_ty(*ty, map))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|(name, ty)| (*name, self.subst_ty(*ty, map)))
                    .collect();
                // The written bounds substitute with the rest (a bound
                // naming the substituted parameter reads the rewritten
                // face). A count-only source keeps its count-only identity.
                let names: Vec<(StringId, Option<Ty>)> = type_param_names
                    .iter()
                    .map(|&(name, bound)| (name, bound.map(|b| self.subst_ty(b, map))))
                    .collect();
                // Own the return face before the re-intern guard:
                // `subst_ty` re-enters the pool in every compound arm
                // (`pool()`'s borrow), which a live `RefMut` here panics
                // against (#3202's frame pair — the same discipline as
                // `canonical_dotted_ty`'s function leg and #2992's
                // alias-fold rebuild).
                let return_ty = self.subst_ty(return_ty, map);
                if names.is_empty() && type_params > 0 {
                    self.boundary
                        .args
                        .borrow_mut()
                        .function(type_params, return_ty, &positional, &named, nullable)
                } else {
                    self.boundary.args.borrow_mut().function_generic(
                        &names,
                        return_ty,
                        &positional,
                        &named,
                        nullable,
                    )
                }
            }
            Ty::Record { shape, nullable } => {
                let (positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.record_shape(shape);
                    (shape.positional.to_vec(), shape.named.to_vec())
                };
                let positional: Vec<Ty> = positional
                    .iter()
                    .map(|ty| self.subst_ty(*ty, map))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|(name, ty)| (*name, self.subst_ty(*ty, map)))
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&positional, &named, nullable)
            }
            other => other,
        }
    }

    /// Joins a written `?` onto a type: the suffix face rides the named,
    /// record, and function shapes; the top and sentinel types ignore it.
    fn join_nullable(ty: Ty, nullable: bool) -> Ty {
        if !nullable {
            return ty;
        }
        match ty {
            Ty::Named { name, args, .. } => Ty::Named {
                name,
                nullable: true,
                args,
            },
            Ty::Record { shape, .. } => Ty::Record {
                shape,
                nullable: true,
            },
            Ty::Function { shape, .. } => Ty::Function {
                shape,
                nullable: true,
            },
            other => other,
        }
    }

    /// The unsolved-formal head test: a named head that is neither a final
    /// primitive nor a collection head and resolves to no container names
    /// a type parameter of some declaration — the residue an unsubstituted
    /// generic formal carries to the verdict (the same predicate the
    /// raw-generic face applies at the send walk).
    fn unbound_formal_head(&self, name: StringId) -> bool {
        !self.members.primitive_ids.contains(&name)
            && !self.collection_heads.contains(&name)
            && resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                name,
            )
            .is_none()
    }

    /// A primitive-literal row: [`Engine::push_primitive_row`] is the only
    /// `RowShape::Value` creator that carries a known final-primitive type
    /// (every other `Value` push holds `Unknown` or `Null`), so the shape
    /// plus a non-nullable final-primitive type identifies the int/double/
    /// bool/String literal rows the conversion decides on.
    pub(crate) fn primitive_literal(row: &Row, primitive_ids: &[StringId; 5]) -> bool {
        let Ty::Named {
            name,
            nullable: false,
            ..
        } = row.ty
        else {
            return false;
        };
        matches!(row.shape, RowShape::Value) && primitive_ids.contains(&name)
    }

    /// The `integer_literal_imprecise_as_double` face (`_checkForOutOfRange`'s
    /// double arm, error_verifier.dart:6789 — the informal spec at
    /// dart-lang/language#4): an integer literal whose conversion
    /// ([`Self::assignable`]'s literal arm, `analyzeIntLiteral`'s
    /// `convertToDouble`) just typed the value `double` must be exactly
    /// representable as an IEEE 754 double (`IntegerLiteralImpl
    /// .isValidAsDouble`, ast.dart:19479); an imprecise value reports at
    /// the literal token with the lexeme (`-`-prefixed under an
    /// immediately-negated operand) and the nearest representable double
    /// in the correction slot.
    ///
    /// Called only where a conversion ask resolved — the pair `int →
    /// double` over a primitive-literal row IS the double context
    /// (upstream keys on the literal's resolved `staticType == double`),
    /// so the gate re-derives the pair and no-ops everywhere else.
    pub(crate) fn check_imprecise_int_literal(
        &mut self,
        from: Ty,
        to: Ty,
        literal: bool,
        range: SourceRange,
    ) {
        if !literal {
            return;
        }
        let (
            Ty::Named {
                name: from_name,
                nullable: false,
                ..
            },
            Ty::Named {
                name: to_name,
                nullable: false,
                ..
            },
        ) = (from, to)
        else {
            return;
        };
        let (int, double) = (self.members.primitive_ids[0], self.members.primitive_ids[1]);
        if from_name != int || to_name != double {
            return;
        }
        // The lexeme keeps its separators for the message (the oracle's
        // `{0}` spells them, the withSeparators census row); the predicate
        // strips them first (upstream `stripSeparators`).
        let start = self.byte_of(range.offset);
        let end = self.byte_of(range.end());
        let Some(lexeme) = self.source.get(start..end) else {
            return;
        };
        let stripped: String = lexeme.chars().filter(|c| *c != '_').collect();
        if valid_as_double(&stripped) {
            return;
        }
        // `immediatelyNegated` (ast.dart:19437): the literal is the operand
        // of a unary `-`. The row stack cannot answer the parent shape, but
        // a primitive-literal row only ever sits under a plain literal or
        // that negation (a binary `-` folds its operand rows), so the
        // source walk is exact.
        let negated = lexeme_negated(self.source.as_bytes(), start);
        let code = SemanticCode::IntegerLiteralImpreciseAsDouble;
        let literal_text = if negated {
            format!("-{lexeme}")
        } else {
            lexeme.to_string()
        };
        let message = code.message(&literal_text, "");
        let closest = nearest_valid_double(&stripped);
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            message,
            self.file,
            range,
            code.correction_message(&closest),
        ));
    }

    /// The enum-constant constructor selector (`c.other()` in the
    /// element's own position): the enclosing enum's constructor face
    /// decides (`parseEnumElement`, the `ConstructorReferenceContinuation`
    /// resolution). Unresolvable enum containers suppress.
    /// The enum's unnamed-constructor face at the constant check: the
    /// collector's flag (the `.new` spelling registers the named selector
    /// `new`), plus the diagnoser's `new.head` recovery kill
    /// (`new.named();` names a constructor upstream, so the implicit
    /// unnamed face is gone). An unresolvable container keeps the face —
    /// the under-report direction.
    pub(crate) fn enum_has_unnamed_constructor(&self, container: StringId) -> bool {
        if self.ctor_new_head_kills.contains(&container) {
            return false;
        }
        // The file's own constructor sites are authoritative for the
        // unnamed face: a written `E()` registers under the container's
        // spelling, `E.new()` under `new` — the same facts the element
        // model's `has_unnamed_constructor` carries, without the
        // collector's capture-fold loss (a later named constant can
        // collapse the flag while the site ledger still holds `E()`).
        let site_has_unnamed = self.decl_sites.iter().any(|site| {
            matches!(
                site.kind,
                DeclKind::Constructor { container: c } if c == container
            ) && (self.bare_name(site.name) == container
                || self.is_nameless(site.name)
                || self.is_bare_new_head(site.name)
                || site.name == self.intern("new"))
        });
        if site_has_unnamed {
            return true;
        }
        match resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        ) {
            Some((resolved, _)) => {
                resolved.has_unnamed_constructor
                    || resolved.constructors.contains(&self.intern("new"))
            }
            None => true,
        }
    }

    pub(crate) fn check_enum_constructor_selector(&mut self, name: StringId, range: SourceRange) {
        let Some((enum_name, _)) = self.container_context() else {
            return;
        };
        let Some((container, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            enum_name,
        ) else {
            return;
        };
        if container.constructors.contains(&name) {
            // The resolved selector is the constructor's use (the official
            // constructor-reference arm of `_isUsedMember`) — it spares the
            // declaration's `unused_element` face.
            self.used_names.insert(name);
            return;
        }
        // The miss still builds the constant's element upstream — its
        // `constructor` field falls back to the unnamed constructor, so
        // that constructor counts as used (`E()` of `enum E { v<int>.; ...
        // const E(); const E.foo(); }` — the constant references `E()`,
        // sparing its `unused_element` face while `foo` reports). The
        // constructor's written sites complete only at the enum's close,
        // so the mark defers there.
        self.enum_unnamed_ctor_uses.push(enum_name);
        let name_text = self.strings.resolve(name).to_string();
        self.emit(
            SemanticCode::UndefinedEnumConstructor,
            range,
            &name_text,
            "",
        );
    }

    /// The annotation name's resolution order (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/summary2/metadata_resolver.dart — member
    /// metadata resolves inside the container's body scope while head
    /// metadata resolves before that scope opens;
    /// pkg/analyzer/lib/src/dart/element/scope.dart — `InstanceScope` holds
    /// only the container's own members and `ExtensionScope` every own
    /// extension member, neither climbs the supertype chain;
    /// pkg/analyzer/lib/src/dart/resolver/annotation_resolver.dart — the
    /// annotation expression resolves like any constant-context expression
    /// and the lookup failure is that resolution's report): locals, the
    /// container's type parameters, the own-container member face (own
    /// statics for class/mixin/enum/extension-type bodies, every own member
    /// inside an extension body — the enum constants are static fields —
    /// plus the synthesized `values` face for enums), then the scoped
    /// top-level/import lookup. The climbed faces never answer an
    /// annotation name — a superclass `static const`, the own instance
    /// members of a class body, the mixin/extension `on`-type walks, the
    /// object members, the extension pool and the implicit-`this`
    /// extension application all stay missing (probe-verified against the
    /// 3.13.1 oracle), so `@internal` over a superclass
    /// `static const internal` reports `undefined_annotation`.
    fn annotation_name_missing(&self, name: StringId) -> bool {
        if self.frames.iter().rev().any(|f| f.locals.contains(&name))
            || self.initializer_locals.contains(&name)
            || self.type_params.contains(&name)
            || self.pending_type_params.contains(&name)
        {
            return false;
        }
        if let Some((container, kind)) = self.container_context() {
            if kind == ContainerKind::Enum && self.members.enum_synth_members.contains(&name) {
                return false;
            }
            // The unnamed extension's own members live in the frame's
            // face-rank map (`resolve_container` cannot serve the
            // placeholder name, #1701).
            let own = if kind == ContainerKind::Extension {
                self.unnamed_extension_members().or_else(|| {
                    resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        container,
                    )
                    .map(|(resolved, _)| &resolved.members)
                })
            } else {
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    container,
                )
                .map(|(resolved, _)| &resolved.members)
            };
            let answered = own.is_some_and(|members| {
                members.get(&name).is_some_and(|entry| {
                    // A class/mixin/enum/extension-type body sees only the
                    // own static members; an extension body sees every own
                    // member (scope.dart `InstanceScope` / `ExtensionScope`).
                    kind == ContainerKind::Extension || entry.is_static
                })
            });
            if answered {
                return false;
            }
        }
        // A top-level/import hit answers; an ambiguous import face keeps
        // the resolve direction (the same verdict `resolve_bare` takes). A
        // file-local `TypeParam` decl does not: the metadata scope carries
        // only the container's own type parameters (the layer above) — a
        // method's own `<T>` parses after its metadata and the oracle
        // reports the miss (probe-verified against the 3.13.1 oracle).
        let scoped = match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => decl.kind != MicroKind::TypeParam,
            ScopedHit::Found(_) | ScopedHit::Ambiguous(_) => true,
            ScopedHit::NotFound => false,
        };
        !scoped
    }

    /// A simple annotation name (`@foo`): the name resolves through the
    /// annotation scope order ([`Engine::annotation_name_missing`]); a fully
    /// missing name is `undefined_annotation`, emitted at `end_metadata`
    /// ([`Engine::emit_undefined_annotation`]) because the upstream row
    /// anchors on the whole Annotation node. A [`CORE_TOP_LEVEL_ANNOTATIONS`]
    /// name is defined by the implicit dart:core import, never missing.
    /// Defined names stay silent: the class/constructor second-part checks
    /// sit outside the M1 model (under-report). The metadata path skips the
    /// broken-import shown-name guard — the oracle's scope lookup reports
    /// the missing annotation regardless of `show` clauses
    /// (element.dart:8217-8250 suppresses the identifier face only;
    /// probe-verified both ways).
    pub(crate) fn check_annotation(&mut self, name: StringId, range: SourceRange) {
        let missing = self.annotation_name_missing(name)
            && !CORE_TOP_LEVEL_ANNOTATIONS.contains(&self.strings.resolve(name));
        if missing {
            self.record_miss(name, range, false, NameUse::Annotation);
        } else {
            // A resolved annotation is a use of the annotation name (the
            // official `lookupResult` on the metadata resolution's scope
            // lookup — scope.dart:292-306): it credits the import that
            // provided it (`unused_import`'s used set) and joins the
            // use-face, never the miss table.
            self.used_names.insert(name);
            self.annotation_uses.insert(name);
        }
    }

    /// The `undefined_annotation` row for the metadata entry that just
    /// closed. Upstream reports it `.at(node)` on the whole Annotation
    /// (`annotation_resolver.dart:245`) — `@` through the arguments'
    /// closing paren when present, else through the name — a span only the
    /// metadata end knows, so the emit lives here instead of
    /// [`Engine::check_annotation`]. The missing-name decision recomputes
    /// on the HEAD segment: the continuation arm rewrites the pending
    /// entry's name to the joined spelling (`A._ann`, `UseResult.unless`),
    /// while the check at the name event — and upstream's scope lookup —
    /// read the unqualified head.
    pub(crate) fn emit_undefined_annotation(&mut self, node: SourceRange) {
        let Some(pending) = self.pending_annotations.last() else {
            return;
        };
        let joined = self.strings.resolve(pending.name);
        let head = joined.split('.').next().unwrap_or(joined);
        if !(self.annotation_name_missing(self.strings.intern(head))
            && !CORE_TOP_LEVEL_ANNOTATIONS.contains(&head))
        {
            return;
        }
        // The oracle names the head identifier, never the joined spelling
        // (corpus rows: zero dotted names — `Undefined name 'p' used as an
        // annotation.` for `@p.C()` shapes).
        self.emit(SemanticCode::UndefinedAnnotation, node, head, "");
    }

    /// A value row with a resolved static type — the typed list literal's
    /// carrier (the `List<E>` read the assignability faces decide on).
    pub(crate) fn push_typed_value_row(&mut self, range: SourceRange, ty: Ty) {
        self.push_row(Row {
            name: self.intern(""),
            range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty,
        });
    }

    /// A literal whose type is a final primitive (`push_value_row` plus the
    /// type the static-nullability checks decide on).
    pub(crate) fn push_primitive_row(&mut self, token: Token, primitive: usize) {
        self.push_primitive_row_at(token.range, primitive);
    }

    /// The range-taking form: a synthesized extent wider than any single
    /// token (the interpolated literal's whole node span).
    pub(crate) fn push_primitive_row_at(&mut self, range: SourceRange, primitive: usize) {
        self.push_row(Row {
            name: self.intern(""),
            range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: Ty::named(self.members.primitive_ids[primitive]),
        });
    }
}

/// `TypeSystem::futureValueType` (analyzer source, type_system.dart
/// :502-529) at name level — the `async` return matrix's view of the
/// declared return (`T_v`). NOT the recursive `flatten` shape: the `?`
/// suffix strips first (:503-507), a `Future`/`FutureOr` head gives up
/// exactly one argument level without recursing (:509-515 —
/// `futureValueType(Future<Future<int>>)` is `Future<int>`), `dynamic`
/// and `void` pass through (:517-523), and everything else — `Null`,
/// `Never`, records, functions, any other head — maps to `Object?`
/// (:525-526). A bare `Future`/`FutureOr` spells the
/// instantiated-to-bounds `…<dynamic>` (the same face [`flatten_ty`]
/// applies), a malformed argument list keeps the written type whole (the
/// recovery guard [`flatten_ty`] shares), and an unknown declared type
/// stays unknown — the under-report direction the verdict's own
/// `Ty::Unknown` arm takes.
pub(crate) fn future_value_ty(strings: &StringTable, args: &crate::model::ArgPool, ty: Ty) -> Ty {
    let Ty::Named {
        name,
        nullable,
        args: list,
    } = ty
    else {
        return match ty {
            Ty::Dynamic | Ty::Void | Ty::Unknown => ty,
            _ => object_question(strings),
        };
    };
    if nullable {
        return future_value_ty(
            strings,
            args,
            Ty::Named {
                name,
                nullable: false,
                args: list,
            },
        );
    }
    if name == strings.intern("Future") || name == strings.intern("FutureOr") {
        if list == crate::model::ArgListId::EMPTY {
            return Ty::Dynamic;
        }
        let flat = args.get(list);
        if flat.len() == 1 {
            return flat[0];
        }
        return ty;
    }
    object_question(strings)
}

/// The `Object?` face `futureValueType` maps every non-`Future`,
/// non-top-type head to (type_system.dart:525-526).
fn object_question(strings: &StringTable) -> Ty {
    Ty::Named {
        name: strings.intern("Object"),
        nullable: true,
        args: crate::model::ArgListId::EMPTY,
    }
}

/// Upstream `IntegerLiteralImpl.isValidAsDouble` (ast.dart:19479) on exact
/// u32-limb arithmetic (the upstream body parses a `BigInt`; the
/// digit-string forms below decide the same steps without a bignum
/// dependency):
///
/// 1. fewer than 16 characters (separators stripped, the `0x` prefix
///    kept) is below `9007199254740992` / `0x10000000000000` — both 16
///    characters, 53 bits;
/// 2. a bit length ≤ 53 fits the double mantissa exactly;
/// 3. beyond `double.maxFinite` the exponent overflows; otherwise the low
///    `bitLength − 53` bits must all be zero (the mantissa's zero
///    extension is exact).
fn valid_as_double(stripped: &str) -> bool {
    if stripped.len() < 16 {
        return true;
    }
    let Some(limbs) = literal_limbs(stripped) else {
        return false;
    };
    let bits = limb_bit_length(&limbs);
    if bits <= 53 {
        return true;
    }
    if gt_max_finite(&limbs) {
        return false;
    }
    low_bits_zero(&limbs, bits - 53)
}

/// The literal's unsigned value as little-endian u32 limbs: hex packs
/// nibbles directly; decimal runs schoolbook long division by 2^32 (each
/// accumulator is bounded by (2^32 − 1)·10 + 9 < 2^36). `None` mirrors
/// the upstream `BigInt.tryParse` failure arm.
fn literal_limbs(stripped: &str) -> Option<Vec<u32>> {
    if let Some(rest) = stripped
        .strip_prefix("0x")
        .or_else(|| stripped.strip_prefix("0X"))
    {
        let mut out = vec![0u32; rest.len().div_ceil(8)];
        for (i, byte) in rest.bytes().rev().enumerate() {
            let nibble = (byte as char).to_digit(16)?;
            out[i / 8] |= nibble << (4 * (i % 8) as u32);
        }
        return Some(out);
    }
    if !stripped.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut cur: Vec<u8> = stripped.bytes().map(|b| b - b'0').collect();
    let mut out = Vec::new();
    while cur.iter().any(|&digit| digit != 0) {
        let mut rem: u64 = 0;
        for digit in &mut cur {
            let acc = rem * 10 + u64::from(*digit);
            *digit = (acc >> 32) as u8;
            rem = acc & 0xFFFF_FFFF;
        }
        out.push(rem as u32);
    }
    Some(out)
}

/// The value's bit length (the highest set bit's position plus one; zero
/// for the zero value).
fn limb_bit_length(limbs: &[u32]) -> u32 {
    let Some(top) = limbs.iter().rposition(|&limb| limb != 0) else {
        return 0;
    };
    32 * top as u32 + (32 - limbs[top].leading_zeros())
}

/// `value > double.maxFinite` (`2^1024 − 2^971`): the 53-bit run of ones
/// the exponent cap leaves starts at bit 971 — limb 31 is all ones, limb
/// 30 carries the run's low 21 bits at positions 11..31, every lower limb
/// is zero.
fn gt_max_finite(limbs: &[u32]) -> bool {
    const TOP: [u32; 2] = [0xFFFF_F800, 0xFFFF_FFFF];
    for i in (0..32).rev() {
        let limb = limbs.get(i).copied().unwrap_or(0);
        let max = if i >= 30 { TOP[i - 30] } else { 0 };
        if limb != max {
            return limb > max;
        }
    }
    false
}

/// Whether the low `k` bits are all zero — the exactness mask: the bits
/// the 53-bit mantissa drops contributed nothing.
fn low_bits_zero(limbs: &[u32], k: u32) -> bool {
    let full = (k / 32) as usize;
    if limbs.iter().take(full).any(|&limb| limb != 0) {
        return false;
    }
    let rest = k % 32;
    if rest == 0 {
        return true;
    }
    limbs.get(full).copied().unwrap_or(0) & ((1u32 << rest) - 1) == 0
}

/// Upstream `IntegerLiteralImpl.nearestValidDouble` (ast.dart:19532):
/// `min(double.maxFinite, value.toDouble())`, rendered as the exact
/// integer the double represents (`BigInt.from(...).toString()` — every
/// digit, never an exponent). The 54-bit mantissa extraction keeps the
/// oracle's single round-to-nearest-even step.
fn nearest_valid_double(stripped: &str) -> String {
    let value = literal_limbs(stripped)
        .map(|limbs| limbs_to_f64(&limbs))
        .unwrap_or(0.0);
    format!("{:.0}", f64::MAX.min(value))
}

/// The correctly rounded double for the limb value: direct up to 64 bits,
/// otherwise the top 54 bits (53 mantissa bits plus the round bit) with a
/// sticky tail, rounded to nearest-even and scaled by the exact power of
/// two.
fn limbs_to_f64(limbs: &[u32]) -> f64 {
    let bits = limb_bit_length(limbs);
    if bits == 0 {
        return 0.0;
    }
    if bits <= 64 {
        let mut value: u64 = 0;
        for limb in limbs.iter().rev() {
            value = (value << 32) | u64::from(*limb);
        }
        return value as f64;
    }
    let mut mant = 0u64;
    for i in 0..54 {
        mant = (mant << 1) | u64::from(limb_bit(limbs, bits - 1 - i));
    }
    let round = mant & 1;
    let sticky = (0..bits - 54).any(|i| limb_bit(limbs, i) == 1);
    let mut m = mant >> 1;
    if round == 1 && (sticky || m & 1 == 1) {
        // May carry to 2^53 — still exact as a float; the scale below
        // then overflows to infinity exactly when the value rounds past
        // the representable range.
        m += 1;
    }
    (m as f64) * 2f64.powi((bits - 53) as i32)
}

/// The bit at absolute position `index` of the limb value.
fn limb_bit(limbs: &[u32], index: u32) -> u32 {
    (limbs[(index / 32) as usize] >> (index % 32)) & 1
}

/// The `immediatelyNegated` face (ast.dart:19437): the closest
/// non-whitespace byte before the literal is the minus.
fn lexeme_negated(bytes: &[u8], start: usize) -> bool {
    let mut cursor = start;
    while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
        cursor -= 1;
    }
    cursor > 0 && bytes[cursor - 1] == b'-'
}

/// The `async` return-matrix pins: upstream's own exemplar faces
/// (`rtv.dart:258-289`), probed end-to-end through the single-file rig
/// below. The sync faces keep their shared `tests`-module pins — the sync
/// branches here are byte-identical to the pre-#2224 matrix.
#[cfg(test)]
mod async_return_tests {
    use super::*;
    use crate::collector::collect;
    use crate::model::ArgPool;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_resolution::LibraryGraphBuilder;
    use dartq_scanner::ScanConfig;
    use dartq_source::FileTable;
    use std::path::Path;

    pub(super) fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// The single-file rig (the shared `tests` module's
    /// `report_full_pipeline` shape, fixed knobs): scan, collect, one
    /// library, fold. The fixtures are parse-clean. Shared with this
    /// file's verdict-pin module below.
    pub(super) fn report(source: &str) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let fold = fold_unit(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        let mut report = fold.report;
        report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
        report
    }

    /// `Future<DB> f() async => DB();` is silent: `T_v` is `DB` and `DB`
    /// assigns to itself — under the sync matrix the same pair reported
    /// (`DB` fits no `Future<DB>` head). The async modifier picks the
    /// flattened face.
    #[test]
    fn async_return_of_future_value_stays_silent() {
        let report = report("class Future<T> {}\nclass DB {}\nFuture<DB> f() async => DB();");
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// The sync control: without `async`, the same declared shape still
    /// reports — the sync branches are untouched (bit-identical to the
    /// pre-#2224 verdict).
    #[test]
    fn sync_return_of_future_typed_expression_still_reports() {
        let report = report("class Future<T> {}\nclass DB {}\nFuture<DB> f() => DB();");
        assert_eq!(code_names(&report), ["return_of_invalid_type"]);
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'DB' can't be returned from the function 'f' because it has a return type of 'Future<DB>'."
        );
    }

    /// `Future<int> f() async { return 3 as Future<int>; }` is silent
    /// through branch (c)'s subtype leg: `flatten_S = int` subtypes
    /// `T_v = int` even though `S = Future<int>` never assigns to `int`
    /// (the assignable question misses, the subtype leg answers).
    #[test]
    fn async_return_subtype_leg_cleans_the_future_source() {
        let report = report(
            "class Future<T> {}\nclass int {}\n\
             Future<int> f() async { return 3 as Future<int>; }",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// The `T_v`-void branch: `Future<void> f() async { return 3 as
    /// Future<int>; }` reports — `flatten_S = int` is neither `void`,
    /// `dynamic`, nor `Null` — and the message keeps the RAW `S` and the
    /// declared `T` (`reportTypeError`, rtv.dart:155-205), never the
    /// projections.
    #[test]
    fn async_void_future_value_reports_the_raw_types() {
        let report = report(
            "class Future<T> {}\nclass int {}\n\
             Future<void> f() async { return 3 as Future<int>; }",
        );
        assert_eq!(code_names(&report), ["return_of_invalid_type"]);
        assert_eq!(
            report.diagnostics[0].message,
            "A value of type 'Future<int>' can't be returned from the function 'f' because it has a return type of 'Future<void>'."
        );
    }

    /// `futureValueType` unwraps exactly one level:
    /// `Future<Future<int>> f() async { return 3 as Future<int>; }` is
    /// silent — `T_v` is the inner `Future<int>` whole, and `S` assigns to
    /// it directly, where the sync matrix would have asked `int` against
    /// the declared `Future<Future<int>>` head.
    #[test]
    fn async_nested_future_declaration_unwraps_one_level() {
        let report = report(
            "class Future<T> {}\nclass int {}\n\
             Future<Future<int>> f() async { return 3 as Future<int>; }",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// A bare `return;` in an async body reads `futureValueType(T)`
    /// (return_type_verifier.dart `_checkReturnWithoutValue`:299-301):
    /// `Future<int>` unwraps to `int`, a non-ok-set face, and reports
    /// `return_without_value` at the keyword; the `Future<void>` host
    /// keeps it silent (`T_v = void` joins the ok-set).
    #[test]
    fn async_bare_return_follows_the_flattened_hosts() {
        let demands_a_value =
            report("class Future<T> {}\nclass int {}\nFuture<int> f() async { return; }");
        assert_eq!(code_names(&demands_a_value), ["return_without_value"]);
        let keeps_it_silent = report("class Future<T> {}\nFuture<void> f() async { return; }");
        assert!(
            code_names(&keeps_it_silent).is_empty(),
            "{:?}",
            code_names(&keeps_it_silent)
        );
    }

    /// The rig-free `futureValueType` shape: one unwrapping level, no
    /// recursion (`Future<Future<int>>` reads `Future<int>`).
    #[test]
    fn future_value_unwraps_exactly_one_level() {
        let (strings, mut args) = (StringTable::new(), ArgPool::default());
        let int = Ty::named(strings.intern("int"));
        let inner = args.named(strings.intern("Future"), false, &[int]);
        let nested = args.named(strings.intern("Future"), false, &[inner]);
        assert_eq!(future_value_ty(&strings, &args, nested), inner);
        assert_eq!(future_value_ty(&strings, &args, inner), int);
    }

    /// The suffix strips first, the top faces pass through, and every
    /// other head lands on `Object?`.
    #[test]
    fn future_value_strips_suffix_first_and_keeps_top_faces() {
        let (strings, mut args) = (StringTable::new(), ArgPool::default());
        let int = Ty::named(strings.intern("int"));
        let future_int_q = args.named(strings.intern("Future"), true, &[int]);
        assert_eq!(future_value_ty(&strings, &args, future_int_q), int);
        assert_eq!(future_value_ty(&strings, &args, Ty::Dynamic), Ty::Dynamic);
        assert_eq!(future_value_ty(&strings, &args, Ty::Void), Ty::Void);
        let object_q = Ty::Named {
            name: strings.intern("Object"),
            nullable: true,
            args: crate::model::ArgListId::EMPTY,
        };
        assert_eq!(future_value_ty(&strings, &args, int), object_q);
        assert_eq!(future_value_ty(&strings, &args, Ty::Null), object_q);
        assert_eq!(future_value_ty(&strings, &args, Ty::Never), object_q);
        let int_q = args.named(strings.intern("int"), true, &[]);
        assert_eq!(future_value_ty(&strings, &args, int_q), object_q);
        let unknown = future_value_ty(&strings, &args, Ty::Unknown);
        assert_eq!(unknown, Ty::Unknown);
    }
}

/// The element/argument verdict pins for this file's census shapes. The
/// rig is the async-return module's single-file pipeline, verbatim.
#[cfg(test)]
mod assignability_face_tests {
    use super::async_return_tests::report;

    /// The arrow-adjacent list literal (`seq(Parser other) => [this,
    /// other]`, petitparser's choice/sequence combinators): the switch
    /// arm's `>` is the arrow's, not a written `<...>` group close, so the
    /// element face never arms and the nearest earlier type-use (the
    /// pattern's head) never becomes the element type.
    #[test]
    fn arrow_adjacent_list_literal_has_no_element_face() {
        let report = report(
            "class Seq {}\n\
             class A {\n\
               A({required this.children});\n\
               final List<Seq> children;\n\
             }\n\
             List<Seq> f(Seq other) => switch (other) {\n\
               A(children: final children) => [\n\
                 ...children,\n\
                 other,\n\
               ],\n\
               _ => [other, other],\n\
             };",
        );
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|d| d.code.name == "list_element_type_not_assignable"),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| (&d.code.name, &d.message))
                .collect::<Vec<_>>()
        );
    }

    /// The written-group control: a real `<String>[...]` literal still
    /// checks its elements against the declared type. Oracle-pinned
    /// (dart analyze 3.13.1, probe5/probe5c): `<String>['x']` carries no
    /// element mismatch — the literal is `List<String>`, so only the
    /// outer `return_of_invalid_type` fires against the `List<int>`
    /// return; a real mismatch element (`42` under `<String>`) reports
    /// `list_element_type_not_assignable` at that element. The fixture
    /// stub-declares `List`/`int`/`String` — the bare unit carries no
    /// `dart:core` scope for them to resolve through.
    #[test]
    fn written_group_literal_still_checks_elements() {
        // No element mismatch — the written `<String>` matches `'x'`
        // exactly, so only the enclosing return-type face fires.
        let matched = report(
            "class List<E> {} class int {} class String {}\n\
             List<int> g() => <String>['x'];",
        );
        let names: Vec<&'static str> = matched.diagnostics.iter().map(|d| d.code.name).collect();
        assert!(
            names.contains(&"return_of_invalid_type"),
            "expected return_of_invalid_type, got {names:?}"
        );
        assert!(
            !names.contains(&"list_element_type_not_assignable"),
            "element face must not fire on a matching element: {names:?}"
        );

        // A real mismatch element reports `list_element_type_not_assignable`
        // at that element (the oracle puts it on `42`, probe5c).
        let mismatched = report(
            "class List<E> {} class int {} class String {}\n\
             List<int> g() => <String>['x', 42];",
        );
        let names: Vec<&'static str> = mismatched.diagnostics.iter().map(|d| d.code.name).collect();
        assert!(
            names.contains(&"return_of_invalid_type"),
            "expected return_of_invalid_type, got {names:?}"
        );
        assert!(
            names.contains(&"list_element_type_not_assignable"),
            "element face must fire on the mismatched `42`: {names:?}"
        );
    }

    /// A `dynamic` argument into an unsolved generic formal's
    /// type-variable slot is the call-site inference's own ground
    /// (`accept<E>(V<E>)` over a `V<dynamic>` argument binds `E`), so the
    /// written residue never reports.
    #[test]
    fn dynamic_argument_into_unsolved_generic_formal_stays_silent() {
        let report = report(
            "class V<T> {\n\
               int visit() => 1;\n\
             }\n\
             class Node {\n\
               int accept<E>(V<E> visitor) => visitor.visit();\n\
             }\n\
             void m(Node node, V<dynamic> visitor) {\n\
               node.accept(visitor);\n\
             }",
        );
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|d| d.code.name == "argument_type_not_assignable"),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| (&d.code.name, &d.message))
                .collect::<Vec<_>>()
        );
    }

    /// The ground-truth control: a `dynamic` slot against a concrete
    /// written argument still refuses (`List<dynamic>` into
    /// `List<int>` — the slot pair is real, no inference stands).
    #[test]
    fn dynamic_slot_against_concrete_argument_still_reports() {
        let report = report(
            "void takeList(List<int> xs) {}\n\
             void m(List<dynamic> y) {\n\
               takeList(y);\n\
             }",
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code.name == "argument_type_not_assignable"),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| (&d.code.name, &d.message))
                .collect::<Vec<_>>()
        );
    }
}

/// The closure-return face's pins (`return_of_invalid_type_from_closure`):
/// the literal's body return checks against the context return type the
/// model carries — the declaration initializer's written function type and
/// the enclosing executable's return position — and never leaks to a
/// farther context. The rig is the async-return module's single-file
/// pipeline; the fixture heads avoid core spellings (`String` walks as an
/// SDK-degenerate head there, the supertype ask stays open) so the
/// distinct-class rejection decides.
#[cfg(test)]
mod closure_return_tests {
    use super::async_return_tests::{code_names, report};

    /// `Str Function() f = () => DB();` reports at the literal's body: the
    /// context is the initializer's written function type, the message the
    /// closure template (diagnostic.g.dart:15314-15328).
    #[test]
    fn initializer_context_reports_the_closure_face() {
        let report = report("class DB {}\nclass Str {}\nStr Function() f = () => DB();");
        assert_eq!(code_names(&report), ["return_of_invalid_type_from_closure"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The returned type 'DB' isn't returnable from a 'Str' function, as required by the closure's context."
        );
    }

    /// The block-bodied literal's own `return` checks through the same
    /// initializer context.
    #[test]
    fn block_literal_statement_return_uses_the_same_context() {
        let report = report("class DB {}\nclass Str {}\nStr Function() f = () { return DB(); };");
        assert_eq!(code_names(&report), ["return_of_invalid_type_from_closure"]);
    }

    /// A fitting body stays silent: the subtype answer cleans the check.
    #[test]
    fn fitting_body_stays_silent() {
        let report = report(
            "class Str {}\nclass Sub extends Str {}\n\
             Str Function() f = () => Sub();",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// The return position, alias-spelled: the literal is the enclosing
    /// executable's `return` expression, whose declared function-shaped
    /// return is the context. The alias spelling is the live one — a
    /// direct `Str Function()` return annotation latches shapeless (the
    /// type-capture face opens no capture in an executable's return
    /// position, so the pool shape stays empty and the context answer is
    /// `dynamic`).
    #[test]
    fn enclosing_return_statement_position_reports() {
        let report = report(
            "class DB {}\nclass Str {}\n\
             typedef F = Str Function();\n\
             F h() {\n  return () => DB();\n}",
        );
        assert_eq!(code_names(&report), ["return_of_invalid_type_from_closure"]);
    }

    /// The direct function-typed return spelling under-reports: the
    /// written shape never reaches the pool (`FnShapeId::EMPTY`), the
    /// context answer degrades to `dynamic` and the verdict stays clean —
    /// the oracle reports here; the recovery belongs to the type-capture
    /// face (executable return positions open no capture), not this file.
    #[test]
    fn direct_function_typed_return_spelling_stays_silent() {
        let report = report(
            "class DB {}\nclass Str {}\n\
             Str Function() h() {\n  return () => DB();\n}",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// The argument literal's own context adjudicates it, not the
    /// enclosing return: `take(() => DB())` infers against the `F` formal
    /// (#2868's argument channel — the slot's folded schema IS the
    /// literal's `K`), so its `DB()` body reports the closure-return row;
    /// the final return-position literal's `Str()` body satisfies its own
    /// `F` context and stays silent. One row, anchored on the argument
    /// literal's body — the enclosing `F` return never adjudicates the
    /// wrong literal (the mark discipline keeps each context to its own
    /// capture).
    #[test]
    fn argument_position_literal_never_leaks_to_the_enclosing_return() {
        let report = report(
            "class DB {}\nclass Str {}\n\
             typedef F = Str Function();\n\
             F take(F f) => Str();\n\
             F h() {\n  take(() => DB());\n  return () => Str();\n}",
        );
        assert_eq!(code_names(&report), ["return_of_invalid_type_from_closure"]);
    }

    /// A non-function context is no context: a literal initialized into a
    /// plain named type stays silent (the ordinary matrix is the outer
    /// check's business, and its Unknown literal row suppresses).
    #[test]
    fn non_function_context_stays_silent() {
        let report = report("class DB {}\nclass Holder {}\nHolder h = () => DB();");
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }
}

/// The instantiated function-type alias's pins (#2317①): a parameterized
/// function-typed alias participates in subtyping as its aliased function
/// shape — formal contravariance across the instantiation (`Pred2<String?>`
/// into `Pred2<String>` fits), the return-mismatch control reports, and the
/// non-alias same-head pair keeps the invariant argument rule.
#[cfg(test)]
mod alias_function_subtyping_tests {
    use super::async_return_tests::{code_names, report};

    /// The triage repro (typeprobe 113), rig head: `Pred2<Cat?>` fits
    /// `Pred2<Cat>` — the alias unfolds, the formal slot is contravariant
    /// (`Cat <: Cat?`), the returns coincide. The `pick2` source spells
    /// the Cat?-accepting shape itself (`Cat Function(Cat?)`); the raw
    /// `pick` tear-off would be the INVALID direction (a Cat-only
    /// handler cannot stand where a Cat?-handler is required — oracle
    /// 3.13.7 reports it). The fixture avoids core head spellings (the
    /// rig rejects a redeclared core name as `undefined_class`).
    #[test]
    fn instantiated_alias_contravariant_formals_fit() {
        let report = report(
            "class Cat {}\n\
             typedef Pred2<T> = Cat Function(T);\n\
             Cat pick(Cat c) => c;\n\
             Cat pick2(Cat? c) => c as Cat;\n\
             Pred2<Cat> x = pick;\nPred2<Cat?> ps = pick2;\nPred2<Cat> q = ps;",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// The identity instantiation stays silent (the same-name fast path's
    /// old answer, now through the function rule).
    #[test]
    fn identical_instantiation_stays_silent() {
        let report = report(
            "class Cat {}\n\
             typedef Pred2<T> = Cat Function(T);\n\
             Cat pick(Cat c) => c;\n\
             Pred2<Cat> p = pick;\nPred2<Cat> q = p;",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
    }

    /// A return-type mismatch across the instantiation reports — the
    /// function rule's covariance leg decides negatively.
    #[test]
    fn return_mismatch_instantiation_reports() {
        let report = report(
            "class Cat {}\nclass Dog {}\n\
             typedef Pick<T> = T Function();\n\
             Pick<Cat> p = Cat();\nPick<Dog> q = p;",
        );
        assert_eq!(code_names(&report), ["invalid_assignment"]);
    }

    /// The regression control: a same-head non-alias instantiated pair
    /// keeps the invariant covariant-argument rule (a `Box<Cat>` never
    /// fits `Box<Dog>`).
    #[test]
    fn non_alias_same_head_pair_still_reports() {
        let report = report(
            "class Cat {}\nclass Dog {}\n\
             class Box<T> { T item; }\n\
             Box<Cat> bc = Box<Cat>();\nBox<Dog> bd = bc;",
        );
        assert_eq!(code_names(&report), ["invalid_assignment"]);
    }
}

// The #2870 pins (the integer-literal-imprecise-as-double face).
// Registered here, not in `diagnose.rs` (another lane's file this wave),
// via `#[path]`: the module stays inside the `diagnose` subtree so the
// test rig's `pub(super)` items remain visible. Moving the declaration
// into `diagnose.rs`'s sibling block is a mechanical follow-up for
// whoever owns that file next.
#[cfg(test)]
#[path = "ticket_2870_tests.rs"]
mod ticket_2870_tests;

// The #2946 pins (C1 — the alias-unfold + literal-leaf verdict faces).
// Same `#[path]` registration as the #2870 pins above: the module stays
// inside the `diagnose` subtree so the shared rig items remain visible.
#[cfg(test)]
#[path = "ticket_2946_tests.rs"]
mod ticket_2946_tests;

// The #2967 pins (the assignability verdict's residual families: the
// `Never`-target refusal, the Right-Nullable decomposition, the bare
// tear-off read, and the return window's send-result resolution).
#[cfg(test)]
#[path = "ticket_2967_tests.rs"]
mod ticket_2967_tests;

// The #3145 pins (the multi-row right-hand-side result window — every
// `=`/initializer/return face reads the fold's tail row — plus the
// callable-class acceptance and `relateTypeParameters`' unified-position
// compare). Same `#[path]` registration as the pins above.
#[cfg(test)]
#[path = "ticket_3145_tests.rs"]
mod ticket_3145_tests;

// The #3147 pins (the return family: bare `return;`'s
// `return_without_value`, the generator's `return_in_generator`, the
// generative constructor's `return_in_generative_constructor`, the
// creation send's instantiated-head read, the record source's closed
// supertype set, and the generic tear-off's context instantiation).
// Same `#[path]` registration as the pins above.
#[cfg(test)]
#[path = "ticket_3147_tests.rs"]
mod ticket_3147_tests;

// The #3173 pins (the `dead_code` switch-chain faces — the member-dead
// keyword-only row, the never-matching-constant body-only row, and the
// switch-expression whole-arm span — upstream `flowEnd`'s SwitchMember
// arm, dead_code_verifier.dart:237-241 / :275-279).
// Same `#[path]` registration as the pins above.
#[cfg(test)]
#[path = "ticket_3173_tests.rs"]
mod ticket_3173_tests;

// The #2487 pins: F5 — the assignment-target scan's bounded form
// (`assignment_target_index`'s `partition_point` prefix + bounded
// reverse scan, with `ASSIGNMENT_TARGET_VISITS` counting the
// instruction face); F6 — the is-check undefined head's id-keyed
// `undefined_ids` set through the mounted `ResolvedTypeOps`.
// Same `#[path]` registration as the pins above.
#[cfg(test)]
#[path = "ticket_2487_tests.rs"]
mod ticket_2487_tests;

// The #3202 pins: the args RefCell re-entrancy panic in `subst_ty`'s
// function leg — the guard held across the recursive return-face
// substitution (the release-binary crash of the extension-type
// primary-constructor sweep) — plus its verdict integrity on the
// hoisted discipline.
#[cfg(test)]
#[path = "ticket_3202_tests.rs"]
mod ticket_3202_tests;

// The #3209 pins (bounds_conformance — the isSubtypeOf
// instantiate-to-bounds residual, the super-bounds arm, and the
// generic-bound residual). Same `#[path]` registration as the pins
// above: the module stays inside the `diagnose` subtree so
// `super::test_rig` resolves.
#[cfg(test)]
#[path = "ticket_bounds_tests.rs"]
mod ticket_bounds_tests;

// The #3424 pins (the untyped collection literal's own upward type under a
// declared context its head cannot match — the initializer and assignment
// faces). Same `#[path]` registration as the pins above.
#[cfg(test)]
#[path = "ticket_3424_tests.rs"]
mod ticket_3424_tests;

// The #3472 pins (the switch covering face: the pre-patterns gate, the
// unresolved matched type, the private-enum template, the sealed split's
// instantiations, and the cast pattern's coverage). Same `#[path]`
// registration as the pins above.
#[cfg(test)]
#[path = "ticket_3472_tests.rs"]
mod ticket_3472_tests;
