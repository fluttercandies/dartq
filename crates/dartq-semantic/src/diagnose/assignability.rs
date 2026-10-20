use super::engine_core::flatten_ty;
use super::*;

impl<'a> Engine<'a> {
    pub(crate) fn typed_list_literal_ty(&mut self, left: Token) -> Option<Ty> {
        let bytes = self.source.as_bytes();
        let mut cursor = left.range.offset as usize;
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
        let (name_range, name) = self
            .type_uses
            .iter()
            .rev()
            .find(|(range, _)| range.end() <= left.range.offset)
            .copied()?;
        if !matches!(self.type_resolution(name), (TypeResolution::Type, _)) {
            return None;
        }
        // The written element's own nullability (`<String?>[…]`): the
        // type-use row names `String` but the `?` rides the suffix, so the
        // element type is `String?` — without it every element reads as a
        // `String? → String` assignability break (#2161).
        let nullable = self.written_nullable_suffix(name_range.end());
        let elem = Ty::Named {
            name,
            nullable,
            args: crate::model::ArgListId::EMPTY,
        };
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
        let mut cursor = left.range.offset as usize;
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
        let group_end = left.range.offset as usize;
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
                self.type_uses
                    .iter()
                    .rev()
                    .find(|(range, _)| {
                        range.offset as usize >= *start && range.end() as usize <= *end
                    })
                    .copied()
            })
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .map(|(name_range, name)| {
                if !matches!(self.type_resolution(name), (TypeResolution::Type, _)) {
                    return None;
                }
                Some(Ty::Named {
                    name,
                    nullable: self.written_nullable_suffix(name_range.end()),
                    args: crate::model::ArgListId::EMPTY,
                })
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
    pub(crate) fn check_list_literal_elements(&mut self, left: Token, right: Token) {
        let bytes = self.source.as_bytes();
        // A typed literal's `<` group sits immediately before the `[`.
        let mut cursor = left.range.offset as usize;
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        // The same arrow face [`Self::typed_list_literal_ty`] rejects: a
        // `=> [` opens an untyped switch-arm/arrow-body literal, and the
        // byte before its bracket is the arrow's `>`, never a group close.
        if cursor < 2 || bytes[cursor - 1] != b'>' || bytes[cursor - 2] == b'=' {
            return;
        }
        // The nearest type reference ending before the bracket is the
        // element type (every bare type reference ships `(span, name)`).
        let Some((name_range, name)) = self
            .type_uses
            .iter()
            .rev()
            .find(|(range, _)| range.end() <= left.range.offset)
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
        let declared = Ty::Named {
            name,
            nullable: self.written_nullable_suffix(name_range.end()),
            args: crate::model::ArgListId::EMPTY,
        };
        // Top-level element segments: comma splits at bracket depth 0,
        // string-literal contents skipped.
        let mut segments: Vec<(usize, usize)> = Vec::new();
        let mut seg_start = left.range.end() as usize;
        let end = right.range.offset as usize;
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
            let Some(row) = self.rows.iter().rev().find(|row| {
                row.ty != Ty::UNKNOWN
                    && row.range.offset >= start as u32
                    && row.range.end() <= stop as u32
            }) else {
                continue;
            };
            if self.assignable(row.ty, declared, false) == Some(false) {
                let (a0, a1) = (self.ty_display(row.ty), self.ty_display(declared));
                let range = row.range;
                self.emit(SemanticCode::ListElementTypeNotAssignable, range, &a0, &a1);
            }
        }
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
        let ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
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
        let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
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

    /// The explicit extension override's argument check
    /// (`extension_member_resolver.dart:290-313`): the single argument is the
    /// receiver, and `!isAssignableTo(receiverType, extendedType)` reports
    /// `extension_override_argument_not_assignable` at the whole receiver
    /// expression (`.at(receiverExpression)` — a `c.bad` operand anchors at
    /// the property-access span). The `on` type is name-level (`on int` →
    /// `int`); a qualified or generic `on` face keeps its bare head. A
    /// missing/mistyped `on` suppresses (the inapplicable extension).
    pub(crate) fn check_extension_override_argument(
        &mut self,
        name: StringId,
        args: &[(Option<StringId>, Ty, bool, SourceRange)],
    ) {
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
        let [(_, arg_ty, literal, arg_range)] = args else {
            return;
        };
        // The void-receiver face is upstream's first arm
        // (`use_of_void_result`, `extension_member_resolver.dart:290`) — the
        // `else if` keeps the assignability row off a void argument.
        if *arg_ty == Ty::Void {
            return;
        }
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
        let range = self.property_access_span(*arg_range).unwrap_or(*arg_range);
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
    /// answers yes).
    pub(crate) fn is_never_ty(&self, ty: Ty) -> bool {
        matches!(
            ty,
            Ty::Named {
                name,
                nullable: false,
                ..
            } if name == self.intern("Never")
        )
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
    fn accessor_read_ty(&self, row: &Row) -> Ty {
        let answer = |entry: &MemberEntry| match entry.kind {
            MemberKind::Field => self.fold_alias_ty(entry.var_ty),
            _ => self.fold_alias_ty(entry.return_ty),
        };
        match &row.shape {
            RowShape::Bare(BareInfo::Plain { .. }) if !row.qualified => {
                match self.resolve_bare(row.name) {
                    (_, RefTarget::Member { container }) => {
                        match self.walk_members(container, row.name) {
                            Walk::Found(entry)
                                if matches!(entry.kind, MemberKind::Getter | MemberKind::Field) =>
                            {
                                answer(&entry)
                            }
                            _ => Ty::UNKNOWN,
                        }
                    }
                    _ => Ty::UNKNOWN,
                }
            }
            RowShape::Member { recv } => {
                let container = match recv {
                    Receiver::This { container, .. }
                    | Receiver::Super { container, .. }
                    | Receiver::Container {
                        name: container, ..
                    }
                    | Receiver::Typed {
                        type_name: container,
                        ..
                    } => Some(*container),
                    _ => None,
                };
                match container {
                    Some(container) => match self.walk_members(container, row.name) {
                        Walk::Found(entry)
                            if matches!(entry.kind, MemberKind::Getter | MemberKind::Field) =>
                        {
                            answer(&entry)
                        }
                        _ => Ty::UNKNOWN,
                    },
                    None => Ty::UNKNOWN,
                }
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
    /// against the flow read-type face (#1465): when the receiver read's
    /// flow type names a different container than the push-time static
    /// type, a promotion stands, and the member walk follows the promoted
    /// spelling — the official resolver reads the flow-aware static type,
    /// so `case Some(): option.value` resolves on `Some`, not the declared
    /// `Option`. Every other shape keeps the push-time classification: a
    /// non-`Typed` receiver has no static container to differ from, a
    /// non-named flow face (`dynamic`/`Null`/unknown, or a join the
    /// staged drop rules refused) has no member face to walk, and an equal
    /// type is the declared fallback — today's behavior byte for byte.
    pub(crate) fn promoted_receiver(&self, member_offset: u32, recv: Receiver) -> Receiver {
        let Receiver::Typed { ty: static_ty, .. } = recv else {
            return recv;
        };
        let Some(read_span) = self
            .promoted_recv_reads
            .binary_search_by(|(member, _)| member.cmp(&member_offset))
            .ok()
            .map(|index| self.promoted_recv_reads[index].1)
        else {
            return recv;
        };
        let Some(flow_ty) = self.read_type_at(read_span) else {
            return recv;
        };
        let Ty::Named { name, .. } = flow_ty else {
            return recv;
        };
        if flow_ty == static_ty {
            return recv;
        }
        // A postfix-`!` receiver already carries its non-null static type:
        // the flow read at the variable's span describes the pre-assert
        // state, not the asserted expression — `x!.m` resolves on `C`
        // even when `x`'s flow type at the read is `C?` (#2161). That
        // pre-state's signature is the static face's own head widened back
        // to nullable — promotion never widens nullability, so only the
        // `!` rewrite produces it. Every other differing flow face is a
        // promotion and re-decides, including one of a non-nullable
        // declared type (`other is Money11 && other.cents` on an
        // `Object other`, #2167).
        if matches!(
            (&flow_ty, &static_ty),
            (
                Ty::Named {
                    name: flow_head,
                    nullable: true,
                    ..
                },
                Ty::Named {
                    name: static_head,
                    nullable: false,
                    ..
                },
            ) if flow_head == static_head
        ) {
            return recv;
        }
        Receiver::Typed {
            type_name: name,
            ty: flow_ty,
        }
    }

    /// The declared return type of an invoked callee row (a completed
    /// send's result type): a bare callee resolves through the top-level
    /// namespace — a same-named local shadows it (suppress; its declared
    /// type already rode the row when captured) — and a member callee
    /// walks the container its merged receiver names. Unresolvable shapes
    /// suppress.
    pub(crate) fn callee_return_ty(&self, row: &Row) -> Ty {
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
                return Ty::UNKNOWN;
            }
            // An unqualified member invocation (`die(x)` inside the class —
            // the implicit-this call): the bare scope resolves it to the
            // enclosing container's member, whose declared return drives the
            // send's result (`Never die(...)` marks the continuation dead —
            // the real-corpus `usageException` idiom, #2161).
            if let (_, RefTarget::Member { container }) = self.resolve_bare(row.name)
                && let Walk::Found(entry) = self.walk_members(container, row.name)
                && matches!(entry.kind, MemberKind::Method | MemberKind::Getter)
            {
                return entry.return_ty;
            }
            return self
                .top_level_decl(row.name)
                .map_or(Ty::UNKNOWN, |decl| decl.return_ty);
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
            (receiver.range.end() <= row.range.offset).then_some(receiver.name)
        });
        let Some(container) = container else {
            return Ty::UNKNOWN;
        };
        match self.walk_members(container, row.name) {
            Walk::Found(entry) if matches!(entry.kind, MemberKind::Method | MemberKind::Getter) => {
                entry.return_ty
            }
            // The unindexed dart:core container's static face
            // ([`CORE_STATIC_RETURNS`]): the member walk chain-breaks on
            // every SDK unit, so the declared return rides the table
            // instead of the walk (`RangeError.checkValidRange(...)`,
            // #2172). An indexed container keeps the miss face — a
            // user-declared same-spelled container never borrows the SDK
            // table.
            _ => {
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

    /// Stages the never-receiver junction (`receiverOfTypeNever`,
    /// messages.yaml :29566). The staged check re-decides at the teardown
    /// join, where a promoted-read fact on the receiver's exact span
    /// replaces the type — the official read's flow-aware staticType. Only
    /// a parse-time `Never` receiver stages: a non-`Never` type could only
    /// fire through a promotion substitution, and #477's drop rule keeps
    /// promoted-to-`Never` facts out of the staging, so staging those rows
    /// would be dead weight on every junction.
    pub(crate) fn stage_receiver_of_type_never(&mut self, at: SourceRange, receiver: Ty) {
        if !self.is_never_ty(receiver) {
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
        for row in self.rows.iter_mut().rev() {
            if row.range.end() > limit {
                continue;
            }
            if matches!(row.shape, RowShape::Value) {
                continue;
            }
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
            return;
        }
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
    pub(crate) fn assignment_target_ty(&mut self, row: &Row) -> Ty {
        match &row.shape {
            RowShape::Bare(BareInfo::Plain { ty }) => *ty,
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
                Receiver::Typed { type_name, .. } => self.member_write_ty(*type_name, row.name),
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

    /// The assignment check (`invalid_assignment`,
    /// `assignment_expression_resolver.dart:795`): a single-row right-hand
    /// side whose type the M1 facts know (`Null` or a captured declared
    /// type) against the target's declared type. Plain `=` only — compound
    /// operators carry their own read semantics (under-report).
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
        if op_text != "=" {
            return;
        }
        let limit = operator.range.offset;
        // The target: the closest row wholly before the operator, skipping
        // folded value rows (the same scan `mark_assignment` runs).
        let Some(target) = self
            .rows
            .iter()
            .enumerate()
            .rev()
            .find(|(_, row)| row.range.end() <= limit && !matches!(row.shape, RowShape::Value))
            .map(|(index, _)| index)
        else {
            return;
        };
        // `c[i] = e` writes through the `[]=` operator: the target's own
        // declared type is not the assignment's destination type.
        if matches!(self.rows[target].intent, Intent::Operator(..)) {
            return;
        }
        // The right-hand side: exactly one row strictly after the operator
        // (rows are pushed in source order, so the first post-operator row
        // and the absence of a second decide without collecting).
        // The single-RHS-row window: collect the candidate before any
        // mutable member-walk call — the iterator borrows `self.rows`, and
        // `assignment_target_ty` borrows `self` mutably.
        let rhs_row = {
            let mut rhs = self.rows[target + 1..]
                .iter()
                .filter(|row| row.range.offset > limit);
            let Some(row) = rhs.next() else {
                return;
            };
            if rhs.next().is_some() {
                return;
            }
            row.clone()
        };
        let (from, range) = (rhs_row.ty, rhs_row.range);
        let literal = Self::primitive_literal(&rhs_row, &self.members.primitive_ids);
        let target_row = self.rows[target].clone();
        let to = self.assignment_target_ty(&target_row);
        if self.assignable(from, to, literal) == Some(false) {
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
        }
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
        if self.rows.len() != mark + 1 {
            return;
        }
        let row = &self.rows[mark];
        let literal = Self::primitive_literal(row, &self.members.primitive_ids);
        if self.assignable(row.ty, to, literal) == Some(false) {
            self.stage(StagedKind::Assignment, row.range, row.ty, to, literal);
        }
        self.init_mark = None;
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
            FrameKind::Executable { ret, is_async, .. } => Some(match ret {
                Some(ret) => Ok((ret.ty, ret.host, ret.name, ret.ctor, *is_async)),
                None => Err(*is_async),
            }),
            _ => None,
        });
        let Some(found) = found else {
            return;
        };
        let (to, host, name, ctor, is_async) = match found {
            Ok(latched) => latched,
            Err(is_async) => {
                self.check_closure_return(from, range, from_literal, is_async);
                return;
            }
        };
        // An unannotated collection literal's row carries no type of its
        // own: the host's declared return is the context that resolves it
        // (`typed_literal_resolver.dart` `_fromContextType` — the context
        // decides list/set/map, an empty `{}` adopts a Map/Set context and
        // defaults to a map without one).
        let from = match self.untyped_literals.get(&range.offset) {
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
    ) {
        let Some(to) = self.closure_context_return() else {
            return;
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

    /// The context return type a function literal's body checks against.
    /// The two channels this name-level model carries: the declaration
    /// initializer's written function type (the literal IS the
    /// initializer), and the enclosing executable's return position (the
    /// literal IS the `return` expression, whose declared type is
    /// function-shaped — the literal's context is that function type, its
    /// body checks against the shape's return). Every other context (an
    /// argument formal, a generic method's solved `T`, the enclosing
    /// `=>` body) answers `None` — under-report.
    fn closure_context_return(&self) -> Option<Ty> {
        let frame = self.frames.last()?;
        let FrameKind::Executable { body_brace, .. } = &frame.kind else {
            return None;
        };
        let open_mark = frame.row_mark;
        // The declaration initializer: the literal opened exactly where the
        // initializer began, so its context is the group's written function
        // type's return (`String Function() f = () => 1;`). A deeper
        // literal inside the initializer (`f = g(() => 1)`) opened after
        // the callee's row — its mark no longer equals the initializer's.
        if self.init_mark == Some(open_mark)
            && let Some(Ty::Function { shape, .. }) = self.pending_decl_ty
        {
            let ret = {
                let pool = self.boundary.pool();
                pool.function_shape(shape).return_ty
            };
            return Some(ret);
        }
        // The enclosing executable's return position: the literal opened
        // exactly where the in-flight `return` began, and the enclosing
        // declared return is function-shaped — its return is the literal's
        // context. An arrow-bodied literal only: a block body's own first
        // `return` re-arms the mark inside the literal, where the equality
        // can no longer separate the outer return position from the
        // literal's own body returns (the false direction — never
        // reported). The `=>`-body position of the enclosing executable
        // stays under-report: the literal's own body event consumes the
        // body mark before this check reads it.
        if body_brace.is_some() || self.return_mark != Some(open_mark) {
            return None;
        }
        let enclosing = self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Executable { ret: Some(ret), .. } => Some(ret.ty),
            _ => None,
        })?;
        self.function_shape_return(enclosing)
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
        if is_async {
            return self.async_return_verdict(from, to, from_literal);
        }
        match to {
            Ty::Void => match from {
                // `Never <: void` holds (Right Bottom — `return throw e;`).
                Ty::Void | Ty::Dynamic | Ty::Null | Ty::Never => Some(true),
                Ty::Unknown => None,
                Ty::Named { .. } | Ty::Record { .. } | Ty::Function { .. } => Some(false),
            },
            Ty::Unknown => None,
            _ => match from {
                Ty::Void => Some(!matches!(to, Ty::Void | Ty::Dynamic)),
                // A `dynamic` return expression is a subtype only of the
                // top-type hosts; the strict-casts mode rejects the rest
                // (the same overlay [`Engine::assignable`] applies — the
                // oracle's return verifier consults the identical
                // `isAssignableTo` gate).
                Ty::Dynamic => Some(matches!(to, Ty::Dynamic | Ty::Void) || !self.strict.casts),
                _ => self.assignable(from, to, from_literal),
            },
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
        if verdict == Some(false) && self.name_subtype(flatten_s, t_v) {
            return Some(true);
        }
        verdict
    }

    /// A single-row expression's type since `mark`, for the body/return
    /// checks, with the row's primitive-literal flag (the conversion's
    /// source-side fact); `None` when the expression is compound. A folded
    /// creation reads the created container (`C()`'s row is the fold's
    /// sentinel, its container names the static type — the factory return
    /// face `factory D.new() => C();` asks the nominal question).
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
        let row = self
            .rows
            .iter()
            .skip(mark)
            .filter(|row| !matches!(row.shape, RowShape::TypeUse(..)))
            .filter(|row| !self.interpolation_child_rows.contains(&row.range.offset))
            .collect::<Vec<_>>();
        let &[row] = row.as_slice() else {
            return None;
        };
        let ty = if row.ty == Ty::Never {
            // The throw retype (`handle_throw_expression`): the
            // expression's static type is `Never` regardless of the
            // operand's shape — the oracle's bottom type decides every
            // downstream assignability ask (`Never f() => throw Object();`
            // stays silent), so the Created-name read below must not
            // resurrect the operand's type.
            Ty::Never
        } else {
            match &row.shape {
                RowShape::Receiver(Receiver::Created { name, kind })
                    if *kind != ContainerKind::Extension =>
                {
                    Ty::named(*name)
                }
                _ => row.ty,
            }
        };
        Some((
            ty,
            Self::primitive_literal(row, &self.members.primitive_ids),
            row.range,
        ))
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
        // `use_of_void_result`'s row.
        if ty == Ty::Void {
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
        match self.assignable(ty, target, literal) {
            Some(false) => {
                let sequence = if await_ { "Stream" } else { "Iterable" };
                let a0 = self.ty_display(ty);
                self.emit(SemanticCode::ForInOfInvalidType, range, &a0, sequence);
            }
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
                    let sequence = if await_ { "Stream" } else { "Iterable" };
                    let a0 = self.ty_display(ty);
                    self.emit(SemanticCode::ForInOfInvalidType, range, &a0, sequence);
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
    pub(crate) fn declared_subtype_negative(&self, from: &Ty, to: &Ty) -> Option<bool> {
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
        let hierarchy = FoldHierarchy {
            graph: self.graph,
            namespaces: self.namespaces,
            index: self.index,
            file: self.file,
            lib: self.lib,
            memo: RefCell::default(),
            in_progress: RefCell::default(),
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

    /// Closes the case-header pattern window and drains its root: the
    /// topmost restriction is the header pattern's coverage (pushed between
    /// the window's open and here), everything below is debris from earlier
    /// bodies. `None` — a legacy or recovered header with no pattern —
    /// falls back to the unknown space.
    pub(crate) fn close_case_sink(&mut self) -> Option<Restriction> {
        self.switch.close_case_sink()
    }

    /// Substitutes a captured root's type-free coverage with the
    /// scrutinee's context type.
    pub(crate) fn substitute_root(&self, root: Option<Restriction>) -> Restriction {
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
        let scrutinee = self.rows.last().map_or(Ty::Unknown, |row| row.ty);
        self.switch.open_switch(scrutinee, is_expression, kw);
    }

    /// Computes the exhaustiveness verdict and emits the switch
    /// diagnostics (per-case reachability, then the covering gap).
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    pub(crate) fn report_switch(&mut self) {
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
        if ctx.scrutinee == Ty::Unknown {
            return;
        }
        let world = self.name_world();
        let value = NameTy::from_semantic(ctx.scrutinee);
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
                    self.ty_display(ctx.scrutinee),
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
        // The flow-dead member rows (statement switches only — a switch
        // expression's covered arms carry `unreachable_switch_case` at the
        // arrow alone). The official chain is flow-side: a member whose
        // unguarded alternative always matches (its space covers the whole
        // scrutinee — `case _`, a variable pattern over the same type)
        // drives the flow `unmatched` state unreachable, so every later
        // member head is entered dead (the keyword row, the bare `case`
        // token) and its member body dies with it (dead_code_verifier.dart's
        // SwitchMember faces). A member whose own space is disjoint from
        // the scrutinee never matches — only its body dies. The
        // default-driven chain is the flow layer's (#868), so this chain
        // stops at the first `default` member. Exhaustiveness-only coverage
        // (a duplicate constant over a fully covered scrutinee) is NOT
        // flow-dead — the oracle pins that to `unreachable_switch_case`
        // alone.
        if !ctx.is_expression && !cases.is_empty() {
            let total = |index: usize| {
                matches!(
                    compute(&world, &value, std::slice::from_ref(&cases[index])).exhaustiveness,
                    Exhaustiveness::Exhaustive
                )
            };
            // The never-match face: the case's witness set equals the bare
            // scrutinee's — its space removes nothing, so no scrutinee
            // value can enter the body. Only constant roots qualify (the
            // equality-check face — see
            // [`restriction_is_constant`]): `case null` over a
            // non-nullable type kills its body, a disjoint type test does
            // not. A guarded case never qualifies: `compute` keeps its row
            // out of the covering set (the guard may fail), so witness
            // equality there says nothing about the pattern's space — its
            // body stays live.
            let whole = compute(&world, &value, &[]);
            let never_matches = |index: usize| {
                let without = compute(&world, &value, std::slice::from_ref(&cases[index]));
                match (&whole.exhaustiveness, &without.exhaustiveness) {
                    (
                        Exhaustiveness::NonExhaustive(whole_witnesses),
                        Exhaustiveness::NonExhaustive(case_witnesses),
                    ) => {
                        whole_witnesses.len() == case_witnesses.len()
                            && whole_witnesses
                                .iter()
                                .zip(case_witnesses.iter())
                                .all(|(w, c)| w.as_witness(&world) == c.as_witness(&world))
                    }
                    _ => false,
                }
            };
            let chain_start = (0..ctx.cases.len())
                .find(|&index| !ctx.cases[index].guarded && total(index))
                .map_or(ctx.cases.len(), |index| index + 1);
            let chain_end = ctx.default_pos.unwrap_or(ctx.cases.len());
            for (index, case) in ctx.cases.iter().enumerate() {
                let in_chain = index >= chain_start && index < chain_end;
                let never = !in_chain
                    && !case.guarded
                    && restriction_is_constant(&ctx.cases[index].space)
                    && never_matches(index);
                if !in_chain && !never {
                    continue;
                }
                if in_chain {
                    self.emit(SemanticCode::DeadCode, case.anchor, "", "");
                }
                if let Some(body) = case.body {
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
            NameTy::Named(name, _) => world.name_resolvable(name),
            NameTy::Unknown(u64::MAX) => false,
            _ => true,
        };
        // The private-enum variant (constant_verifier.dart:1110-1121): an
        // enum declared in another library whose name is private, or whose
        // every witness names a private constant (`._`), reports the
        // `_private` code family instead — unmodeled here, so the face
        // suppresses rather than emit the public row.
        let private_foreign_enum = match value {
            NameTy::Named(name, _)
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
        let must_be_exhaustive = !private_foreign_enum
            && ((ctx.is_expression && expression_face_decides)
                || is_always_exhaustive(&world, value));
        if let Exhaustiveness::NonExhaustive(_) = report.exhaustiveness {
            if must_be_exhaustive && ctx.default_kw.is_none() {
                let code = if ctx.is_expression {
                    SemanticCode::NonExhaustiveSwitchExpression
                } else {
                    SemanticCode::NonExhaustiveSwitchStatement
                };
                // The oracle row carries a correctionMessage whose `{2}` is
                // the first witness's correction text (diagnostic.g.dart
                // :13018/:13079, constant_verifier.dart:1104-1120); the
                // diagnostic carries it in the correction slot.
                self.report.diagnostics.push(Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    code.message3(&ty_text, &unmatched, &suggested),
                    self.file,
                    ctx.switch_kw,
                    code.correction_message(&suggested),
                ));
            }
        } else if must_be_exhaustive && let Some(default_at) = ctx.default_kw {
            // The cases already cover the scrutinee, so the `default`
            // clause can never match (`nonExhaustiveness == null &&
            // defaultNode != null && mustBeExhaustive`,
            // constant_verifier.dart:1141-1148).
            self.emit(SemanticCode::UnreachableSwitchDefault, default_at, "", "");
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

    pub(crate) fn ty_display(&self, ty: Ty) -> String {
        self.render_ty(ty)
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
        self.assignable(sub, sup, false)
    }

    pub(crate) fn fn_sig_subtype(&self, from: &FnVarSig, to: &FnVarSig) -> Option<bool> {
        if to.return_ty != Ty::Void {
            match self.slot_subtype(from.return_ty, to.return_ty) {
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
            match self.slot_subtype(*to_ty, from.positional[index]) {
                Some(false) => return Some(false),
                Some(true) => {}
                None => return None,
            }
        }
        for (name, from_ty, from_required) in &from.named {
            let Some((_, to_ty, to_required)) = to.named.iter().find(|(n, _, _)| n == name) else {
                return Some(false);
            };
            if *from_required && !to_required {
                return Some(false);
            }
            match self.slot_subtype(*to_ty, *from_ty) {
                Some(false) => return Some(false),
                Some(true) => {}
                None => return None,
            }
        }
        Some(true)
    }

    /// The written function-type display for the message slots: the
    /// return type, `Function(`, the positional list with the optional
    /// tail bracketed, the named section braced with its `required`
    /// keywords (`void Function({required int x})`).
    pub(crate) fn fn_sig_display(&self, sig: &FnVarSig) -> String {
        let mut text = self.ty_display(sig.return_ty);
        text.push_str(" Function(");
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
    pub(crate) fn assignable(&self, from: Ty, to: Ty, from_literal: bool) -> Option<bool> {
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
        // The function-shape face: when both sides carry a written function
        // shape — directly, or through a function-typed alias, which is
        // transparent at the subtype question (`subtype.dart` unfolds the
        // alias before `_isFunctionSubtypeOf`) — the function rule decides
        // (formal contravariance, return covariance). The invariant
        // same-head argument rule would misread a `bool Function(String?)`
        // alias instantiation against `bool Function(String)`, and the
        // interface walk cannot relate two alias heads at all. Non-function
        // pairs (and pairs where one side alone unfolds) keep the ordinary
        // paths below, byte-identical.
        if let (Some(from_fn), Some(to_fn)) =
            (self.function_shape_sig(from), self.function_shape_sig(to))
        {
            return self.fn_sig_subtype(&from_fn, &to_fn);
        }
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
            let pool = self.boundary.pool();
            let (from, to) = (pool.record_shape(from_shape), pool.record_shape(to_shape));
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
            return self.covariant_args(from_args, to_args);
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
        let from_primitive = self.members.primitive_ids.contains(&from_name);
        let to_primitive = self.members.primitive_ids.contains(&to_name);
        let collections = &self.collection_heads;
        if (collections.contains(&from_name) && to_primitive)
            || (from_primitive && collections.contains(&to_name))
        {
            return Some(false);
        }
        if !from_primitive || !to_primitive {
            return None;
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
    /// mismatch is the `wrong_number` family's written residue; the
    /// assignability face stays silent on it.
    pub(crate) fn covariant_args(&self, from: ArgListId, to: ArgListId) -> Option<bool> {
        // The narrow borrow ([`Engine::function_shape_sig`]'s pattern): the
        // slots copy out and the pool guard drops before the element
        // verdicts — each re-enters [`Engine::assignable`] and the chain
        // below it interns into the same pool (`intern_list`'s
        // `borrow_mut`), which a live `Ref` here would panic against.
        let (from, to) = {
            let pool = self.boundary.pool();
            (pool.get(from).to_vec(), pool.get(to).to_vec())
        };
        if from.len() != to.len() {
            return None;
        }
        let mut unknown = false;
        for (&f, &t) in from.iter().zip(to.iter()) {
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
            // resolves to no container.
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
                && self.unbound_formal_head(name)
            {
                continue;
            }
            // The covariant-interface element rule is the subtype question
            // (`Ctrl<dynamic>` does not fit `Ctrl<int>`: `dynamic <: int`
            // is false — the expression-position downcast leniency never
            // enters a type's argument shape).
            match self.slot_subtype(f, t) {
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
    fn function_shape_return(&self, ty: Ty) -> Option<Ty> {
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
                let (return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                Some(Box::new(FnVarSig {
                    params: Box::default(),
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
    /// names). `None` on an arity residue (the `wrong_number` family's
    /// business), when the written capture failed, or when the head is no
    /// function-RHS alias.
    fn instantiated_alias_sig(&self, name: StringId, args: ArgListId) -> Option<Box<FnVarSig>> {
        let params: Box<[StringId]> = match self.bounds.alias_fn_shape(name) {
            Some((params, ..)) => params.iter().map(|(param, ..)| *param).collect(),
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
            params: Box::default(),
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
                let (type_params, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
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
                self.boundary.args.borrow_mut().function(
                    type_params,
                    self.subst_ty(return_ty, map),
                    &positional,
                    &named,
                    nullable,
                )
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
    /// missing name is `undefined_annotation` anchored over the `@` (3.13.1
    /// caliber — the diagnostic was removed upstream in 3.14, `m1_codes.txt`
    /// freezes the measured form). A [`CORE_TOP_LEVEL_ANNOTATIONS`] name is
    /// defined by the implicit dart:core import, never missing. Defined
    /// names stay silent: the class/constructor second-part checks sit
    /// outside the M1 model (under-report). The metadata path skips the
    /// broken-import shown-name guard — the oracle's scope lookup reports
    /// the missing annotation regardless of `show` clauses
    /// (element.dart:8217-8250 suppresses the identifier face only;
    /// probe-verified both ways).
    pub(crate) fn check_annotation(&mut self, name: StringId, range: SourceRange) {
        let missing = self.annotation_name_missing(name)
            && !CORE_TOP_LEVEL_ANNOTATIONS.contains(&self.strings.resolve(name));
        let at = self.annotation_at.take();
        if missing {
            let span = SourceRange {
                offset: at.map_or(range.offset, |at| at.offset),
                length: range.end() - at.map_or(range.offset, |at| at.offset),
            };
            let name_text = self.strings.resolve(name).to_string();
            self.emit(SemanticCode::UndefinedAnnotation, span, &name_text, "");
        }
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

    /// A bare `return;` in an async body reads `flatten_S = void` (branch
    /// b): `Future<int>` demands a value (`T_v = int` fits no
    /// void/dynamic host) and reports; the `Future<void>` host keeps it
    /// silent (`T_v = void`, branch a's ok-set).
    #[test]
    fn async_bare_return_follows_the_flattened_hosts() {
        let demands_a_value =
            report("class Future<T> {}\nclass int {}\nFuture<int> f() async { return; }");
        assert_eq!(code_names(&demands_a_value), ["return_of_invalid_type"]);
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
        let names: Vec<&'static str> = matched
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect();
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
        let names: Vec<&'static str> = mismatched
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect();
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

    /// The leak control: a literal nested deeper in the body (an argument,
    /// not the return expression) checks nothing — the oracle's context is
    /// the argument formal's signature, which this layer does not carry,
    /// and the enclosing declared return must never adjudicate it. The
    /// final return-position literal stays clean. With the alias spelling
    /// the enclosing context IS decidable, so a leaked mark equality would
    /// misjudge the argument literal — the empty report proves the
    /// channel's mark discipline.
    #[test]
    fn argument_position_literal_never_leaks_to_the_enclosing_return() {
        let report = report(
            "class DB {}\nclass Str {}\n\
             typedef F = Str Function();\n\
             F take(F f) => Str();\n\
             F h() {\n  take(() => DB());\n  return () => Str();\n}",
        );
        assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
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

    /// The triage §4 repro (typeprobe 113), rig head: `Pred2<Cat?>` fits
    /// `Pred2<Cat>` — the alias unfolds, the formal slot is contravariant
    /// (`Cat <: Cat?`), the returns coincide. The fixture avoids core head
    /// spellings (the rig rejects a redeclared core name as
    /// `undefined_class`).
    #[test]
    fn instantiated_alias_contravariant_formals_fit() {
        let report = report(
            "class Cat {}\n\
             typedef Pred2<T> = Cat Function(T);\n\
             Cat pick(Cat c) => c;\n\
             Pred2<Cat?> p = pick;\nPred2<Cat> q = p;",
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
