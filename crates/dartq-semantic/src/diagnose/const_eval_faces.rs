use super::*;
use crate::const_eval::CreatedState;

/// The leading-dot shorthand callee's face, decided against the context
/// type's own member set (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/dart/resolver/method_invocation_resolver.dart`
/// `_resolveReceiverTypeLiteralForDotShorthand`: the member lookup first,
/// the constructor rewrite only on a miss).
enum ShorthandCallee {
    /// A static METHOD of the context type — the shorthand is a
    /// `DotShorthandInvocation`, so a call through it is the oracle's
    /// `const_eval_method_invocation` fault.
    StaticMethod,
    /// A static field/getter/setter of the context type. Upstream
    /// rewrites the call to a `FunctionExpressionInvocation`
    /// (`method_invocation_resolver.dart` :1287-1296), which
    /// `ConstantVisitor` has no arm for — the enclosing site's default
    /// row fires instead, never the method fault.
    PropertyAccessor,
    /// A constructor of the context type (the unnamed face included) —
    /// the shorthand is a `DotShorthandConstructorInvocation`, a valid
    /// constant creation whose initializer walk still runs. Carries the
    /// context container the walk keys on.
    Ctor(StringId),
    /// The context type is undecidable here (no written annotation, an
    /// unresolved head, a broken member climb) or answers neither arm —
    /// the under-report posture: the caller keeps the creation face, and
    /// the shorthand's own `dot_shorthand_undefined_member` row owns the
    /// miss.
    Defer,
}

/// One const-variable reference's instance key (the equality face of a
/// const set/map element spelled `name` / `p.name` whose instance value
/// this unit cannot fold): the declaring file, the creation's container,
/// the variable's declared type, and the surviving argument texts. Equal
/// keys ⇒ the oracle's instances are equal (`DartObjectImpl ==` over the
/// `GenericState` field map, value.dart:1528 — two instances of one
/// container whose surviving written fields hold identical same-file
/// spellings, which denote identical values). The declaring file rides
/// the key so argument texts — and [`Ty`] argument slots, which index the
/// declaring file's pool — never compare across files.
#[derive(PartialEq)]
struct InstanceRefKey {
    file: FileId,
    container: StringId,
    var_ty: Ty,
    fields: Box<[(StringId, StringId)]>,
}

/// The const-variable reference shape of a set element: a bare `name`, or
/// `p.name` whose receiver is a plain identifier (an import prefix the
/// key walk resolves, or anything else it defers on).
fn const_var_ref(part: &crate::const_eval::Node) -> Option<(Option<StringId>, StringId)> {
    match &part.expr {
        Expr::Ident(name) => Some((None, *name)),
        Expr::Member { recv, name } => match &recv.expr {
            Expr::Ident(prefix) => Some((Some(*prefix), *name)),
            _ => None,
        },
        _ => None,
    }
}

/// Consumes one balanced same-kind group starting at `bytes[i]` — the
/// byte at `i` must equal `open` (`<` for type arguments, `(` for an
/// argument list); strings are skipped so their brackets never skew the
/// depth. Returns the index past the matching close, or `i` unchanged
/// when the byte is not `open` or the group never closes.
fn skip_balanced(bytes: &[u8], i: usize, open: u8) -> usize {
    if bytes.get(i) != Some(&open) {
        return i;
    }
    let start = i;
    let close = if open == b'<' { b'>' } else { b')' };
    let mut depth = 0usize;
    let mut i = i;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b if b == quote => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b if b == open => {
                depth += 1;
                i += 1;
            }
            b if b == close => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => i += 1,
        }
    }
    // Unbalanced — leave the caller's position untouched.
    start
}

/// Skips whitespace and `//`/`/* */` comments; returns the first
/// non-trivia byte index at/after `i`.
fn ws_skip(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            _ => break,
        }
    }
    i
}

/// The end index of one identifier lexeme starting at `i`
/// (`[A-Za-z_$][A-Za-z0-9_$]*`); `i` unchanged when the byte is not an
/// identifier start.
fn ident_end(bytes: &[u8], i: usize) -> usize {
    match bytes.get(i) {
        Some(b) if b.is_ascii_alphabetic() || *b == b'_' || *b == b'$' => {
            let mut j = i + 1;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'$')
            {
                j += 1;
            }
            j
        }
        _ => i,
    }
}

/// Whether the identifier ending at `i` is `kw` — walks back over the
/// lexeme and confirms a non-identifier boundary on its left (a `myFunction`
/// or `xFunction` identifier never aliases the keyword).
fn preceded_by_keyword(bytes: &[u8], i: usize, kw: &[u8]) -> bool {
    let mut j = i;
    while j > 0 && (bytes[j - 1].is_ascii_alphanumeric() || bytes[j - 1] == b'_') {
        j -= 1;
    }
    &bytes[j..i] == kw
}

/// Collects the byte spans of every leaf identifier inside the
/// `<...>` type-argument group `(open, close)` that spells an in-scope
/// type parameter — the recursive-leaf face of the oracle's
/// `_checkForConstWithTypeParameters` walk. A nested `<...>` group a
/// `Function` keyword opens is that function type's OWN parameter list
/// (its idents are exempt `allowedTypeParameters` upstream), so the scan
/// steps over it; any other nested group's interior is recursed so a
/// parameterized argument's inner `T` leaf reports at its own span.
fn param_leaf_spans(
    bytes: &[u8],
    source: &str,
    strings: &StringTable,
    open: usize,
    close: usize,
    params: &[StringId],
    spans: &mut Vec<(usize, usize)>,
) {
    let mut j = open + 1;
    while j < close.saturating_sub(1) {
        match bytes[j] {
            b'<' => {
                let inner_end = skip_balanced(bytes, j, b'<');
                if inner_end == j {
                    break;
                }
                if !preceded_by_keyword(bytes, j, b"Function") {
                    param_leaf_spans(bytes, source, strings, j, inner_end, params, spans);
                }
                j = inner_end;
            }
            b if b.is_ascii_alphabetic() || b == b'_' || b == b'$' => {
                let end = ident_end(bytes, j);
                let text = &source[j..end];
                if params.iter().any(|p| strings.resolve(*p) == text) {
                    spans.push((j, end));
                }
                j = end;
            }
            _ => j += 1,
        }
    }
}

impl ConstLookup for CtorScope<'_, '_> {
    fn ident(&self, name: StringId) -> IdentVerdict {
        match self.env.get(&name) {
            Some(CtorBinding::Value(v)) => IdentVerdict::Value(v.clone()),
            Some(CtorBinding::Opaque) => IdentVerdict::Constant,
            Some(CtorBinding::Invalid) => IdentVerdict::NotConstant,
            None => self.engine.ident(name),
        }
    }

    fn member(&self, recv: &ConstantValue, name: StringId) -> Verdict {
        self.engine.member(recv, name)
    }

    fn prefixed(&self, prefix: StringId, name: StringId) -> Option<IdentVerdict> {
        if self.env.contains_key(&prefix) {
            return None;
        }
        self.engine.prefixed(prefix, name)
    }

    fn is_identical(&self, name: StringId) -> bool {
        self.engine.is_identical(name)
    }

    fn created_state(&self, container: StringId) -> CreatedState {
        self.engine.created_state(container)
    }

    fn shorthand_context(&self, span: SourceRange) -> bool {
        self.engine.shorthand_context(span)
    }

    /// `isSubtypeOf(runtimeType(value), ty)` under the creation's
    /// argument values; the constructor's type parameters were already
    /// substituted into the captured `Expr` trees at
    /// [`Self::check_ctor_initializers`] (`subst_init_tys`), so a
    /// parameter-spelled target arriving here is genuinely unbound and
    /// defers like the engine face.
    fn value_subtype(&self, value: &ConstantValue, ty: &Ty) -> Option<bool> {
        self.engine.value_subtype(value, ty)
    }
}

impl ConstLookup for Engine<'_> {
    /// Bare-name classification for the const evaluator: const-environment
    /// bindings first (a self-reference is the cycle fault), then
    /// in-scope locals (provably non-const), then the model (containers
    /// fold to type literals, top-level functions are constant tear-offs;
    /// everything else defers — the rows report genuine misses).
    fn ident(&self, name: StringId) -> IdentVerdict {
        for scope in self.const_scopes.iter().rev() {
            match scope.get(&name) {
                Some(ConstBinding::Value(v)) => return IdentVerdict::Value(v.clone()),
                Some(ConstBinding::InProgress) => return IdentVerdict::InProgress,
                // A failed const binding never re-reports: the declaring
                // initializer carried the error (`avoidReporting`), so a
                // later read folds as a deferring constant (`const h =
                // C(g)` over a failed `g` stays silent).
                Some(ConstBinding::Failed) => return IdentVerdict::Constant,
                None => {}
            }
        }
        // A wildcard name (`_`) never denotes a readable const variable, but
        // a read of it is *undefined* only when nothing declares the
        // spelling: a `typedef _`/`class _` folds to its type literal (a
        // valid constant) and a `_`-named local answers itself, so the read
        // defers. Upstream pairs `undefined_identifier` with the const site
        // default for the undeclared spelling only (`const y = _;` with no
        // `_` in scope); after `class _ {}` or `typedef _ = Type;` the same
        // initializer is a type literal constant and nothing reports. A
        // wildcard local is never a `NotConstant` either — it binds nothing
        // to be non-constant.
        if self.is_wildcard(name) {
            let declared = self.frames.iter().any(|f| f.locals.contains(&name))
                || self.top_level_decl(name).is_some()
                || !matches!(self.resolve_bare(name).0, BareInfo::Missing { .. });
            return if declared {
                IdentVerdict::Constant
            } else {
                IdentVerdict::Unresolved
            };
        }
        if self.frames.iter().any(|f| f.locals.contains(&name)) {
            return IdentVerdict::NotConstant;
        }
        match self.resolve_bare(name).0 {
            BareInfo::Container { name, .. } => IdentVerdict::Value(ConstantValue::Type(name)),
            BareInfo::Plain { .. } => match self.top_level_decl(name).map(|d| d.kind) {
                Some(MicroKind::TopLevelVar) => {
                    if self.nonconst_top_level.contains(&name) {
                        IdentVerdict::NotConstant
                    } else {
                        IdentVerdict::Constant
                    }
                }
                // A bare read of the enclosing container's own non-const
                // field (`static int c` read as `c` inside `class C`): the
                // oracle resolves it through the class scope, where the
                // member is provably not a constant (`const x = c` fires at
                // `c`). The composite key joins at `const_decl_name`.
                _ => {
                    if let Some((container, _)) = self.container_context() {
                        let key = format!(
                            "{}\u{2}{}",
                            self.strings.resolve(container),
                            self.strings.resolve(name)
                        );
                        if self.nonconst_top_level.contains(&self.strings.intern(&key)) {
                            return IdentVerdict::NotConstant;
                        }
                    }
                    IdentVerdict::Constant
                }
            },
            // An unresolved name is never a constant value (the official
            // evaluates it `dynamic`, which fails every constant-argument
            // check — `const_with_non_constant_argument`); a broken-import
            // suppression stays deferential (the undefined name is the
            // reported root cause, #1351). The Unresolved grade keeps the
            // argument face hot while the declaration's own verdict stays
            // silent (the oracle's `hasKnownValue` gate — the undefined
            // name already carries the report).
            BareInfo::Missing { .. } => {
                // A bare `dynamic` read is a valid `Type` literal constant
                // (`const d = dynamic` — the resolver's `dynamic` pseudo-
                // element is a `TypeLiteral`, `TypeState` of unknown reified
                // type but a legal constant); the name never reaches the
                // model's resolution faces, so the bare-name path defers.
                if self.strings.resolve(name) == "dynamic" {
                    IdentVerdict::Constant
                } else if self.guard.ignore(None, name) {
                    IdentVerdict::Constant
                } else {
                    IdentVerdict::Unresolved
                }
            }
            _ => IdentVerdict::Constant,
        }
    }

    /// `recv.name` where `recv` is a type literal: the in-file enum's
    /// constants fold to enum values; this pass's captured static const
    /// field values fold; everything else (synthesized enum members,
    /// foreign/static faces) defers.
    fn member(&self, recv: &ConstantValue, name: StringId) -> Verdict {
        let ConstantValue::Type(container) = recv else {
            return Verdict::Constant;
        };
        if let Some(c) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(*container))
            && c.kind == ContainerKind::Enum
            && c.enum_constants.contains(&name)
        {
            return Verdict::Value(ConstantValue::Enum {
                container: *container,
                name,
            });
        }
        if let Some(v) = self.const_field_values.get(&(*container, name)) {
            return Verdict::Value(v.clone());
        }
        Verdict::Constant
    }

    /// `p.name` through an import prefix: a name the prefix's namespace
    /// answers stays a deferring constant (its value is foreign); a miss —
    /// a broken import, or a name its library does not export — is
    /// unresolved (`const x = [diag.a]` over a missing import reports at
    /// the reference, exactly like a bare undefined name).
    fn prefixed(&self, prefix: StringId, name: StringId) -> Option<IdentVerdict> {
        if self.const_scopes.iter().any(|s| s.contains_key(&prefix))
            || self.frames.iter().any(|f| f.locals.contains(&prefix))
            || !matches!(self.resolve_bare(prefix).0, BareInfo::Prefix { .. })
        {
            return None;
        }
        Some(if self.prefixed_hit(prefix, name) {
            IdentVerdict::Constant
        } else {
            IdentVerdict::Unresolved
        })
    }

    fn is_identical(&self, name: StringId) -> bool {
        name == self.identical_id
    }

    /// The `Created(container)` values this file can push carry the
    /// container's own name (`bool.fromEnvironment` → `Created(bool)`);
    /// the oracle state class is the primitive-table lookup — every other
    /// container is a generic instance whose operand asserts fault.
    fn created_state(&self, container: StringId) -> CreatedState {
        let ids = &self.members.primitive_ids;
        if container == ids[3] {
            CreatedState::Bool
        } else if container == ids[0] {
            CreatedState::Int
        } else if container == ids[1] || container == ids[2] {
            CreatedState::Num
        } else if container == ids[4] {
            CreatedState::String
        } else {
            CreatedState::Generic
        }
    }

    fn shorthand_context(&self, span: SourceRange) -> bool {
        self.dot_shorthand_context(span.offset)
    }

    /// `isSubtypeOf(runtimeType(value), ty)` — the `castToType`/`hasType`
    /// compare (value.dart:378 / :617): the value's own runtime-type face
    /// against the WRITTEN target as the resolved `Ty` spells it.
    ///
    /// `Some` decides, `None` defers to the valid-but-unknown face:
    /// - `Ty::Unknown` (the recovered `InvalidType` splice — upstream's
    ///   `isInvalid || castType.isInvalid → return this`, value.dart:368);
    /// - an in-scope type PARAMETER still spelled `T` (upstream's
    ///   `hasTypeParameterReference → return this`, value.dart:373 —
    ///   `castToType` only; `hasType` decides it, but the subtype verdict
    ///   over an unbound parameter head is meaningless at this face, so
    ///   both folds read the same deferral);
    /// - a `Record`/`Function` shaped target the name-level lattice does
    ///   not reach (the sealed shapes carry no interface head).
    ///
    /// The value→type map follows the oracle's `DartObjectImpl.type`:
    /// literals to their primitive heads, `Enum`/`Created`/`Type`/`Symbol`
    /// to their container/interface faces, collections to the raw generic
    /// head (the element type does not ride the subset's value) — the
    /// `List<dynamic>` approximation faults only where the oracle's own
    /// `List<int>` runtime type would already differ.
    fn value_subtype(&self, value: &ConstantValue, ty: &Ty) -> Option<bool> {
        let ids = &self.members.primitive_ids;
        let value_ty = match value {
            ConstantValue::Null => Ty::Null,
            ConstantValue::Bool(_) => Ty::named(ids[3]),
            ConstantValue::Int(_) => Ty::named(ids[0]),
            ConstantValue::Double(_) => Ty::named(ids[1]),
            ConstantValue::Str(_) => Ty::named(ids[4]),
            ConstantValue::Enum { container, .. } | ConstantValue::Created(container) => {
                Ty::named(*container)
            }
            ConstantValue::Type(_) => Ty::named(self.intern("Type")),
            ConstantValue::Symbol(_) => Ty::named(self.intern("Symbol")),
            ConstantValue::List(_) | ConstantValue::Instance(InstanceKind::List) => {
                Ty::named(self.intern("List"))
            }
            ConstantValue::Set(_) | ConstantValue::Instance(InstanceKind::Set) => {
                Ty::named(self.intern("Set"))
            }
            ConstantValue::Map(_) | ConstantValue::Instance(InstanceKind::Map) => {
                Ty::named(self.intern("Map"))
            }
            ConstantValue::Record { .. } | ConstantValue::Instance(InstanceKind::Record) => {
                Ty::named(self.intern("Record"))
            }
            // The `Object`/`Function` instance placeholders carry no
            // container name — the runtime type face is unreadable here.
            ConstantValue::Instance(InstanceKind::Function | InstanceKind::Object) => {
                return None;
            }
        };
        match ty {
            // `isInvalid || castType.isInvalid → return this` — the
            // recovered splices and the unresolved-type face both pass.
            Ty::Unknown | Ty::Dynamic | Ty::Void => Some(true),
            // `hasTypeParameterReference → return this`: a parameter
            // head in scope defers (the creation's substitution runs in
            // `subst_init_tys`; a surviving spelling is unbound).
            Ty::Named { name, .. } if self.is_scope_param(*name) => None,
            Ty::Named { .. } => Some(self.name_subtype(value_ty, *ty)),
            // `Null` as a target answers on the value's own nullability
            // face; `Never` rejects every value but `Never`'s.
            Ty::Null | Ty::Never => Some(self.name_subtype(value_ty, *ty)),
            // Record/function-shaped targets and any other spelling the
            // name level cannot prove stay deferred (the under-report).
            Ty::Record { .. } | Ty::Function { .. } => None,
        }
    }
}

impl<'a> Engine<'a> {
    /// Arms a declaration group as a const context (the `const` keyword
    /// seen on a local-variable or field group).
    pub fn arm_const_group(&mut self) {
        self.const_group = true;
        self.const_group_names.clear();
        self.const_group_cursor = 0;
    }

    /// A declaration-group name: const groups queue it for their
    /// initializer windows; a non-const *top-level* variable joins the
    /// provable non-const set (in-file, declaration order). A non-const
    /// field declaration joins the same set under a `{container}\u{2}{name}`
    /// composite key — `finish_const_initializer`'s member walk answers
    /// `C.c` on a type literal from it (the bare-name key stays reserved
    /// for top-level variables).
    pub fn const_decl_name(&mut self, name: StringId, field_context: bool) {
        if self.const_group {
            self.const_group_names.push(name);
        } else if field_context {
            if let Some((container, _)) = self.container_context() {
                let key = format!(
                    "{}\u{2}{}",
                    self.strings.resolve(container),
                    self.strings.resolve(name)
                );
                let key = self.intern(&key);
                self.nonconst_top_level.insert(key);
            } else if self.frames.is_empty() {
                self.nonconst_top_level.insert(name);
            }
        }
    }

    /// The leftmost `recv.name` subtree of `node` that reads a declared
    /// non-const field through a type literal (`const x = C.c`, and the
    /// `C.c` inside `C.c.v`): `member()` defers every unresolved static
    /// face to a silent constant, so the walk re-asks the resolution
    /// layer — a `BareInfo::Container` receiver keyed `{container}\u{2}{name}`
    /// into `nonconst_top_level` proves the field was declared non-const
    /// in this file. Mirrors upstream's first-failure order (evaluation
    /// is left-to-right), so only the smallest-offset candidate is kept.
    fn nonconst_member_span(&self, node: &crate::const_eval::Node) -> Option<SourceRange> {
        let mut best: Option<SourceRange> = None;
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            match &n.expr {
                Expr::Member { recv, name } => {
                    if let Expr::Ident(head) = &recv.expr
                        && let BareInfo::Container { name: c, .. } = self.resolve_bare(*head).0
                    {
                        let key = format!(
                            "{}\u{2}{}",
                            self.strings.resolve(c),
                            self.strings.resolve(*name)
                        );
                        if self.nonconst_top_level.contains(&self.strings.intern(&key))
                            && best.is_none_or(|b| n.span.offset < b.offset)
                        {
                            best = Some(n.span);
                        }
                    }
                    stack.push(recv);
                }
                Expr::Binary { left, right, .. } => {
                    stack.push(left);
                    stack.push(right);
                }
                Expr::Unary { operand, .. } => stack.push(operand),
                Expr::Cond { cond, then, els } => {
                    stack.push(cond);
                    stack.push(then);
                    stack.push(els);
                }
                Expr::Interp { parts }
                | Expr::Adjacent(parts)
                | Expr::List(parts)
                | Expr::Set(parts)
                | Expr::Record(parts) => {
                    for part in parts {
                        stack.push(part);
                    }
                }
                Expr::Map(parts) => {
                    for part in parts {
                        stack.push(part);
                    }
                }
                Expr::Entry(key, value) => {
                    stack.push(key);
                    stack.push(value);
                }
                Expr::IfElement { cond, then, els } => {
                    stack.push(cond);
                    stack.push(then);
                    if let Some(els) = els {
                        stack.push(els);
                    }
                }
                Expr::Spread { operand, .. } => stack.push(operand),
                Expr::Call { args, .. } => {
                    for arg in args {
                        stack.push(arg);
                    }
                }
                _ => {}
            }
        }
        best
    }

    /// The const group closes: any declared name whose slot the cursor
    /// never consumed had no initializer — `const_not_initialized` reports
    /// at the name token (upstream `visitVariableDeclaration`'s
    /// `variable.initializer == null` arm, `constant_verifier.dart:550-556`).
    /// Only LOCAL groups emit here — field and top-level const declarations
    /// are covered by the parser's own `constNotInitialized` message
    /// (`members.rs`'s declarator walk), whose `parse_diagnostics` rows the
    /// report merge already carries. A field group sits outside any
    /// executable frame, so `frames.is_empty()` identifies the parser-owned
    /// arm; inside a body `frames` is non-empty and this face owns the row.
    /// The name's token range comes from the token table — the names arrive
    /// in declaration order, so a forward walk from the previous match
    /// lands on each one's spelling.
    pub fn disarm_const_group(&mut self) {
        if self.const_group {
            let names = std::mem::take(&mut self.const_group_names);
            // Only a LOCAL group emits here — a field/top-level group's
            // rows arrive through the parser's own `constNotInitialized`
            // face (`members.rs`'s declarator walk). A field group sits
            // inside a `Container` frame, a top-level group has no frame;
            // a local group's innermost frame is `Executable`.
            let local = self
                .frames
                .last()
                .is_some_and(|f| matches!(f.kind, FrameKind::Executable { .. }));
            if local {
                // Resolve (name, span) pairs first — the emit borrows `&mut
                // self`, so the token walk must finish before any row fires.
                let spans: Vec<(StringId, SourceRange)> = {
                    let toks = self.tokens.as_slice();
                    let mut cursor = 0usize;
                    names
                        .iter()
                        .skip(self.const_group_cursor)
                        .map(|name| {
                            let name_text = self.strings.resolve(*name);
                            while cursor < toks.len()
                                && self.tokens.lexeme(self.source, cursor) != name_text
                            {
                                cursor += 1;
                            }
                            let range = if cursor < toks.len() {
                                let t = &toks[cursor];
                                let end = self.utf16_of((t.byte_offset + t.byte_len) as usize);
                                let r = SourceRange {
                                    offset: t.utf16_offset,
                                    length: end - t.utf16_offset,
                                };
                                cursor += 1;
                                r
                            } else {
                                SourceRange {
                                    offset: 0,
                                    length: 0,
                                }
                            };
                            (*name, range)
                        })
                        .collect()
                };
                for (name, range) in spans {
                    let name_text = self.strings.resolve(name);
                    self.emit(SemanticCode::ConstNotInitialized, range, name_text, "");
                }
            }
            // `std::mem::take` already cleared the vector; restore it for reuse.
            self.const_group_names = names;
            self.const_group_names.clear();
        }
        self.const_group = false;
        self.const_group_cursor = 0;
    }

    /// A const initializer window opens: the group's next name marks
    /// in-progress (a self-reference inside is the cycle fault).
    pub fn open_const_initializer(&mut self) {
        if !self.const_group {
            return;
        }
        let Some(name) = self.const_group_names.get(self.const_group_cursor).copied() else {
            return;
        };
        self.const_group_cursor += 1;
        if let Some(scope) = self.const_scopes.last_mut() {
            scope.insert(name, ConstBinding::InProgress);
        }
        // The pending slot keys the initializer's staged creation entries
        // (`open_const_creations`/`pending_const_creations`'s `init`
        // field): the oracle codes one row per *initializer* while the
        // name alone collides for sibling `const event` declarations in
        // different scopes, so the pushed id carries a per-declaration
        // suffix (`{name}\u{1}{rows}:{diags}` — both counters grow over
        // any entry-staging initializer, making the pair unique per
        // declaration site). `finish_const_initializer` splits the real
        // name back out for the binding faces; held push sites read
        // `.last()` verbatim and propagate the keyed id into the entries
        // the tail dedups on.
        let site = format!(
            "{}\u{1}{}:{}",
            self.strings.resolve(name),
            self.rows.len(),
            self.report.diagnostics.len()
        );
        let keyed = self.intern(&site);
        self.pending_init_names.push(keyed);
        self.const_capture.open_initializer();
    }

    /// A const local-variable initializer closes: evaluate, report per
    /// the official site mapping, bind the folded value and type.
    pub fn close_const_initializer(&mut self) {
        self.finish_const_initializer(true);
    }

    /// A const field initializer closes (class static consts and
    /// top-level consts — const fields are implicitly static).
    pub fn close_const_field_initializer(&mut self) {
        self.finish_const_initializer(false);
    }

    pub(crate) fn finish_const_initializer(&mut self, local: bool) {
        // Every variable-initializer close routes here (const and non-const
        // alike — the const machinery below early-returns on the non-const
        // shape): the declaration group's leading-dot shorthands resolve
        // against the armed annotation while `pending_decl_ty` still
        // carries it (#3433).
        self.resolve_decl_shorthands();
        let Some(keyed) = self.pending_init_names.pop() else {
            return;
        };
        // The pending slot carries the site-keyed id (`{name}\u{1}…` —
        // see `open_const_initializer`): the binding faces below need the
        // declared name itself, not the dedup key.
        let resolved = self.strings.resolve(keyed);
        let name = match resolved.split_once('\u{1}') {
            Some((real, _)) => self.intern(real),
            None => keyed,
        };
        if !self.const_capture.open() {
            return;
        }
        let root = self.const_capture.close_initializer();
        // The decl-root dot shorthand with no written context type
        // (`const a = .new();`, `const a = .id;`): the capture folds the
        // selector to a bare name, so the root node's own shape names the
        // shorthand — see [`Self::report_root_shorthand`].
        if let Some(node) = &root {
            self.report_root_shorthand(node);
        }
        let verdict = root
            .as_ref()
            .map_or(Verdict::Constant, |node| eval(node, self));
        // A non-const field read through a type literal (`const x = C.c`)
        // defers through `member()`'s silent arm — the walk re-discovers it
        // and the smallest-offset candidate answers the oracle's
        // left-to-right first failure (it beats a `NotConstant`/`Unresolved`
        // verdict anchored further right; an allow-list fault keeps its own
        // code).
        let member_span = root
            .as_ref()
            .and_then(|node| self.nonconst_member_span(node));
        let member_wins = match (&verdict, member_span) {
            (Verdict::Fault(..), _) | (_, None) => false,
            (Verdict::NotConstant(s) | Verdict::Unresolved(s), Some(m)) => m.offset < s.offset,
            // A `Constant` verdict defers every static face silently, and a
            // `Value` collection CONTAINING such a deferred read (`const l =
            // [C.c]` folds a list whose element defers) keeps the same
            // blind spot — the walk re-decides both against the resolution
            // layer (probed 3.13.7: the oracle reports
            // `const_initialized_with_non_constant_value` at the `C.c`
            // inside the list, beside the element face).
            (Verdict::Value(_) | Verdict::Constant, Some(_)) => true,
        };
        if let Some(span) = member_span.filter(|_| member_wins) {
            self.emit(
                SemanticCode::ConstInitializedWithNonConstantValue,
                span,
                "",
                "",
            );
        } else if let Verdict::Fault(ConstFault::ForElement, _) = verdict {
            // The element face already reported `const_eval_for_element`
            // at the element. A top-level/field const's own value-
            // computation wraps the initializer as
            // `genericError(node.initializer)` upstream — non-allow-
            // listed, so the site default `const_init` fires at the whole
            // initializer expression (the literal). A LOCAL const's
            // `visitVariableDeclaration` `_reportError` carries the
            // element's `InvalidConstant` verbatim — the allow-list swaps
            // the code at the element, no twin fires (nothing more to do).
            if !local {
                if let Some(span) = root.as_ref().map(|node| node.span) {
                    self.emit(
                        SemanticCode::ConstInitializedWithNonConstantValue,
                        span,
                        "",
                        "",
                    );
                }
            }
        } else {
            self.resolve_const_verdict(&verdict);
        }
        // This initializer's creation candidates: an immediate failure
        // (a genuine non-constant operand) already reported — the oracle
        // codes one row per initializer, so the deferred entries drop; a
        // passing verdict promotes them to the tail pass, whose whole-file
        // const-ctor set decides.
        // The `const_init` verdict is the oracle's site default — it fires
        // only when the root's own error code is NOT allow-listed
        // (`_reportError`'s constEval* override). An allow-list fault row
        // emitted inside this initializer whose span contains a staged
        // entry's span already reported that node, so the entry's
        // `const_init` twin drops; a `Creation` entry's `const_with_non_const`
        // face is per-node and still fires (it is the allow-list row's
        // sibling, not the site default). The spans snapshot first — the
        // loop below emits and pushes through `&mut self`.
        // The `const_init` verdict anchors at the deepest node whose error
        // is NOT allow-listed (`_reportError`'s constEval* override). An
        // allow-list row whose span contains a staged entry's span already
        // reported that subtree at a `constEval*` code — UNLESS the entry
        // itself already took a non-allow-listed row at its own span (the
        // `const_with_non_constant_argument` of a bad creation argument —
        // the arg's own face is what `const_init` anchors on). The spans
        // snapshot first — the loop below emits and pushes through `&mut
        // self`.
        let (fault_spans, named_spans): (Vec<SourceRange>, Vec<SourceRange>) = {
            let mut faults = Vec::new();
            let mut named = Vec::new();
            for row in &self.report.diagnostics {
                if row.code.name.starts_with("const_eval") {
                    faults.push(row.range);
                } else {
                    named.push(row.range);
                }
            }
            (faults, named)
        };
        let fault_covers = |span: SourceRange| {
            fault_spans
                .iter()
                .any(|f| f.offset <= span.offset && span.end() <= f.end())
                && !named_spans.iter().any(|&s| s == span)
        };
        for (init, span, kind) in std::mem::take(&mut self.open_const_creations) {
            if !matches!(verdict, Verdict::Value(_) | Verdict::Constant) {
                continue;
            }
            match kind {
                ConstCreationEntry::ArgFailure if fault_covers(span) => {}
                ConstCreationEntry::Creation { container } if fault_covers(span) => {
                    // The node's own creation face still fires (the
                    // `const_init` site-default is what the fault replaces).
                    if !self.const_ctor_target(container) {
                        self.emit(SemanticCode::ConstWithNonConst, span, "", "");
                    }
                }
                _ => self.pending_const_creations.push((init, span, kind)),
            }
        }
        if let Verdict::Value(value) = &verdict {
            let folded = self.folded_ty(value);
            if local {
                // A WRITTEN annotation keeps the declared slot it already
                // owns: `bind_declared_type` bound it at the name event,
                // and the oracle re-types a local from its initializer only
                // for an UNANNOTATED declarator — `variable_declaration_
                // resolver.dart:84`'s `parent.type == null && element is
                // LocalVariableElementImpl` gate. Overwriting the slot with
                // the constant's VALUE type made every later read of a
                // `const Object o = 'a'` / `const dynamic d = 3` see
                // `String`/`int` instead of the declared face (the census's
                // `const dynamic i1 = 3; i1 | 2.2` family: the operator's
                // argument check then read an `int` receiver and reported
                // `argument_type_not_assignable`, where the oracle reads
                // `dynamic` and stays silent).
                if self.pending_decl_ty.is_none() {
                    self.bind_var_type(name, folded);
                }
            } else if let Some((container, _)) = self.container_context() {
                self.const_field_values
                    .insert((container, name), value.clone());
            }
            // The value joins the const environment: locals in their
            // frame scope, top-level consts in the file scope (later
            // in-file initializers fold from it).
            let scope = if local || !self.frames.is_empty() {
                self.const_scopes.last_mut()
            } else {
                self.const_scopes.first_mut()
            };
            if let Some(scope) = scope {
                scope.insert(name, ConstBinding::Value(value.clone()));
            }
        } else {
            // The failed binding stays answerable as a deferring constant
            // (never the frame-local `NotConstant` miss): a later read of
            // a failed const is silent (`avoidReporting`).
            let scope = if local || !self.frames.is_empty() {
                self.const_scopes.last_mut()
            } else {
                self.const_scopes.first_mut()
            };
            if let Some(scope) = scope {
                scope.insert(name, ConstBinding::Failed);
            }
        }
    }

    /// Widens a `new`-keyword-anchored non-constancy to the whole
    /// `InstanceCreationExpression`. Upstream rejects a non-`const`
    /// creation with `InvalidConstant.genericError(node: node)` where
    /// `node` is the creation itself — `new` through the argument list's
    /// closing paren (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/constant/evaluation.dart:1035,
    /// `visitInstanceCreationExpression`; every corpus oracle row for
    /// this shape spans `new C()` / `new Object()` whole, none anchors
    /// at the bare keyword). The capture folds a `new` expression to its
    /// keyword token (listener `end_new_expression`), so this emit site
    /// re-reads the creation's extent: the type head, optional balanced
    /// type arguments, `.selector` repeats, and one balanced argument
    /// list. Any span that is not exactly a `new` keyword followed by a
    /// type name stays untouched.
    fn new_creation_extent(&self, span: SourceRange) -> SourceRange {
        if span.length != 3 {
            return span;
        }
        let bytes = self.source.as_bytes();
        let start = self.byte_of(span.offset);
        if self.source.get(start..start + 3) != Some("new") {
            return span;
        }
        let mut i = start + 3;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if !bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_' || *b == b'$')
        {
            return span;
        }
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
        {
            i += 1;
        }
        // Type arguments (`new C<int>()`), then `.selector` repeats
        // (`new p.C()`, `new C.named()`), then one argument list.
        i = skip_balanced(bytes, i, b'<');
        while bytes.get(i) == Some(&b'.')
            && bytes
                .get(i + 1)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
        {
            i += 2;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
            {
                i += 1;
            }
            i = skip_balanced(bytes, i, b'<');
        }
        i = skip_balanced(bytes, i, b'(');
        SourceRange {
            offset: span.offset,
            length: self.utf16_of(i).saturating_sub(span.offset),
        }
    }

    /// Reports one const verdict at its site mapping: a fault carries its
    /// own `constEval*` code (the oracle's allow-list override), a
    /// generic non-constancy takes the site default
    /// (`const_initialized_with_non_constant_value`).
    pub(crate) fn resolve_const_verdict(&mut self, verdict: &Verdict) {
        match verdict {
            Verdict::Value(_) | Verdict::Constant => {}
            Verdict::NotConstant(span) => {
                let span = self.new_creation_extent(*span);
                self.emit(
                    SemanticCode::ConstInitializedWithNonConstantValue,
                    span,
                    "",
                    "",
                );
            }
            // An unresolved root (`const a = x`) reports like a plain
            // non-constancy: the oracle's evaluation of the unresolvable
            // reference is known-invalid at the variable. A leading-dot
            // shorthand (`.blue` — `DotShorthandNameExpression` resolves
            // against the context type) reads unresolved through the
            // bare-name path the capture folds it to: a RESOLVED shorthand
            // defers like any constant (the declaration shorthand pass
            // upgraded its row's type), while a FAILED one — the context
            // type cannot answer the member — is the oracle's
            // `invalidConstant` at the whole shorthand node (the dot
            // included), alongside its own `dot_shorthand_*` face row.
            Verdict::Unresolved(span) => {
                if !self.dot_shorthand_context(span.offset) {
                    self.emit(
                        SemanticCode::ConstInitializedWithNonConstantValue,
                        *span,
                        "",
                        "",
                    );
                } else if !self.shorthand_resolved_at(*span) {
                    let whole = self.shorthand_whole_span(*span);
                    // A decl-root shorthand's `report_root_shorthand` already
                    // landed this row; the Unresolved verdict's twin collapses.
                    let info = SemanticCode::ConstInitializedWithNonConstantValue.code_info();
                    if !self
                        .report
                        .diagnostics
                        .iter()
                        .any(|d| d.range == whole && d.code == info)
                    {
                        self.emit(
                            SemanticCode::ConstInitializedWithNonConstantValue,
                            whole,
                            "",
                            "",
                        );
                    }
                }
            }
            Verdict::Fault(fault, span) => self.emit_const_fault(*fault, *span),
        }
    }

    /// Narrows an operator fault's anchor to the prefix application that
    /// produced it.
    ///
    /// Upstream anchors every constant fault at the *operator node*:
    /// `ConstantVisitor.visitPrefixExpression` evaluates the operand and
    /// returns a non-`DartObjectImpl` verdict verbatim, so the reported
    /// entity is the innermost `PrefixExpression` that threw (upstream
    /// Dart SDK source, pkg/analyzer/lib/src/dart/constant/
    /// evaluation.dart, `visitPrefixExpression`; `value.dart`'s
    /// `negated`/`bitNot`/`logicalNot` throw with that node).
    ///
    /// The capture latches every prefix token of a run innermost-first
    /// (`unary_prefix` fires when each operand completes) and applies
    /// them outermost-first, so a nested run like `!!D` or `- - D`
    /// arrives here with the OUTER application's extent while the fault
    /// it carries was thrown by the INNER one. When the span's first
    /// byte is itself an operator, the correct anchor is the LAST
    /// operator of a HOMOGENEOUS run (`!!`, `- -`, `~ ~`) — a run of one
    /// repeated op can only fault in its innermost application (the
    /// inner fold answers a Bool/Int/Double the same op accepts), so the
    /// throwing `PrefixExpression` always starts at the operand-adjacent
    /// token; `!!D` narrows to the oracle's `(35,2)`, `- - D` to
    /// `(51,3)`. A mixed run (`!-x`) faults at the OUTER op instead,
    /// and a composite that merely begins with an operator (`- -x / 0`)
    /// anchors at its own first byte — both keep the incoming span. A
    /// single-operator run narrows onto its own byte, so `!D`, `-'a'`
    /// and the type-fault pins are untouched.
    ///
    /// A span that still anchors at the operand walks back to its own
    /// prefix instead: a candidate operator the *binary* grammar
    /// produced keeps the span untouched (`1 - x` and `a ~ b` put a
    /// value-ending byte left of the operator, which a prefix operator
    /// never has — it follows an opener, another operator, or nothing).
    fn unary_fault_extent(&self, span: SourceRange) -> SourceRange {
        let bytes = self.source.as_bytes();
        let start_byte = self.byte_of(span.offset).min(bytes.len());
        if matches!(bytes.get(start_byte), Some(b'!' | b'-' | b'~')) {
            // The operator run is `op (ws|op)*`. A HOMOGENEOUS run
            // (`!!`, `- -`, `~ ~`) can only fault in its innermost
            // application — the inner fold answers a Bool/Int/Double the
            // same operator accepts — so the run's LAST operator token
            // is the throwing `PrefixExpression`'s start. A mixed run
            // (`!-x`) faults at the outer operator, and a composite that
            // merely STARTS with an operator (`- -x / 0`, `-x ? y : z`)
            // anchors at its own first byte, so the narrow requires the
            // whole run to spell one operator and the operand region to
            // carry no top-level infix byte.
            let end_byte = self.byte_of(span.end()).min(bytes.len());
            let op_byte = bytes[start_byte];
            let mut last_op = start_byte;
            let mut p = start_byte + 1;
            while p < end_byte {
                let b = bytes[p];
                if b == op_byte {
                    last_op = p;
                    p += 1;
                } else if b.is_ascii_whitespace() {
                    p += 1;
                } else {
                    break;
                }
            }
            let mut depth = 0u32;
            let mut composite = last_op == start_byte;
            let mut q = p;
            while q < end_byte && !composite {
                match bytes[q] {
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                    b'\'' | b'"' => {
                        // Skip a quoted run (raw prefixes never reach here —
                        // the `r` rides inside the operand's ident head);
                        // escapes are consumed so `\'` does not close early.
                        let quote = bytes[q];
                        q += 1;
                        while q < end_byte && bytes[q] != quote {
                            q += if bytes[q] == b'\\' { 2 } else { 1 };
                        }
                    }
                    b'+' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^' | b'<' | b'>' | b'=' | b'?'
                    | b':' | b'-' | b'~' | b'!'
                        if depth == 0 =>
                    {
                        composite = true
                    }
                    _ => {}
                }
                q += 1;
            }
            if !composite {
                let start = self.utf16_of(last_op);
                return SourceRange {
                    offset: start,
                    length: span.end().saturating_sub(start),
                };
            }
            return span;
        }
        let mut i = start_byte;
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        let Some(op) = i.checked_sub(1) else {
            return span;
        };
        if !matches!(bytes[op], b'!' | b'-' | b'~') {
            return span;
        }
        let mut left = op;
        while left > 0 && bytes[left - 1].is_ascii_whitespace() {
            left -= 1;
        }
        if let Some(&b) = left.checked_sub(1).and_then(|j| bytes.get(j))
            && (b.is_ascii_alphanumeric()
                || b == b'_'
                || b == b'$'
                || matches!(b, b')' | b']' | b'}' | b'\'' | b'"'))
        {
            return span;
        }
        let start = self.utf16_of(op);
        SourceRange {
            offset: start,
            length: span.end().saturating_sub(start),
        }
    }

    /// A const-eval fault reports its allow-listed `constEval*` code. The
    /// throws-exception face carries the oracle's context row (#1876 W5):
    /// upstream wraps a runtime-exception `InvalidConstant` into
    /// `constEvalThrowsException` and links the throw site with
    /// `"The exception is '<formattedMessage>' and occurs here."`
    /// (upstream Dart SDK source, pkg/analyzer/lib/src/dart/constant/
    /// evaluation.dart:352-373). Every generic runtime path in
    /// `value.dart` throws `EvaluationException(diag.constEvalThrowsException)`
    /// itself, so the inner formatted message is that code's
    /// problemMessage — the constant below (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart,
    /// `constEvalThrowsException`); the inner span is the verdict's own
    /// fault span (the deepest construct that threw — the same entity the
    /// upstream context row anchors at). `ConstFault::ThrowsException`
    /// therefore needs no payload of its own: the variant's doc records
    /// the pairing, and this is the only emitter that reads it. The
    /// super-constructor stack-trace contexts (evaluation.dart:3453/:3464)
    /// stay out of scope until the oracle corpus exercises them.
    pub(crate) fn emit_const_fault(&mut self, fault: ConstFault, span: SourceRange) {
        // The operator faults anchor at the operator's node, not at the
        // operand the capture folded it onto — see
        // [`Self::unary_fault_extent`]. The remaining variants anchor at the
        // entity their own face already named (an element, a condition, a
        // cyclic reference), so they keep the incoming span.
        let span = match fault {
            ConstFault::TypeBool
            | ConstFault::TypeBoolInt
            | ConstFault::TypeBoolNumString
            | ConstFault::TypeInt
            | ConstFault::TypeNum
            | ConstFault::TypeNumString
            | ConstFault::TypeString
            | ConstFault::ThrowsException
            | ConstFault::ThrowsIdbze => self.unary_fault_extent(span),
            ConstFault::PrimitiveEquality
            | ConstFault::Recursive
            | ConstFault::ForElement
            | ConstFault::NonBoolCondition
            | ConstFault::SpreadExpectedListOrSet
            | ConstFault::SpreadExpectedMap
            | ConstFault::ExpressionInMap => span,
        };
        let code = match fault {
            ConstFault::TypeBool => SemanticCode::ConstEvalTypeBool,
            ConstFault::TypeBoolInt => SemanticCode::ConstEvalTypeBoolInt,
            ConstFault::TypeBoolNumString => SemanticCode::ConstEvalTypeBoolNumString,
            ConstFault::TypeInt => SemanticCode::ConstEvalTypeInt,
            ConstFault::TypeNum => SemanticCode::ConstEvalTypeNum,
            ConstFault::TypeNumString => SemanticCode::ConstEvalTypeNumString,
            ConstFault::TypeString => SemanticCode::ConstEvalTypeString,
            ConstFault::ThrowsException => {
                return self.emit_with_context(
                    SemanticCode::ConstEvalThrowsException,
                    span,
                    "",
                    "",
                    vec![ContextMessage {
                        message: SharedStr::from(
                            "The exception is 'Evaluation of this constant expression \
                             throws an exception.' and occurs here.",
                        ),
                        file: self.file,
                        range: span,
                    }],
                );
            }
            ConstFault::ThrowsIdbze => SemanticCode::ConstEvalThrowsIdbze,
            ConstFault::PrimitiveEquality => SemanticCode::ConstEvalPrimitiveEquality,
            ConstFault::Recursive => SemanticCode::RecursiveCompileTimeConstant,
            ConstFault::ForElement => SemanticCode::ConstEvalForElement,
            ConstFault::NonBoolCondition => SemanticCode::NonBoolCondition,
            ConstFault::SpreadExpectedListOrSet => SemanticCode::ConstSpreadExpectedListOrSet,
            ConstFault::SpreadExpectedMap => SemanticCode::ConstSpreadExpectedMap,
            ConstFault::ExpressionInMap => SemanticCode::ExpressionInMap,
        };
        // One allow-listed failure is one row upstream — the element face's
        // row and the propagated verdict's row are the same diagnostic
        // (`_reportError` on the `InvalidConstant`), so a second emit at the
        // same `(code, range)` collapses (the fault leaf's re-eval lands on
        // the entity the element check already reported).
        let info = code.code_info();
        if self
            .report
            .diagnostics
            .iter()
            .any(|d| d.range == span && d.code == info)
        {
            return;
        }
        self.emit(code, span, "", "");
    }

    /// The declared type a constant value folds to (the oracle infers a
    /// const variable's type from its constant initializer; only the
    /// shapes [`Ty`] can carry fold).
    pub(crate) fn folded_ty(&self, value: &ConstantValue) -> Ty {
        match value {
            ConstantValue::Int(_) => Ty::named(self.members.primitive_ids[0]),
            ConstantValue::Double(_) => Ty::named(self.members.primitive_ids[1]),
            ConstantValue::Bool(_) => Ty::named(self.members.primitive_ids[3]),
            ConstantValue::Str(_) => Ty::named(self.members.primitive_ids[4]),
            ConstantValue::Null => Ty::Null,
            // `const x = C();` has static type C (the const-creation
            // container); collection shapes have no Ty face here.
            ConstantValue::Enum { container, .. } | ConstantValue::Created(container) => {
                Ty::named(*container)
            }
            ConstantValue::Type(_)
            | ConstantValue::Symbol(_)
            | ConstantValue::List(_)
            | ConstantValue::Set(_)
            | ConstantValue::Map(_)
            | ConstantValue::Record { .. }
            | ConstantValue::Instance(_) => Ty::UNKNOWN,
        }
    }

    /// The folded declared type of an in-file const top-level variable
    /// (the type-folding face of #279: `const x = 1;` folds `x` to
    /// `int`).
    pub fn const_top_level_ty(&self, name: StringId) -> Option<Ty> {
        match self.const_scopes.first()?.get(&name)? {
            ConstBinding::Value(v) => Some(self.folded_ty(v)),
            ConstBinding::InProgress | ConstBinding::Failed => None,
        }
    }

    /// An enum constant's window opens (`handle_enum_element` doubles as
    /// the previous element's close); its constructor arguments are the
    /// const context. The arg gate needs the enum to resolve in this file
    /// and carry a declared const constructor.
    pub fn open_enum_const_element(&mut self) {
        if self.enum_element_open {
            self.close_enum_const_element();
        }
        // Every enum value creation is a const creation (the spec requires
        // an enum's generative constructors to be const), so the gate arms
        // on the in-file enum alone — enum constants precede their
        // constructor declarations in source order.
        let gate = self
            .container_context()
            .filter(|(_, kind)| *kind == ContainerKind::Enum)
            .is_some_and(|(name, _)| {
                self.index
                    .get(self.file)
                    .is_some_and(|f| f.container(name).is_some())
            });
        self.const_capture.open_enum_element();
        self.arg_gates.push(gate);
        self.enum_element_open = true;
    }

    pub fn close_enum_const_element(&mut self) {
        if !self.enum_element_open {
            return;
        }
        self.enum_element_open = false;
        self.enum_const_call = None;
        self.arg_gates.pop();
        if self.const_capture.open() {
            self.const_capture.close_enum_element();
        }
    }

    /// An explicit `const C(...)` opened (`begin_const_expression`); the
    /// gate arms once the target reference resolves.
    pub fn open_const_ctor_invocation(&mut self) {
        self.const_capture.open_ctor_invocation();
        self.arg_gates.push(false);
    }

    /// `end_constructor_reference(..., Const)` inside a const-creation
    /// window: the head token names the target; a const constructor this
    /// file declared arms the argument check (foreign and non-const
    /// targets defer — under-report).
    pub fn arm_const_ctor_gate(&mut self, head: Token) {
        // An explicit `const C<T>`-shaped reference: the type-argument check
        // runs for every const-context constructor reference the verifier
        // visits (`visitInstanceCreationExpression.isConst` —
        // `_checkForConstWithTypeParameters`), independent of whether the
        // constructor resolves or is const, so it fires here before the
        // gate below decides the argument check.
        if let Some(name) = self.name_of(head) {
            self.emit_const_type_param_args(name, self.byte_of(head.range.end()));
        }
        let gate = self
            .name_of(head)
            .is_some_and(|name| self.const_ctor_target(name));
        if let Some(slot) = self.arg_gates.last_mut() {
            // OR-in: an enum element's window pre-arms its gate (the
            // constant's own ctor reference names the constant, never the
            // enum's constructor) — a genuine `const C(...)` target only
            // ever widens it.
            *slot |= gate;
        }
    }

    pub fn close_const_ctor_invocation(&mut self) {
        self.arg_gates.pop();
        if self.const_capture.open() {
            self.const_capture.close_ctor_invocation();
        }
    }

    /// The container `name` resolves to from this file (file-local first,
    /// then the library namespace), when it is a class with a declared
    /// `const` constructor or factory. The fact is the resolved
    /// container's own ([`crate::model::Container::has_const_constructor`]),
    /// so a const constructor declared in another library or package
    /// answers exactly like an in-file one (the oracle resolves the
    /// creation's constructor element wherever it is declared). A name the
    /// element model cannot resolve (an SDK face, a missing import) is not
    /// a target — callers keep their under-report posture there.
    pub(crate) fn const_ctor_decl(&self, container: StringId) -> Option<&'a Container> {
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .map(|(decl, _)| decl)
        .filter(|decl| decl.has_const_constructor)
    }

    pub(crate) fn const_ctor_target(&self, container: StringId) -> bool {
        self.const_ctor_decl(container).is_some()
    }

    /// The `(container, constructor-site)` key [`Self::check_ctor_initializers`]
    /// looks the initializer stash up under.
    ///
    /// The stash registers each const constructor under its own DECLARATION
    /// spelling: the unnamed face under the class name, a named one under
    /// its selector. A creation reaches the walk with whatever spelling the
    /// capture folded — `C(...)` with the class, `C.named(...)` with the
    /// selector, and a leading-dot shorthand (`.new(...)` / `.named(...)`)
    /// with the selector ALONE, its head only recoverable from the
    /// declaration's context type (exactly the container the oracle
    /// resolves the shorthand against, `method_invocation_resolver.dart`
    /// `_resolveReceiverTypeLiteralForDotShorthand`). [`Self::ctor_decl_site`]
    /// is the normalizer on the other side: it already answers `new` and
    /// the class name as the unnamed face's spellings, so its site name is
    /// the stash key. A head this pass cannot name leaves the key as given,
    /// which simply misses the stash — the under-report direction.
    fn ctor_init_key(&self, callee: StringId, span: SourceRange) -> (StringId, StringId) {
        let container = if self.dot_shorthand_context(span.offset) {
            match self.shorthand_ctx_ty() {
                Some(Ty::Named { name: context, .. }) => context,
                _ => callee,
            }
        } else {
            // `C.named(` — the written head precedes the selector; a bare
            // `C(` already arrives as the container itself.
            let text = self
                .source
                .get(self.byte_of(span.offset)..self.byte_of(span.end()))
                .unwrap_or("");
            let head = text.split('(').next().unwrap_or("");
            match head.split_once('.') {
                Some((head_text, _)) => self.intern(head_text),
                None => callee,
            }
        };
        let site = self
            .ctor_decl_site(container, callee)
            .map_or(callee, |site| site.name);
        (container, site)
    }

    /// Whether a const-declaration initializer's creation is a valid
    /// constant creation: the container must carry a const constructor
    /// answering the written selector. The selector re-reads the written
    /// source (the capture carries the head only): `C.named(...)` needs a
    /// declared `named` constructor — a miss fails the creation
    /// (`const_with_undefined_constructor`'s initializer row) without the
    /// `const_with_non_const` twin (no constructor exists to be
    /// non-const).
    pub(crate) fn const_creation_valid(&self, container: StringId, span: SourceRange) -> bool {
        // The dot-shorthand creation (`.new(...)`/`.named(...)`) resolves
        // against the context type's constructor — `classify_call`
        // already adopted the valid-creation verdict for it, so the
        // tail-pass re-decision answers the same.
        if self.dot_shorthand_context(span.offset) {
            return true;
        }
        let Some(decl) = self.const_ctor_decl(container) else {
            // A qualified creation arrives named for its SELECTOR
            // (`C.named(` closes with the selector as the callee), which
            // resolves to no container: the written head at the span is
            // the container the verifier answers.
            let text = self
                .source
                .get(self.byte_of(span.offset)..self.byte_of(span.end()))
                .unwrap_or("");
            let head = text.split('(').next().unwrap_or("");
            let Some((head_text, selector)) = head.split_once('.') else {
                return false;
            };
            return resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                self.intern(head_text),
            )
            .map(|(decl, _)| decl)
            .is_some_and(|decl| {
                decl.has_const_constructor && decl.constructors.contains(&self.intern(selector))
            });
        };
        let text = self
            .source
            .get(self.byte_of(span.offset)..self.byte_of(span.end()))
            .unwrap_or("");
        let head = self.strings.resolve(container);
        let selector = text
            .strip_prefix(head)
            .and_then(|rest| rest.strip_prefix('.'))
            .map(|rest| rest.split(['(', ' ', '?', '\n', '\t']).next().unwrap_or(""))
            .unwrap_or("");
        if selector.is_empty() || selector == "new" {
            return true;
        }
        decl.constructors.contains(&self.intern(selector))
    }

    /// Whether the tail-pass's invalid creation resolves to a written
    /// constructor that exists but is not const — the
    /// `const_with_non_const` twin's condition. `container` is the staged
    /// head (`C` for `C(...)`, the receiver `C` for `C.named(...)`'s
    /// selector-staged entries the same — `const_creation_valid`'s
    /// source-parse handles both spellings). A missing selector
    /// (`C.named()` over `const C()`) reports the initializer row alone
    /// (the oracle's `const_with_undefined_constructor` fixture): no
    /// constructor exists to be non-const.
    pub(crate) fn const_non_const_ctor_answered(
        &self,
        container: StringId,
        span: SourceRange,
    ) -> bool {
        // The head + written selector the entry answers: `container` names
        // the head directly when it resolves to a container; a selector
        // name (`C.named(` closes with `named` as the callee) re-reads the
        // written head at the span like `const_creation_valid` does.
        let text = self
            .source
            .get(self.byte_of(span.offset)..self.byte_of(span.end()))
            .unwrap_or("");
        let (head, selector) = if resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .is_some()
        {
            let head = self.strings.resolve(container);
            let selector = text
                .strip_prefix(head)
                .and_then(|rest| rest.strip_prefix('.'))
                .map(|rest| rest.split(['(', ' ', '?', '\n', '\t']).next().unwrap_or(""))
                .unwrap_or("");
            (container, selector)
        } else {
            let head_text = text.split('(').next().unwrap_or("");
            let Some((head_text, selector)) = head_text.split_once('.') else {
                return false;
            };
            (self.intern(head_text), selector)
        };
        let Some((decl, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            head,
        ) else {
            return false;
        };
        if decl.has_const_constructor {
            return false;
        }
        if selector.is_empty() || selector == "new" {
            return decl.has_unnamed_constructor;
        }
        decl.constructors.contains(&self.intern(selector))
    }

    /// The receiver row below the pending selector row (`recv.selector(`):
    /// the selector row must be QUALIFIED (`invokes.last()` indexes it,
    /// the receiver sits immediately below) — `None` for a bare callee,
    /// whose send carries no receiver row.
    fn qualified_recv(&self) -> Option<&Row> {
        let callee = self.invokes.last().and_then(|slot| *slot)?;
        let row = self.rows.get(callee)?;
        if !row.qualified {
            return None;
        }
        self.rows.get(callee.checked_sub(1)?)
    }

    /// Reports `const_with_type_parameters` once per type-parameter type
    /// argument a const creation's head carries — the oracle's
    /// `_checkForConstWithTypeParameters` walks the written `NamedType`'s
    /// `typeArguments` recursively and reports each `TypeParameterElement`
    /// leaf at its own span (upstream Dart SDK source, pkg/analyzer/lib/src/
    /// dart/constant/constant_verifier.dart:591-660), BEFORE the
    /// constructor resolves, so an unresolved (`Missing<T>`) or non-const
    /// (`A<T>()` with no const constructor) head still answers.
    ///
    /// The head face gates: a class container, an import prefix, or an
    /// unresolved name reaches the check; a function or value head
    /// (`f<T>()`, `x.C<T>()`) is the invocation/teardown face, never a
    /// creation — `resolve_bare` reads that distinction directly. The args
    /// are scanned from the WRITTEN `<...>` group's interior (not the row
    /// stack — a resolved type-param leaf pushes no `TypeUse` row), so a
    /// param-spelled leaf (`T`) and a nested param (`List<T>`'s inner `T`)
    /// each report at their own byte span. A `Function<U>` group's OWN
    /// `<U>` declares that function type's parameter (`allowedTypeParameters`
    /// exempts it upstream), so the scan skips the parameter-list bracket
    /// that follows the `Function` keyword and only visits the argument
    /// leaves around it.
    fn emit_const_type_param_args(&mut self, head_name: StringId, head_byte_end: usize) {
        match self.resolve_bare(head_name).0 {
            BareInfo::Container { .. } | BareInfo::Prefix { .. } | BareInfo::Missing { .. } => {}
            BareInfo::Plain { .. } => return,
        }
        // Locate the `<...>` type-argument group: skip the optional `.C`
        // qualified rest of a `prefix.C` head, then the group's `<`.
        let bytes = self.source.as_bytes();
        let mut i = ws_skip(bytes, head_byte_end);
        if bytes.get(i) == Some(&b'.') {
            i = ws_skip(bytes, i + 1);
            i = ident_end(bytes, i);
            i = ws_skip(bytes, i);
        }
        if bytes.get(i) != Some(&b'<') {
            return;
        }
        let group_end = skip_balanced(bytes, i, b'<');
        if group_end == i {
            return;
        }
        let params: Vec<StringId> = self
            .type_params
            .iter()
            .chain(self.pending_type_params.iter())
            .copied()
            .collect();
        if params.is_empty() {
            return;
        }
        let mut spans: Vec<(usize, usize)> = Vec::new();
        param_leaf_spans(
            bytes,
            self.source,
            self.strings,
            i,
            group_end,
            &params,
            &mut spans,
        );
        for (start, end) in spans {
            self.emit(
                SemanticCode::ConstWithTypeParameters,
                SourceRange {
                    offset: self.utf16_of(start),
                    length: self.utf16_of(end) - self.utf16_of(start),
                },
                "",
                "",
            );
        }
    }

    /// The implicit-creation face of [`Self::emit_const_type_param_args`]:
    /// `const x = C<T>(...)` / `const x = C<T>.named(...)` carry no `const`
    /// keyword before the head, so no `ConstructorReferenceContext::Const`
    /// event fires — the argument group's close (this `classify_call`) is
    /// the only hook that sees the whole reference. `name` is the callee
    /// (`C` for the unnamed shape, `named` for the qualified one); the head
    /// resolves from the callee row.
    ///
    /// A bare callee IS the head (`const x = C<T>(` — the send latched `C`
    /// itself); a qualified callee's head sits behind the type-argument
    /// rows (`C<T>.named(` — `invokes.last()` is `named`'s selector row,
    /// `T`'s `TypeUse` row and `C`'s head row stack below it). The scan
    /// stops at the first non-`TypeUse` row, which for the qualified shape
    /// is the class head. `this`/`super`/`new` heads name no creation the
    /// check accepts — the keyword guard excludes them before the
    /// `resolve_bare` read (their `Missing`/`Receiver` faces would
    /// otherwise reach the emit).
    fn emit_const_type_param_call_args(&mut self) {
        // Resolve the head row: the callee itself for a bare reference, or
        // the class-segment `type_uses` entry for a qualified
        // `C<T>.named(` shape (the selector row is `named` — the class head
        // `C` never pushes a row when it resolves to a defined type, so a
        // `rows` walk cannot find it). The bare path keeps the row-based
        // walk for `C<T>(`.
        let head = match self.invokes.last().and_then(|slot| *slot) {
            Some(callee_index) => match self.rows.get(callee_index) {
                Some(callee) if callee.qualified => {
                    let mut index = callee_index;
                    while index > 0
                        && matches!(
                            self.rows.get(index - 1).map(|r| &r.shape),
                            Some(RowShape::TypeUse(..))
                        )
                    {
                        index -= 1;
                    }
                    index
                        .checked_sub(1)
                        .and_then(|i| self.rows.get(i))
                        .map(|row| (row.name, row.range))
                }
                other => other.map(|row| (row.name, row.range)),
            },
            None => {
                // `C<T>.named(` — `creation_ref` carries the class head `C`;
                // its `type_uses` entry is the span the `<` follows.
                self.creation_ref.and_then(|(class, _)| {
                    self.type_uses
                        .iter()
                        .rev()
                        .find(|(_, entry_name)| *entry_name == class)
                        .map(|(range, _)| (class, *range))
                })
            }
        };
        let Some((head_name, head_range)) = head else {
            return;
        };
        let head_end = head_range.end();
        // `this`/`super`/`new` heads carry no class the check accepts.
        match self.strings.resolve(head_name) {
            "this" | "super" | "new" => return,
            _ => {}
        }
        self.emit_const_type_param_args(head_name, self.byte_of(head_end));
    }

    /// The qualified implicit-creation face of
    /// [`Self::emit_const_type_param_args`]: `const x = C<T>.named(...)`
    /// parses as the parser's implicit creation (`parseImplicitCreation
    /// Expression` — `id ('.' id)? typeArguments '.' id '('`), whose
    /// selector rides a constructor-reference event, so the capture never
    /// closes a bare `Ident` callee and [`Self::classify_call`] never sees
    /// the reference. The creation's own argument list opening (this
    /// creation is the innermost one and no list opened since it began) is
    /// the hook; the head is the `type_uses` entry the written `<...>`
    /// group follows, with `.selector(` after the group.
    ///
    /// The const context mirrors the capture's own: an open window that is
    /// neither a non-const candidate (literal-ctor, map-pattern key,
    /// constant pattern) nor a non-const dup literal — a dup window opens
    /// only outside every const window, so a const initializer, a `const`
    /// creation, or an enum element must own the position. Explicit
    /// `const C<T>.named()` never reaches here (`implicit_creation_depth`
    /// stays 0 on that path; [`Self::arm_const_ctor_gate`] owns it), and
    /// the unqualified `C<T>(` parses as an invocation instead.
    fn emit_const_type_param_implicit_named(&mut self) {
        if self.implicit_creation_depth == 0
            || !self.at_creation_level()
            || !self.const_capture.open()
            || self.const_capture.top_is_literal_ctor()
            || self.const_capture.top_is_map_pattern_key()
            || self.const_capture.top_is_constant_pattern()
            || (self.pending_init_names.is_empty()
                && self.const_kw_spans.is_empty()
                && !self.enum_element_open)
        {
            return;
        }
        let bytes = self.source.as_bytes();
        let head = self.type_uses.iter().rev().find_map(|&(range, name)| {
            let head_end = self.byte_of(range.end());
            let open = ws_skip(bytes, head_end);
            let group_end = skip_balanced(bytes, open, b'<');
            if group_end == open {
                return None;
            }
            let dot = ws_skip(bytes, group_end);
            if bytes.get(dot) != Some(&b'.') {
                return None;
            }
            let selector = ws_skip(bytes, dot + 1);
            let selector_end = ident_end(bytes, selector);
            (selector_end > selector && bytes.get(ws_skip(bytes, selector_end)) == Some(&b'('))
                .then_some((name, head_end))
        });
        if let Some((name, head_end)) = head {
            self.emit_const_type_param_args(name, head_end);
        }
    }

    /// The callee classification a closed bare-identifier call resolves
    /// to (`evaluation.dart` `_invalidConstantForMethodInvocation`). The
    /// `&mut` receiver stages the `C.named(` implicit-const creation the
    /// qualified-selector split detects — the tail pass needs the
    /// whole-file const-ctor set before it can verdict.
    pub(crate) fn classify_call(&mut self, name: StringId) -> CallClass {
        // The leading-dot shorthand call (`.new(...)` / `.named(...)` /
        // `.method()`): a bare selector callee reaches here only through
        // the shorthand's selector event — `C.new(...)` parses the
        // constructor-reference path and `x.new(...)` folds its member
        // before the group opens. The oracle resolves the callee against
        // the CONTEXT type (`method_invocation_resolver.dart`
        // `_resolveReceiverTypeLiteralForDotShorthand`: `_resolveElement`'s
        // getter/method lookup first, a static executable taking the
        // method-invocation face, `getNamedConstructor` after it rewriting
        // the node to a `DotShorthandConstructorInvocation`), so the two
        // kinds split on the context type's own member set — see
        // [`Self::shorthand_callee`]. A context this pass cannot resolve
        // (an unindexed SDK head, an unwritten annotation) keeps the
        // valid-creation defer, the under-report direction.
        if self
            .invokes
            .last()
            .and_then(|slot| *slot)
            .and_then(|index| self.rows.get(index))
            .is_some_and(|row| self.dot_shorthand_context(row.range.offset))
        {
            return match self.shorthand_callee(name) {
                // A STATIC METHOD of the context type: the oracle's
                // `visitDotShorthandInvocation` →
                // `_invalidConstantForMethodInvocation`
                // (evaluation.dart:930-932).
                ShorthandCallee::StaticMethod => CallClass::Invocable,
                // A static field/getter: the oracle's
                // `FunctionExpressionInvocation` rewrite, which no
                // `ConstantVisitor` arm answers — the site default.
                ShorthandCallee::PropertyAccessor => CallClass::ValueCall,
                // A constructor of the context type — and every context
                // the model cannot decide. Both keep the creation face;
                // the constructor's initializer walk runs at
                // [`Self::check_ctor_initializers`], which re-reads the
                // same discriminator with the closed argument list in
                // hand.
                ShorthandCallee::Ctor(_) | ShorthandCallee::Defer => CallClass::ConstCtor,
            };
        }
        // A pattern's constant-expression context is NOT a const context: a
        // bare `C(...)` there is an ordinary (non-const) creation, not the
        // implicit `const C(...)` the initializer face folds. The oracle
        // reports the enclosing site's default at the creation
        // (`{A(): 0}` → `non_constant_map_pattern_key`, `case > A()` →
        // `non_constant_relational_pattern_expression`), which is exactly
        // the [`CallClass::ValueCall`] posture.
        // Only an IMPLICIT creation is affected, and only a MAP-PATTERN
        // KEY's window: an explicit `const C()` is the same constant it is
        // anywhere else, and a CONSTANT pattern's `case C()` is a variable
        // pattern rather than a constant expression (its own face owns the
        // shape). `const_kw_spans` being non-empty marks the explicit
        // prefix.
        let in_pattern_expression = self.const_capture.top_is_map_pattern_key();
        // The implicit-creation type-argument check (`const x = C<T>(`) —
        // the creation carries no `const` keyword so no
        // `ConstructorReferenceContext::Const` event armed the explicit
        // hook; the argument group's close is the only point that sees the
        // whole reference. A map-pattern key's window is NOT a const
        // context (the enclosing site reports `non_constant_map_pattern_key`
        // at the creation, never the type-argument face), so the emit is
        // gated on the same `in_pattern_expression` read the verdict below
        // uses.
        if !in_pattern_expression {
            self.emit_const_type_param_call_args();
        }
        if self.const_ctor_target(name) {
            return if in_pattern_expression {
                CallClass::ValueCall
            } else {
                CallClass::ConstCtor
            };
        }
        // `C.named(` over an in-file class container: the implicit
        // `const C.named(...)` creation when a declared CONST constructor
        // answers the selector — the oracle resolves the creation and
        // validates each argument against it
        // (`_validateConstantArguments`). The arm precedes the selector's
        // own bare-miss short-circuit: a private selector (`C._(`) names
        // nothing in the bare namespace yet still resolves through the
        // receiver. A selector no declared constructor answers falls
        // through to the unknown-callee face (the
        // `const_with_undefined_constructor` world).
        if let Some(recv) = self.qualified_recv()
            && let RowShape::Bare(BareInfo::Container {
                name: container,
                kind: ContainerKind::Class,
            }) = &recv.shape
            && self
                .const_ctor_decl(*container)
                .is_some_and(|decl| decl.constructors.contains(&name))
        {
            return CallClass::ConstCtor;
        }
        if matches!(self.resolve_bare(name).0, BareInfo::Missing { .. }) {
            return CallClass::Unknown;
        }
        // A qualified selector (`recv.name(`) reaches here as a bare
        // `Ident` callee — `dot_access` folds `recv.name` into `Member`
        // only after the argument list closes. `invokes.last()` indexes the
        // selector row and the receiver row sits immediately below it; the
        // receiver's shape decides the face: a name resolving to a type is
        // the implicit `const C.named(...)` creation (staged for the tail
        // pass beside the unnamed-`C(...)` arm), a value/creation result is
        // the `constEvalMethodInvocation` fault.
        if let Some(recv) = self.qualified_recv() {
            match &recv.shape {
                RowShape::Bare(BareInfo::Plain { .. }) => {
                    let (info, target) = self.resolve_bare(recv.name);
                    let decl_type = matches!(target, RefTarget::Decl { .. })
                        && !matches!(info, BareInfo::Missing { .. })
                        && self.type_resolution(recv.name).0 == TypeResolution::Type;
                    // `C.named(` — the receiver's resolution decides: a
                    // type-valued decl is the implicit const creation, a
                    // value-typed read (a local, a member, a const var) is
                    // the method-invocation fault.
                    let is_const_ctor = decl_type && {
                        let container = self.strings.resolve(recv.name);
                        let selector = self.strings.resolve(name);
                        CORE_CONST_CTOR
                            .iter()
                            .any(|(c, sels)| *c == container && sels.iter().any(|s| *s == selector))
                    };
                    if decl_type && !is_const_ctor {
                        // The `const_with_non_const` face fires when the
                        // container's constructor exists but is not const
                        // (an unindexed `DateTime` answers the under-report
                        // side the same way — `resolve_container` cannot
                        // reach `<dart:core>`'s decl, `const_ctor_target`
                        // stays false, the oracle's row is the creation's).
                        let span = self.creation_span(recv.range);
                        if let Some(init) = self.pending_init_names.last().copied() {
                            self.open_const_creations.push((
                                init,
                                span,
                                ConstCreationEntry::Creation {
                                    container: recv.name,
                                },
                            ));
                        }
                        return CallClass::Unknown;
                    }
                    if matches!(target, RefTarget::Local | RefTarget::Opaque) {
                        // A bare-identifier call on a LOCAL / FORMAL: the
                        // resolver rewrites it to a
                        // `FunctionExpressionInvocation` (see
                        // [`CallClass::ValueCall`]).
                        return CallClass::ValueCall;
                    }
                    if matches!(target, RefTarget::This | RefTarget::Member { .. })
                        || (matches!(target, RefTarget::Decl { .. })
                            && self.type_resolution(recv.name).0 == TypeResolution::NonType)
                    {
                        return CallClass::Invocable;
                    }
                    return CallClass::Unknown;
                }
                // A value or folded-creation result receiver — the
                // method-invocation fault (`DateTime.now().toString()`,
                // `C().m()`, `this.m()`). The container-family receivers
                // (`Bare(Container)`, `Receiver::Container`, prefixed) stay
                // out — `named_ctor`/`resolve_container` below own the
                // `C.named(` creation face there.
                RowShape::Member { .. }
                | RowShape::Value
                | RowShape::Record(_)
                | RowShape::Receiver(
                    Receiver::Unknown
                    | Receiver::Typed { .. }
                    | Receiver::Created { .. }
                    | Receiver::This { .. }
                    | Receiver::Super { .. },
                ) => {
                    return CallClass::Invocable;
                }
                _ => {}
            }
        }
        // A DECLARED LOCAL FUNCTION is an `ExecutableElement` callee: the
        // constant evaluator answers `constEvalMethodInvocation` at the
        // call (`const i = x()` over `int x() => 1;`), never the
        // `FunctionExpressionInvocation` rewrite a local VARIABLE or a
        // function-typed formal takes. The tear-off capture rides the
        // same enclosing frame as the `locals` entry (`local_fn_sigs`,
        // #2842), so the pair decides the face under one scope walk.
        if self
            .frames
            .iter()
            .any(|f| f.locals.contains(&name) && f.local_fn_sigs.contains_key(&name))
        {
            return CallClass::Invocable;
        }
        if self.frames.iter().any(|f| f.locals.contains(&name))
            || self.const_scopes.iter().any(|s| s.contains_key(&name))
        {
            // Same rewrite face as the receiver-shaped local call above.
            return CallClass::ValueCall;
        }
        match self.top_level_decl(name).map(|d| d.kind) {
            // A top-level FUNCTION is a real method invocation
            // (`const_eval_method_invocation`); a top-level VARIABLE is a
            // getter read followed by a call, which the resolver rewrites to
            // a `FunctionExpressionInvocation` ([`CallClass::ValueCall`]).
            Some(MicroKind::TopLevelFn) => CallClass::Invocable,
            Some(MicroKind::TopLevelVar) => CallClass::ValueCall,
            _ => CallClass::Unknown,
        }
    }

    /// Whether the leading-dot shorthand whose name sits at `span`
    /// resolved through the declaration shorthand pass: a hit upgrades the
    /// row's `Bare` shape to `Plain { ty }` with a non-`UNKNOWN` `ty`
    /// (`verdict_shorthand_at`), so the const root defers like any
    /// constant; a miss keeps `ty == UNKNOWN` (its own
    /// `dot_shorthand_undefined_member`/`missing_context` row already
    /// reports the member face, and the const root still owes the oracle's
    /// `invalidConstant` at the whole shorthand).
    fn shorthand_resolved_at(&self, span: SourceRange) -> bool {
        self.rows
            .iter()
            .find(|row| row.range.offset <= span.offset && row.range.end() >= span.end())
            .is_some_and(|row| row.ty != Ty::UNKNOWN)
    }

    /// The whole leading-dot shorthand span whose name sits at `span` —
    /// the `.` through the selector's end (the oracle anchors a failed
    /// shorthand's `invalidConstant` at the node, not the name).
    fn shorthand_whole_span(&self, span: SourceRange) -> SourceRange {
        let toks = self.tokens.as_slice();
        let idx = toks.partition_point(|t| t.utf16_offset < span.offset);
        let Some(dot) = idx.checked_sub(1).and_then(|i| toks.get(i)) else {
            return span;
        };
        if self.source.as_bytes().get(self.byte_of(dot.utf16_offset)) != Some(&b'.') {
            return span;
        }
        SourceRange {
            offset: dot.utf16_offset,
            length: span.end() - dot.utf16_offset,
        }
    }

    /// The leading-dot shorthand's callee face: the context type's member
    /// set decides whether the selector names a constructor (the oracle's
    /// `DotShorthandConstructorInvocation`) or a static method (the
    /// `DotShorthandInvocation` method face).
    ///
    /// Upstream's order is member-first: `method_invocation_resolver.dart`
    /// `_resolveReceiverTypeLiteralForDotShorthand` runs `_resolveElement`
    /// (setter, then getter, then method, :484-500) and takes the
    /// method-invocation face for a static `InternalExecutableElement`
    /// (:1287-1304); only a miss falls through to
    /// `receiver.getNamedConstructor(name)`'s rewrite to a
    /// `DotShorthandConstructorInvocation` (:1310-1330). The constructor
    /// probe is therefore only reached when the member walk found nothing.
    ///
    /// The context type is the declaration group's written annotation
    /// ([`Self::shorthand_ctx_ty`] — alias-folded, `FutureOr`-unwrapped
    /// like the oracle's `futureOrBase`). [`ShorthandCallee::Defer`] covers
    /// every context this pass cannot decide — no annotation, an
    /// unresolved or non-interface head, a member walk that broke on a
    /// foreign supertype — and the caller keeps the creation face there
    /// (the under-report direction: a method shorthand in such a context
    /// stays silent, exactly the pre-#3956 posture).
    fn shorthand_callee(&self, name: StringId) -> ShorthandCallee {
        let Some(Ty::Named { name: context, .. }) = self.shorthand_ctx_ty() else {
            return ShorthandCallee::Defer;
        };
        // The member face first: a STATIC member of the context type
        // answers the shorthand. Upstream splits it by element kind —
        // `_resolveElement`'s getter/method lookup answers a
        // `InternalExecutableElement`, and
        // `_resolveReceiverTypeLiteralForDotShorthand` sends a
        // `InternalPropertyAccessorElement` (a field or getter) through
        // `_rewriteAsFunctionExpressionInvocation` while a method takes
        // the invocation face (:1287-1304). The rewrite has no
        // `ConstantVisitor` arm, so the call is the site default
        // ([`CallClass::ValueCall`]), never the method fault. A
        // non-static member never answers the shorthand (there is no
        // receiver) — it falls through to the constructor probe, its own
        // `dot_shorthand_undefined_member` row already reported.
        match self.walk_members(context, name) {
            Walk::Found(entry) if entry.is_static => {
                return match entry.kind {
                    MemberKind::Method => ShorthandCallee::StaticMethod,
                    MemberKind::Field | MemberKind::Getter | MemberKind::Setter => {
                        ShorthandCallee::PropertyAccessor
                    }
                };
            }
            // A broken climb leaves the inherited half of the member set
            // unknown — neither arm can claim the selector.
            Walk::ChainBroken => return ShorthandCallee::Defer,
            Walk::Found(_) | Walk::Missing => {}
        }
        // The constructor face: the same decl-site/model union
        // [`Self::shorthand_member_ty`] uses, so a context head declared
        // in a sibling unit resolves through the model.
        let ctor_hit = self.ctor_decl_site(context, name).is_some()
            || resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                context,
            )
            .is_some_and(|(decl, _)| self.ctor_face_exists(decl, context, name));
        if ctor_hit {
            return ShorthandCallee::Ctor(context);
        }
        // Neither arm answers: the shorthand's own
        // `dot_shorthand_undefined_member` face reported it, and the
        // initializer's `const_initialized_with_non_constant_value` twin
        // rides the failed-verdict path — never the creation face, which
        // would walk a nonexistent constructor's initializers.
        ShorthandCallee::Defer
    }

    /// The shorthand resolution's context type — the declaration group's
    /// armed annotation folded through the alias chain and the
    /// `FutureOr<S>` base (upstream `_resolveDotShorthand` unwraps both:
    /// `typedef E2 = E; E2 e = .a` resolves `.a` against `E`, and the
    /// static namespace `S` denotes is the one `FutureOr<S>` denotes).
    /// `None` when the group carries no decidable context: unwritten
    /// (`var x = .a`), unresolvable (`pending_decl_ty_unresolved` — the
    /// oracle's `InvalidType`, which swallows the member verdict), or a
    /// non-interface shape the shorthand's lookup never climbs.
    fn shorthand_ctx_ty(&self) -> Option<Ty> {
        if self.pending_decl_ty_unresolved {
            return None;
        }
        let ty = self.future_or_base(self.fold_alias_ty(self.pending_decl_ty?));
        (ty != Ty::UNKNOWN).then_some(ty)
    }

    /// Whether the name expression at `offset` is a leading-dot shorthand
    /// (`.name`/`.new` — upstream `_isDotShorthand`, parser_impl.dart:6499):
    /// the byte before the name's span is a `.` no receiver expression
    /// precedes. `recv.name` never reaches this probe's callers — the
    /// member fold merges it — so the qualified shapes left are the
    /// keyword receivers (`this.new`, `super.new`: constructor-initializer
    /// redirects) and the cascade section (`x..name`, whose selector binds
    /// the cascade target). Every expression-opener, operator, `?`/`=>`
    /// byte, and the const-context keywords before the dot is the
    /// shorthand's context (`= .new`, `const .name`, `return .name`,
    /// `foo(.name)`).
    pub(crate) fn dot_shorthand_context(&self, offset: u32) -> bool {
        let bytes = self.source.as_bytes();
        // `offset` is a UTF-16 position — walk the source in byte space.
        let mut i = self.byte_of(offset);
        // The shorthand's `.` precedes the member with at most trivia
        // between (the parser's `_isDotShorthand` decides on the token
        // pair, so `. name` is still the shorthand shape).
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        if i == 0 || bytes[i - 1] != b'.' {
            return false;
        }
        i -= 1;
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        let Some(byte) = i.checked_sub(1).and_then(|j| bytes.get(j)).copied() else {
            return true;
        };
        match byte {
            // A receiver-end byte: `x.name` (handled below), `f().name`,
            // `x!.name`, `x..name` (cascade target), `'s'.name`.
            b')' | b']' | b'}' | b'\'' | b'"' | b'!' | b'.' => false,
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => {
                // Read the identifier/keyword token back: `this`/`super`
                // are keyword receivers (the redirect-invocation shapes
                // reach this probe's call-classification caller as bare
                // `new` heads); the literals spell receiver values; every
                // other keyword (`const`, `return`, `case`, `await`,
                // `throw`, `else`, `default`, `in`, `new`) opens the
                // shorthand's context. A plain identifier is a receiver.
                let mut start = i - 1;
                while start > 0 {
                    let Some(c) = bytes.get(start - 1).copied() else {
                        break;
                    };
                    if !(c.is_ascii_alphanumeric() || c == b'_' || c == b'$') {
                        break;
                    }
                    start -= 1;
                }
                matches!(
                    &self.source[start..i],
                    "const"
                        | "new"
                        | "return"
                        | "case"
                        | "yield"
                        | "await"
                        | "throw"
                        | "else"
                        | "default"
                        | "in"
                        | "assert"
                )
            }
            // Every other byte — openers `(`/`[`/`{`, operators, `=`, `,`,
            // `:`, `;`, `?`, `>` (of `=>`/`>=`/shift-writeback), `<`, `~`,
            // `#`, `@` — opens an expression position: the shorthand's
            // context.
            _ => true,
        }
    }

    /// The decl-root dot shorthand whose declaration writes no context type
    /// (`const a = .new();`, `const a = .id;`). The capture folds a
    /// shorthand's selector to a bare name and drops the `.`, so the root
    /// node's span starts AT the selector token with only the dot before it
    /// — [`Self::dot_shorthand_context`] recognises exactly that shape, and
    /// requires the shorthand to be the initializer's first token (the first
    /// node evaluated, so the failure anchors there). A nested or later
    /// shorthand keeps the under-report the capture folds it to.
    ///
    /// Upstream resolves the shorthand against the declaration's context
    /// type (`visitDotShorthandConstructorInvocation2` /
    /// `visitDotShorthandNameExpression`, evaluation.dart:880-916); with no
    /// written type the context is `dynamic`, so the evaluation is
    /// known-invalid at the shorthand. The invocation face reports
    /// `dot_shorthand_undefined_member` at the selector, the name-expression
    /// face `dot_shorthand_missing_context` at the whole shorthand, and both
    /// pair the site default at the whole shorthand span.
    fn report_root_shorthand(&mut self, root: &crate::const_eval::Node) {
        if !self.dot_shorthand_context(root.span.offset)
            || self.pending_decl_ty.is_some()
            || self.pending_decl_ty_unresolved
        {
            return;
        }
        let Some((whole, selector, call)) = self.shorthand_face(root.span) else {
            return;
        };
        if call {
            self.emit_shorthand_undefined_member(selector, "dynamic");
        } else {
            // The declaration-initializer shorthand pass may already have
            // reported this row (`emit_shorthand_missing_context` dedups on
            // `(code, range)`); a second emit here would twin it.
            let info = SemanticCode::DotShorthandMissingContext.code_info();
            if !self
                .report
                .diagnostics
                .iter()
                .any(|d| d.range == whole && d.code == info)
            {
                self.emit(SemanticCode::DotShorthandMissingContext, whole, "", "");
            }
        }
        let info = SemanticCode::ConstInitializedWithNonConstantValue.code_info();
        if !self
            .report
            .diagnostics
            .iter()
            .any(|d| d.range == whole && d.code == info)
        {
            self.emit(
                SemanticCode::ConstInitializedWithNonConstantValue,
                whole,
                "",
                "",
            );
        }
    }

    /// The `dot_shorthand_undefined_member` emit surface every shorthand
    /// face shares (#3433): the const evaluation's root failure
    /// ([`Self::report_root_shorthand`]) and the declaration-initializer
    /// pass ([`Engine::resolve_decl_shorthands`]) both land the row at the
    /// shorthand's selector span — `memberName` upstream — with the context
    /// type's display as the second slot (`'_'` for a context-free
    /// invocation, `dynamic` for the const root's known-invalid context).
    /// The message shape follows the selector's own spelling — a selector
    /// carrying an argument list renders the invocation face's "static
    /// method or constructor" (`dotShorthandUndefinedInvocation`,
    /// analyzer `messages.yaml` :4981), a read selector the getter face's
    /// "static getter" (:4927). Emitting twice at one selector is one
    /// verdict — the diagnostic set dedups on the `(code, range)` pair so
    /// a stale-window resweep stays idempotent.
    pub(crate) fn emit_shorthand_undefined_member(&mut self, selector: SourceRange, context: &str) {
        if self
            .report
            .diagnostics
            .iter()
            .any(|d| d.range == selector && d.code.name == "dot_shorthand_undefined_member")
        {
            return;
        }
        let selector_text = self.source_slice(selector).to_string();
        let code = if self.shorthand_selector_invokes(selector) {
            SemanticCode::DotShorthandUndefinedMemberInvocation
        } else {
            SemanticCode::DotShorthandUndefinedMember
        };
        self.emit(code, selector, &selector_text, context);
    }

    /// Whether the selector's written spelling carries an argument list —
    /// the invocation/read message split of the shared
    /// `dot_shorthand_undefined_member` code (#3603). The probe walks the
    /// identifier, one optional balanced type-argument group, then the
    /// argument list's `(` — the same lexical shape [`Self::shorthand_face`]
    /// uses to classify the const root's call face. The open byte must sit
    /// exactly at the probe point (no trivia skip), so a read selector can
    /// never read as a call.
    fn shorthand_selector_invokes(&self, selector: SourceRange) -> bool {
        let bytes = self.source.as_bytes();
        let mut token = self.byte_of(selector.offset);
        while let Some(&b) = bytes.get(token)
            && (b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
        {
            token += 1;
        }
        let typed = skip_balanced(bytes, token, b'<');
        skip_balanced(bytes, typed, b'(') != typed
    }

    /// `(whole shorthand span, selector span, is-a-call)` for a leading-dot
    /// shorthand whose capture span opens at the selector token: the span
    /// walks back over trivia to the dot and forward through the selector,
    /// its type arguments, and one argument list when a call follows.
    fn shorthand_face(&self, span: SourceRange) -> Option<(SourceRange, SourceRange, bool)> {
        let bytes = self.source.as_bytes();
        let mut dot = self.byte_of(span.offset);
        while dot > 0 && bytes[dot - 1].is_ascii_whitespace() {
            dot -= 1;
        }
        if dot == 0 || bytes[dot - 1] != b'.' {
            return None;
        }
        let mut token = self.byte_of(span.offset);
        while let Some(&b) = bytes.get(token)
            && (b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
        {
            token += 1;
        }
        let typed = skip_balanced(bytes, token, b'<');
        let closed = skip_balanced(bytes, typed, b'(');
        let call = closed != typed;
        let end = if call { closed } else { typed };
        let start = self.utf16_of(dot - 1);
        Some((
            SourceRange {
                offset: start,
                length: self.utf16_of(end).saturating_sub(start),
            },
            SourceRange {
                offset: span.offset,
                length: self.utf16_of(token).saturating_sub(span.offset),
            },
            call,
        ))
    }

    /// The `C.named(...)` creation span — the receiver's head offset through
    /// the call's closing paren. `arg_levels.last()` still holds this call's
    /// arguments at the `classify_call` point (the close drains it after),
    /// so the `)` sits one byte past the last argument's end; a zero-arg
    /// `()` sits two bytes past the callee's end. The tail pass re-reads the
    /// written selector from this span (`const_creation_valid`), so it must
    /// cover `C.named` exactly — never the arguments.
    fn creation_span(&self, recv: SourceRange) -> SourceRange {
        let callee_end = self
            .invokes
            .last()
            .and_then(|slot| *slot)
            .and_then(|index| self.rows.get(index))
            .map(|row| row.range.end())
            .unwrap_or(recv.end());
        let close = self
            .arg_levels
            .last()
            .and_then(|level| level.args.last())
            .map(|arg| arg.3.end() + 1)
            .unwrap_or(callee_end + 2);
        SourceRange {
            offset: recv.offset,
            length: close.saturating_sub(recv.offset),
        }
    }

    /// Arguments of a closed const creation: each evaluates against the
    /// `const_with_non_constant_argument` default
    /// (`constant_verifier.dart` `_validateConstantArguments`). A
    /// non-constant argument also fails the enclosing const declaration's
    /// initializer (`const_initialized_with_non_constant_value` at the
    /// same argument) — the entry defers to the tail pass beside the
    /// creation faces.
    pub fn check_const_args(&mut self, gate: bool, args: &[crate::const_eval::Node]) {
        if !gate {
            return;
        }
        // The oracle evaluates the creation as one constant and reports only
        // its first invalid argument (probed `const C(nc, nc)` → one row at
        // the first `nc`); an unresolved argument continues with a
        // placeholder, so later arguments still evaluate.
        for arg in args {
            // A named-argument value (the written `name:` label before
            // it): the oracle's creation evaluation reports its failures
            // through the value's own evaluation — an unresolved leaf
            // collects `invalid_constant` (value.dart `genericError`),
            // not the positional arm's argument face.
            let named = self.named_argument_value(arg.span.offset);
            // `genericError`'s parent rule (upstream Dart SDK source,
            // pkg/analyzer/lib/src/dart/constant/value.dart): the failing
            // node's parent — skipping NamedArgument — must be the const
            // ConstructorInvocation's ArgumentList to take
            // `constWithNonConstantArgument`; every deeper or labeled
            // position collects `invalidConstant`. A direct positional
            // argument is exactly a bare identifier the evaluation leaf
            // still spans whole — parens, member selectors, unary/binary
            // operators and collection literals all nest under the arg's
            // own node.
            let verdict = eval(arg, self);
            match verdict {
                Verdict::Value(_) | Verdict::Constant => {}
                Verdict::NotConstant(span) => {
                    let code = self.const_arg_code(arg, span, named);
                    if !self.const_arg_row_at(span) {
                        self.emit(code, span, "", "");
                    }
                    if let Some(init) = self.pending_init_names.last().copied() {
                        self.open_const_creations.push((
                            init,
                            span,
                            ConstCreationEntry::ArgFailure,
                        ));
                    }
                    return;
                }
                // An unresolved argument reports its own face but leaves the
                // declaration's verdict silent (`B(x)`'s undefined `x`: the
                // oracle's evaluation is unknown, not known-invalid — the
                // ArgFailure staging stays out, #2138). A leading-dot
                // shorthand resolves against the context type upstream —
                // under-report beside the declaration-root arm.
                Verdict::Unresolved(span) => {
                    if self.dot_shorthand_context(span.offset) || self.const_arg_row_at(span) {
                        continue;
                    }
                    let code = self.const_arg_code(arg, span, named);
                    self.emit(code, span, "", "");
                }
                Verdict::Fault(fault, span) => {
                    self.emit_const_fault(fault, span);
                    return;
                }
            }
        }
    }

    /// The `genericError` parent rule (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/constant/value.dart): a failing argument
    /// takes `constWithNonConstantArgument` only when its node is a direct
    /// child of the const constructor's `ArgumentList` — a bare identifier
    /// whose own span still covers the whole argument. Named values,
    /// parenthesized operands, member selectors (`missing.name`,
    /// `DriftSqlType.dateTime`) and every composite (`missing + 1`,
    /// `[missing]`, `!missing`, `missing?.name`) nest the leaf inside the
    /// argument's own expression and collect `invalidConstant` at the
    /// leaf instead. Enum-constant arguments run the
    /// `_validateConstantArguments` path instead
    /// (constant_verifier.dart): `constWithNonConstantArgument` is the
    /// *default* code for every non-allow-listed leaf there, whatever the
    /// argument's shape.
    fn const_arg_code(
        &self,
        arg: &crate::const_eval::Node,
        span: SourceRange,
        named: bool,
    ) -> SemanticCode {
        if self.const_capture.top_is_enum_element()
            || (!named
                && matches!(arg.expr, Expr::Ident(_))
                && arg.span == span
                && !self.paren_wrapped_arg(span))
        {
            SemanticCode::ConstWithNonConstantArgument
        } else {
            SemanticCode::InvalidConstant
        }
    }

    /// Whether the leaf at `span` sits inside an extra pair of written
    /// parens the node span no longer shows (`C((missing))`): the byte
    /// before the leaf is `(` whose own preceding byte cannot be a
    /// receiver-end (identifier, `>`, `.`, `)`, `]`, literal quote, `!`)
    /// — an arg list's or call's open — so the `(` is a wrapping paren,
    /// the oracle's `ParenthesizedExpression` arm that takes
    /// `invalidConstant`.
    fn paren_wrapped_arg(&self, span: SourceRange) -> bool {
        let bytes = self.source.as_bytes();
        let mut i = self.byte_of(span.offset);
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        if i == 0 || bytes[i - 1] != b'(' {
            return false;
        }
        i -= 1;
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        if i == 0 {
            return true;
        }
        !matches!(
            bytes[i - 1],
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$' | b'.' | b'>' | b')' | b']'
                | b'\'' | b'"' | b'!' | b'?'
        )
    }

    /// Whether the argument value starting at `offset` carries a written
    /// `name:` label: the previous non-space character is a colon whose
    /// left side is an identifier character.
    fn named_argument_value(&self, offset: u32) -> bool {
        let bytes = self.source.as_bytes();
        let mut i = self.byte_of(offset);
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        if i < 2 || bytes[i - 1] != b':' {
            return false;
        }
        let before = bytes[i - 2];
        before.is_ascii_alphanumeric() || before == b'_' || before == b'$'
    }

    /// Whether the span's last character is `)` — the invocation shape
    /// (`Missing()`) the set/map element suppression keys on.
    fn span_ends_call(&self, span: SourceRange) -> bool {
        let end = self.byte_of(span.end());
        end > 0 && self.source.as_bytes()[end - 1] == b')'
    }

    /// Whether the annotated declaration following the metadata's `)` is a
    /// type declaration (`class`/`enum`/`mixin`/`extension`/`typedef`,
    /// behind any written modifiers): only those hosts' set/map literals
    /// lose the call-element face (the probed host split — the oracle's
    /// evaluation path owns the report on type declarations, the element
    /// checks on every other host). Earlier metadata annotations and
    /// comments are skipped.
    fn annotation_host_is_type_declaration(&self, close: SourceRange) -> bool {
        let bytes = self.source.as_bytes();
        let mut i = self.byte_of(close.end());
        loop {
            while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b';') {
                i += 1;
            }
            if bytes[i..].starts_with(b"//") {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if bytes[i..].starts_with(b"/*") {
                i += 2;
                while i + 1 < bytes.len() && !bytes[i..].starts_with(b"*/") {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
            if bytes.get(i) == Some(&b'@') {
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric()
                        || bytes[i] == b'_'
                        || bytes[i] == b'$'
                        || bytes[i] == b'.')
                {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'<') {
                    let mut depth = 0usize;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'<' => depth += 1,
                            b'>' => {
                                depth -= 1;
                                if depth == 0 {
                                    i += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                if bytes.get(i) == Some(&b'(') {
                    let mut depth = 0usize;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'(' => depth += 1,
                            b')' => {
                                depth -= 1;
                                if depth == 0 {
                                    i += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                continue;
            }
            break;
        }
        // The declaration keyword, behind any written class modifiers.
        for _ in 0..4 {
            let mut end = i;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] == b'$')
            {
                end += 1;
            }
            match &bytes[i..end] {
                b"class" | b"enum" | b"mixin" | b"extension" | b"typedef" => return true,
                b"abstract" | b"base" | b"final" | b"sealed" | b"interface" | b"augment" => {
                    i = end;
                }
                _ => return false,
            }
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
        }
        false
    }

    /// Takes the capture stack's top node into the const-constructor
    /// initializer stash (the field initializer's RHS, saved before the
    /// assignment merge folds it away).
    pub(crate) fn take_ctor_init_rhs(&mut self) {
        if let Some(node) = self.const_capture.take_top_node() {
            self.ctor_init_pending.push(node);
        }
    }

    /// The creation-time initializer walk (#1351): an in-file const
    /// constructor's captured field initializers evaluate with the
    /// creation's arguments bound to the formals (upstream Dart SDK
    /// source, `ConstantEvaluator` inlines the constructor's
    /// initializers under the invocation's argument values — a
    /// `dynamic`-bound formal's property access fails the `assertString`
    /// face, reporting `const_eval_type_string` at the creation node).
    /// Only a fault reports; the folded value is not tracked (the
    /// creation stays its container's opaque value).
    pub(crate) fn check_ctor_initializers(
        &mut self,
        container: StringId,
        args: &[crate::const_eval::Node],
        span: SourceRange,
    ) {
        // The `(container, constructor-site)` key the stash registers under:
        // a plain `C(...)` arrives with the class itself as the callee, a
        // `C.named(...)` and a leading-dot shorthand (`.new(...)` /
        // `.named(...)`) arrive with the SELECTOR alone — the head is
        // re-read from the written creation (see
        // [`Self::ctor_init_key`]).
        let (container, site) = self.ctor_init_key(container, span);
        let Some(inits) = self.ctor_inits.get(&(container, site)) else {
            return;
        };
        if inits.is_empty() {
            return;
        }
        // The container's type parameters substitute with the creation's
        // written type arguments (upstream evaluates the initializers
        // under `definingType`'s instantiated element —
        // `SubstitutedConstructorElementImpl.from2`, evaluation.dart
        // :3228 — so `x as T` inside `C<int>`'s init sees `int`). The
        // `call_type_args` entry keys on the callee head offset, which
        // sits inside the creation span (the head is the first
        // identifier past the `const` keyword); only an exact offset
        // match binds — a nested expression's group never substitutes.
        let type_env: Option<FxHashMap<StringId, Ty>> = (|| {
            let decl = self.index.get(self.file)?.container(container)?;
            if decl.type_param_names.is_empty() {
                return None;
            }
            let bytes = self.source.as_bytes();
            let mut p = self.byte_of(span.offset);
            while matches!(bytes.get(p), Some(b) if b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
            {
                p += 1;
            }
            while matches!(bytes.get(p), Some(b) if b.is_ascii_whitespace()) {
                p += 1;
            }
            let head = self.utf16_of(p);
            let entry = self.call_type_args.iter().find(|(at, _)| *at == head)?;
            let env: FxHashMap<StringId, Ty> = decl
                .type_param_names
                .iter()
                .copied()
                .zip(entry.1.tys.iter().copied())
                .collect();
            (!env.is_empty()).then_some(env)
        })();
        let owned;
        let inits = if let Some(env) = &type_env {
            owned = inits
                .iter()
                .map(|node| self.subst_init_tys(node, env))
                .collect::<Vec<_>>();
            &owned
        } else {
            inits
        };
        // Positional formals bind the arguments in order (the capture
        // drops named-argument labels — a named-only call binds its
        // values positionally, which only ever moves the walk toward
        // silence, the under-report direction).
        let mut env: FxHashMap<StringId, CtorBinding> = FxHashMap::default();
        let formals = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.ctor_formals.get(&site));
        if let Some(formals) = formals {
            for (formal, arg) in formals.positional.iter().zip(args) {
                let binding = match eval(arg, self) {
                    Verdict::Value(v) => CtorBinding::Value(v),
                    Verdict::Constant => CtorBinding::Opaque,
                    Verdict::NotConstant(_) | Verdict::Unresolved(_) | Verdict::Fault(..) => {
                        CtorBinding::Invalid
                    }
                };
                env.insert(formal.name, binding);
            }
        }
        let fault = {
            let scope = CtorScope {
                engine: self,
                env: &env,
            };
            let mut fault = None;
            for init in inits {
                // The property-access operator requires a String receiver
                // in a constant expression: a `dynamic` operand (an
                // invalid formal, an unresolved name) faults at the
                // creation node.
                if let Expr::Member { recv, .. } = &init.expr
                    && let Verdict::NotConstant(_) = eval(recv, &scope)
                {
                    fault = Some(ConstFault::TypeString);
                    break;
                }
                if let Verdict::Fault(found, _) = eval(init, &scope) {
                    fault = Some(found);
                    break;
                }
            }
            fault
        };
        if let Some(fault) = fault {
            self.emit_const_fault(fault, span);
        }
    }

    /// Substitutes the constructor's type parameters inside a captured
    /// initializer tree's `as`/`is` target types with the creation's
    /// written type arguments (the `T` of `x as T` under `const C<int>(x)`
    /// — upstream instantiates the constructor under the invocation's
    /// `typeArguments` before evaluating its initializers). Only `Named`
    /// heads bound in `env` rewrite — a target spelling a real container
    /// or an unbound parameter stays as written (`castToType` defers on
    /// it anyway). Argument lists substitute elementwise (`x as List<T>`
    /// under `C<int>` sees `List<int>`), interning through the shared
    /// pool; `Record`/`Function` shapes carry no substitutable face this
    /// deep into the parse-time `Ty`.
    fn subst_init_tys(
        &self,
        node: &crate::const_eval::Node,
        env: &FxHashMap<StringId, Ty>,
    ) -> crate::const_eval::Node {
        let expr = match &node.expr {
            Expr::Unary { op, operand } => Expr::Unary {
                op: match op {
                    // Only the type-carrying operators substitute — the
                    // creation's env answers a `T`/`List<T>` target with
                    // the written arguments (`x as T` under `C<int>` sees
                    // `int`, evaluation.dart's `definingType`
                    // instantiation). `Assert`/the prefix kinds carry no
                    // `Ty`.
                    UnOp::As(ty) => UnOp::As(self.subst_init_ty(*ty, env)),
                    UnOp::Is(ty) => UnOp::Is(self.subst_init_ty(*ty, env)),
                    UnOp::IsNot(ty) => UnOp::IsNot(self.subst_init_ty(*ty, env)),
                    _ => *op,
                },
                operand: Box::new(self.subst_init_tys(operand, env)),
            },
            Expr::Member { recv, name } => Expr::Member {
                recv: Box::new(self.subst_init_tys(recv, env)),
                name: *name,
            },
            Expr::Binary { op, left, right } => Expr::Binary {
                op: *op,
                left: Box::new(self.subst_init_tys(left, env)),
                right: Box::new(self.subst_init_tys(right, env)),
            },
            Expr::Cond { cond, then, els } => Expr::Cond {
                cond: Box::new(self.subst_init_tys(cond, env)),
                then: Box::new(self.subst_init_tys(then, env)),
                els: Box::new(self.subst_init_tys(els, env)),
            },
            Expr::Interp { parts } => Expr::Interp {
                parts: parts.iter().map(|p| self.subst_init_tys(p, env)).collect(),
            },
            Expr::Adjacent(parts) | Expr::List(parts) | Expr::Set(parts) | Expr::Record(parts) => {
                let parts = parts.iter().map(|p| self.subst_init_tys(p, env)).collect();
                match &node.expr {
                    Expr::Adjacent(_) => Expr::Adjacent(parts),
                    Expr::List(_) => Expr::List(parts),
                    Expr::Set(_) => Expr::Set(parts),
                    _ => Expr::Record(parts),
                }
            }
            Expr::Map(parts) => {
                Expr::Map(parts.iter().map(|p| self.subst_init_tys(p, env)).collect())
            }
            Expr::Entry(key, value) => Expr::Entry(
                Box::new(self.subst_init_tys(key, env)),
                Box::new(self.subst_init_tys(value, env)),
            ),
            Expr::IfElement { cond, then, els } => Expr::IfElement {
                cond: Box::new(self.subst_init_tys(cond, env)),
                then: Box::new(self.subst_init_tys(then, env)),
                els: els.as_ref().map(|e| Box::new(self.subst_init_tys(e, env))),
            },
            Expr::Spread {
                operand,
                null_aware,
            } => Expr::Spread {
                operand: Box::new(self.subst_init_tys(operand, env)),
                null_aware: *null_aware,
            },
            Expr::Call { name, args } => Expr::Call {
                name: *name,
                args: args.iter().map(|a| self.subst_init_tys(a, env)).collect(),
            },
            // Value/Ident/Faulted/Opaque/Other carry no `Ty` face.
            _ => node.expr.clone(),
        };
        crate::const_eval::Node {
            expr,
            span: node.span,
        }
    }

    /// The `Ty` half of [`Self::subst_init_tys`]: a `Named` head bound in
    /// the creation's env resolves to its written argument; an unbound
    /// head keeps its spelling with its own argument list substituted
    /// elementwise (`List<T>` under `C<int>` → `List<int>`). Other shapes
    /// return verbatim — the `Ty` lattice carries no param face inside
    /// `Record`/`Function` slots this face needs. Named `subst_init_ty`
    /// against the alias-instantiation `subst_ty` (`assignability.rs`)
    /// — the two `impl Engine` faces share the type's method namespace.
    fn subst_init_ty(&self, ty: Ty, env: &FxHashMap<StringId, Ty>) -> Ty {
        let Ty::Named {
            name,
            nullable,
            args,
        } = ty
        else {
            return ty;
        };
        if let Some(bound) = env.get(&name) {
            // The written `?` joins the bound argument's nullability
            // (`x as T?` under `C<int>` stays `int?`).
            return if nullable {
                match *bound {
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
            } else {
                *bound
            };
        }
        if args == ArgListId::EMPTY {
            return ty;
        }
        let list = self.boundary.args.borrow().get(args).to_vec();
        if !list.iter().any(|arg| self.ty_mentions_init_env(arg, env)) {
            return ty;
        }
        let joined: Vec<Ty> = list
            .iter()
            .map(|arg| self.subst_init_ty(*arg, env))
            .collect();
        self.boundary
            .args
            .borrow_mut()
            .named(name, nullable, &joined)
    }

    /// Whether `ty` mentions an env-bound name (its own head or inside
    /// its argument list) — the early-exit half of [`Self::subst_init_ty`].
    fn ty_mentions_init_env(&self, ty: &Ty, env: &FxHashMap<StringId, Ty>) -> bool {
        let Ty::Named { name, args, .. } = ty else {
            return false;
        };
        if env.contains_key(name) {
            return true;
        }
        if *args == ArgListId::EMPTY {
            return false;
        }
        self.boundary
            .args
            .borrow()
            .get(*args)
            .iter()
            .any(|arg| self.ty_mentions_init_env(arg, env))
    }
    //
    // `_checkForLiteralConstructorUse` (best_practices_verifier.dart
    // :1276-1290) hooks every constructor invocation; the decision order is
    // resolved target → `isConst` → `hasLiteral` → `canBeConst` → report.
    // The dartq faces: the plain send shape (`Point(1, 2)`, `Point.named(1,
    // 2)`) arms a capture window at its argument list's open (the
    // const-context guards live there — already-`const` and `new`-prefixed
    // shapes parse through different events and never arm), the arguments
    // constant-evaluate for the `canBeConst` face, and the send's fold
    // adjudicates.

    /// Arms the candidate window for an opening argument list (parse-time
    /// facts only): the head must be an in-file class container used in a
    /// plain send (`ctor_pos` 0 excludes the `new`/`const`/generic-creation
    /// reference shapes — the twin code is out of the frozen universe, so
    /// the `new` shape stays silent), no const context may own the
    /// position, and the capture's nesting rule must accept another
    /// non-const window. Returns the arm for the list's per-list slot —
    /// `None` leaves the slot empty.
    pub(crate) fn arm_literal_ctor_args(&mut self) -> Option<LiteralCtorArm> {
        // The list-open hook every argument list passes through, ahead of
        // the engine's own level push (the creation-level read needs it).
        self.emit_const_type_param_implicit_named();
        if self.ctor_pos != 0
            || self.annotation_at.is_some()
            || self.default_value_depth != 0
            || (self.initializer_depth != 0 && self.in_const_ctor)
        {
            return None;
        }
        let (callee, head) = self.literal_ctor_callee_face()?;
        if !self.const_capture.open_literal_ctor() {
            return None;
        }
        Some(LiteralCtorArm { callee, head })
    }

    /// The candidate face of the argument list's callee rows: the unnamed
    /// shape's head row IS the class container (`Point(`); the named
    /// shape's selector rides a continuation placeholder whose receiver
    /// row (the head, still in its pre-merge shape — the merge event only
    /// fires after the send) sits directly below. Only class containers
    /// are candidates — enums are never constructed, mixins cannot be,
    /// and extension overrides carry no constructor.
    pub(crate) fn literal_ctor_callee_face(&self) -> Option<(usize, SourceRange)> {
        let idx = self.rows.len().checked_sub(1)?;
        let row = self.rows.get(idx)?;
        match &row.shape {
            RowShape::Bare(BareInfo::Container {
                kind: ContainerKind::Class,
                ..
            }) => Some((idx, row.range)),
            // `C.named(`: the placeholder's receiver row carries the head
            // (the creation fold, or the raw container when the head send
            // had no argument list to fold through).
            RowShape::Bare(BareInfo::Plain { ty: Ty::UNKNOWN }) if row.qualified => {
                let recv = self.rows.get(idx.checked_sub(1)?)?;
                match &recv.shape {
                    RowShape::Receiver(Receiver::Created {
                        kind: ContainerKind::Class,
                        ..
                    })
                    | RowShape::Bare(BareInfo::Container {
                        kind: ContainerKind::Class,
                        ..
                    }) => Some((idx, recv.range)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Closes the armed candidate window at the argument list's close: the
    /// captured argument trees join the stash under the callee row's
    /// identity, awaiting the send's adjudication. Exactly one slot pops —
    /// the list's own; only an armed list closes a window (the
    /// `top_is_literal_ctor` probe keeps a desynced slot from stashing an
    /// empty-args entry, which would adjudicate feasible).
    pub(crate) fn close_literal_ctor_args(&mut self, end: SourceRange) {
        // The annotation window's own argument list closes through the
        // same event (#2261): its head never became a capture operand (a
        // metadata reference is not an expression), so the generic close
        // drops the level unreported — the drain takes it here, ahead of
        // the capture's own `end_arguments`, while the engine's argument
        // level still holds the labels.
        self.check_annotation_const_arguments(end);
        let Some(arm) = self.literal_ctor_arms.pop().flatten() else {
            return;
        };
        if !self.const_capture.top_is_literal_ctor() {
            return;
        }
        let span = SourceRange {
            offset: arm.head.offset,
            length: end.end().saturating_sub(arm.head.offset),
        };
        let args = self.const_capture.close_literal_ctor(span);
        self.literal_ctor_stash.push((arm.callee, args, span));
    }

    /// The send-side adjudication (the oracle's decision order): the
    /// constructor must resolve, the constructor must be declared `const`
    /// (`canBeConst`'s opening guard, ast.dart:13743-13744), the
    /// constructor declaration must carry the meta `@literal` annotation,
    /// and every argument must constant-evaluate. Reported at the
    /// whole-invocation span with the written constructor-reference text
    /// (`{0}`); `isConst` and the `new` twin never reach this face (their
    /// parse shapes never arm).
    pub(crate) fn check_literal_ctor_invocation(
        &mut self,
        callee: usize,
        args: &[crate::const_eval::Node],
        span: SourceRange,
    ) {
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        // The same two shapes the arm read: the unnamed head row, and the
        // named continuation placeholder with its head row (still in its
        // pre-merge shape) one row below.
        let (container, selector) = match &row.shape {
            RowShape::Bare(BareInfo::Container {
                name,
                kind: ContainerKind::Class,
            }) => (*name, None),
            RowShape::Bare(BareInfo::Plain { ty: Ty::UNKNOWN }) if row.qualified => {
                let receiver = callee.checked_sub(1).and_then(|i| self.rows.get(i));
                match receiver {
                    Some(receiver) => match &receiver.shape {
                        RowShape::Receiver(Receiver::Created {
                            name,
                            kind: ContainerKind::Class,
                            ..
                        })
                        | RowShape::Bare(BareInfo::Container {
                            name,
                            kind: ContainerKind::Class,
                        }) => (*name, Some(row.name)),
                        _ => return,
                    },
                    None => return,
                }
            }
            _ => return,
        };
        // This-file constructor resolution (cross-file containers
        // under-report, the standing producer posture — `const_ctor_target`
        // and `site_annotation` share it).
        let decl = self
            .index
            .get(self.file)
            .and_then(|elements| elements.container(container));
        let resolves = match (decl, selector) {
            (Some(decl), None) => {
                decl.has_unnamed_constructor || decl.constructors.contains(&container)
            }
            (Some(decl), Some(selector)) => decl.constructors.contains(&selector),
            (None, _) => false,
        };
        if !resolves {
            return;
        }
        // The declaration-side faces read one constructor site: the const
        // gate first (the oracle's `canBeConst` opening guard — a
        // constructor-level fact, not the container-level const-ctor set:
        // a `@literal` non-const constructor stays silent while its const
        // sibling reports), then the `@literal` face (the CONSTRUCTOR
        // declaration's collected annotations resolve to meta's `literal`
        // — an annotation on the class header is inert, the official reads
        // the constructor's own metadata). Resolution routes through the
        // import faces (`crate::annotation`), never a foreign `Ty`.
        let ctor_name = selector.unwrap_or(container);
        let Some(site) = self.decl_sites.iter().find(|site| {
            matches!(&site.kind, DeclKind::Constructor { container: c } if *c == container)
                && site.name == ctor_name
        }) else {
            return;
        };
        if !site.const_ctor {
            return;
        }
        let has_literal = site.annotations.iter().any(|a| {
            crate::annotation::is_meta_literal(
                &crate::model::Annotation {
                    name: *a,
                    range: site.range,
                    args_text: None,
                },
                self.strings,
                self.graph,
                self.lib,
            )
        });
        if !has_literal {
            return;
        }
        // The `canBeConst` face: every argument constant-evaluates (the
        // deferral verdict counts — a foreign/forward constant is one).
        let feasible = args
            .iter()
            .all(|arg| matches!(eval(arg, self), Verdict::Value(_) | Verdict::Constant));
        if !feasible {
            return;
        }
        // The written reference text: the container spelling, dot-joined
        // with the selector for the named form.
        let text = match selector {
            None => self.strings.resolve(container).to_owned(),
            Some(selector) => {
                format!(
                    "{}.{}",
                    self.strings.resolve(container),
                    self.strings.resolve(selector)
                )
            }
        };
        self.stage_usage(SemanticCode::NonConstCallToLiteralConstructor, span, &text);
    }

    /// Elements of a closed explicit const literal: each evaluates
    /// against its `non_constant_*_element` default (the
    /// `_ConstLiteralVerifier` site).
    /// Checks each element of a const literal against `code`. Returns the
    /// first non-constant element's anchor: the literal itself is then not
    /// a constant, and the enclosing declaration's verdict reports there
    /// (`const x = [nc]` carries both `non_constant_list_element` and
    /// `const_initialized_with_non_constant_value` at `nc`). A nested
    /// literal's failure re-surfaces at the outer element with the same
    /// anchor; the oracle's diagnostic set collapses the identical row
    /// (`RecordingDiagnosticListener` stores a `Set<Diagnostic>`), so the
    /// emit skips an exact repeat.
    /// The invocation's own span when the element leaf at `span` closed an
    /// unresolved callee's invocation (`[Undefined()]`): the head answer is
    /// the span's FIRST bare reference row — a bare miss there is the callee
    /// (`undefined` names nothing), a resolved head is a
    /// method-invocation/deferral world the probe must not speak for
    /// (`resolvableFn(missing())`'s invocation fault is override-listed),
    /// and literal-completion rows (`Value` — a string key beside the call)
    /// are not references at all. The row's offset re-anchors the face when
    /// a neighboring operand widened the staged span (the close's receiver
    /// extension cannot tell a `:` from a `.`). Only the literal element
    /// faces consult this — a declaration root's unresolved callee keeps
    /// the deferred-creation verdict path (its
    /// `const_initialized_with_non_constant_value` row anchors through the
    /// tail pass beside the resolution rows).
    fn unresolved_call(&self, span: SourceRange) -> Option<SourceRange> {
        let mut at = self
            .rows
            .partition_point(|row| row.range.offset < span.offset);
        while let Some(row) = self.rows.get(at)
            && row.range.offset < span.end()
        {
            match &row.shape {
                // A literal/receiver completion: not a reference — skip.
                RowShape::Value => {}
                _ => {
                    if !row.qualified
                        && row.range.end() <= span.end()
                        && matches!(row.shape, RowShape::Bare(BareInfo::Missing { .. }))
                    {
                        return Some(SourceRange {
                            offset: row.range.offset,
                            length: span.end().saturating_sub(row.range.offset),
                        });
                    }
                    // A resolved reference heads the invocation: the
                    // overridden/deferred world — not an unresolved call.
                    return None;
                }
            }
            at += 1;
        }
        None
    }

    /// The enclosing const constructor's formal names while ITS initializer
    /// list is open (`initializer_depth`/`in_const_ctor`), empty everywhere
    /// else: the site the constructor parse armed (`open_ctor_site`) names
    /// the constructor whose formals are in scope (the site name rewritten
    /// to the selector — the collector's own table key), and its table
    /// carries the initializing formals under their reference spelling too.
    pub(crate) fn ctor_init_formals(&self) -> Vec<StringId> {
        if self.initializer_depth == 0 || !self.in_const_ctor {
            return Vec::new();
        }
        self.open_ctor_site
            .and_then(|site| self.decl_sites.get(site))
            .and_then(|site| match &site.kind {
                DeclKind::Constructor { container } => Some((*container, site.name)),
                _ => None,
            })
            .and_then(|(container, ctor_name)| {
                // A bare `new` head (`const new(...)`) keeps the `(` site
                // spelling; the collector keys its formals under the class
                // name (the unnamed face).
                let key = if self.is_bare_new_head(ctor_name) {
                    container
                } else {
                    ctor_name
                };
                self.index
                    .get(self.file)
                    .and_then(|f| f.container(container))?
                    .ctor_formals
                    .get(&key)
                    .map(|formals| {
                        formals
                            .positional
                            .iter()
                            .chain(formals.named.iter())
                            .map(|formal| formal.name)
                            .collect()
                    })
            })
            .unwrap_or_default()
    }

    /// The element verifier's return: `Some(verdict)` is the literal's
    /// own value-fold — `Verdict::Fault` is an allow-listed
    /// `constEval*`/`nonBoolCondition` failure (a `for` element's
    /// `Faulted` leaf or an `if` condition's non-bool), so the caller
    /// pushes a fault leaf (the allow list overrides the site's
    /// const-init default); `Verdict::NotConstant`/`Unresolved` is a
    /// plain element failure pushed as a non-constant leaf. `None` means
    /// every element folded clean.
    pub fn check_const_elements(
        &mut self,
        code: SemanticCode,
        parts: &[crate::const_eval::Node],
    ) -> Option<Verdict> {
        // `constantContext2` (ast.dart:17927-17983) walks a literal's
        // ancestors: inside a `this(...)`/`this.named(...)`/`super(...)`/
        // `super.named(...)` argument list it hits the
        // `RedirectingConstructorInvocation`/`SuperConstructorInvocation`
        // `default: return null` arm — the literal is never `isConst`, so
        // no element face runs (only the arglist close's
        // `_reportNotPotentialConstantsArguments` scan applies, owned by
        // the init-invocation argument close).
        if self.initializer_depth > 0
            && self
                .arg_levels
                .iter()
                .any(|level| self.init_invocation_level(level))
        {
            return None;
        }
        // The const constructor's own initializer list is the
        // potentially-constant context (upstream Dart SDK source,
        // pkg/analyzer/lib/src/dart/constant/constant_verifier.dart
        // `_validateConstructorInitializers` +
        // pkg/analyzer/lib/src/dart/constant/potentially_constant.dart
        // `_nameExpression`): a formal-parameter reference of the
        // enclosing const constructor inside a constructor initializer is
        // potentially constant, so the element faces must not fault it —
        // the `const L({num? progress}) : rec = (progress: progress);`
        // riverpod shape stays silent. The verdicts evaluate with the
        // formals bound as opaque (potentially-constant) names; every
        // other leaf classifies exactly as before, so a genuinely
        // non-constant element still reports through the same face.
        let formals = self.ctor_init_formals();
        let mut env: FxHashMap<StringId, CtorBinding> = FxHashMap::default();
        for name in &formals {
            env.insert(*name, CtorBinding::Opaque);
        }
        let scope = CtorScope {
            engine: self,
            env: &env,
        };
        // The element evaluates under the ctor-formals scope when the
        // enclosing context binds any (the potentially-constant arm);
        // `self` answers every other leaf through the same `ConstLookup`
        // face.
        let lookup: &dyn ConstLookup = if formals.is_empty() { self } else { &scope };
        // The pure verifier walk: each element contributes its report rows
        // (site-code leaves and allow-listed faults), the spans folding
        // clean (whose subtrees still owe the resolved-creation leaf
        // sweep), and the literal's own verdict. Row emission happens in
        // the drain below — the `lookup` borrow ends there, freeing `self`
        // for `&mut` reporting.
        let mut rows: Vec<ElementRow> = Vec::new();
        let mut clean: Vec<SourceRange> = Vec::new();
        let mut failed = None;
        for part in parts {
            if let Some(v) = verify_const_element(part, code, lookup, &mut rows, &mut clean, self) {
                failed.get_or_insert(v);
            }
        }
        for span in clean {
            self.check_invalid_constant_leaves(span);
        }
        for row in rows {
            match row {
                ElementRow::Site(span, row_code) => self.report_element_leaf(span, row_code),
                ElementRow::Fault(span, fault) => self.emit_const_fault(fault, span),
            }
        }
        failed
    }

    /// Reports the element's site code at the failing leaf — the resolved-
    /// creation/ctor-initializer override arms and the annotation pairing
    /// all funnel through here (upstream `_reportError` on the site
    /// default).
    fn report_element_leaf(&mut self, span: SourceRange, code: SemanticCode) {
        let info = code.code_info();
        let repeat = self
            .report
            .diagnostics
            .iter()
            .rev()
            .any(|d| d.range == span && d.code == info);
        if repeat {
            return;
        }
        if self.leaf_in_resolved_creation(span.offset) {
            // The failing leaf sits inside a resolved const creation's
            // argument list: the oracle reports the leaf directly there
            // and the literal's element face never substitutes — the
            // positional arm's `const_with_non_constant_argument` already
            // owns the span, everything else is `invalid_constant`.
            if !self.const_arg_row_at(span) {
                self.emit(SemanticCode::InvalidConstant, span, "", "");
            }
        } else if self.initializer_depth > 0 && self.in_const_ctor {
            // A literal inside the const constructor's initializer list is
            // never `isConst` (`constantContext2` answers null on the
            // initializer ancestors), so the element faces never run there
            // — the gate is the initializer's potentially-constant scan,
            // whose face is `invalid_constant` at the failing leaf.
            self.emit(SemanticCode::InvalidConstant, span, "", "");
        } else {
            self.emit(code, span, "", "");
            // A leaf under an open annotation window joins the argument
            // faces' pairing log (#2261); a set/map element's own face
            // rides retractable — the type-declaration hosts' evaluation
            // owns the report there, decided at the annotation's close.
            let retractable = matches!(
                code,
                SemanticCode::NonConstantSetElement
                    | SemanticCode::NonConstantMapValue
                    | SemanticCode::NonConstantMapKey
            ) && self.span_ends_call(span);
            self.const_capture.note_annotation_leaf(span, retractable);
        }
    }

    /// The unresolved leaves (`Bare(Missing)` rows) under `span` that sit
    /// inside a resolved const creation's argument list: the oracle
    /// reports `invalid_constant` at the leaf (value.dart `genericError`
    /// — an unevaluatable operand reaches the constant machinery there)
    /// and marks the propagation avoid-reporting, so no enclosing literal
    /// or declaration face substitutes (#2261).
    fn check_invalid_constant_leaves(&mut self, span: SourceRange) {
        let mut at = self
            .rows
            .partition_point(|row| row.range.offset < span.offset);
        while let Some(row) = self.rows.get(at)
            && row.range.offset < span.end()
        {
            at += 1;
            if row.qualified
                || row.range.end() > span.end()
                || !matches!(row.shape, RowShape::Bare(BareInfo::Missing { .. }))
            {
                continue;
            }
            if self.const_arg_row_at(row.range) || !self.leaf_in_resolved_creation(row.range.offset)
            {
                continue;
            }
            self.emit(SemanticCode::InvalidConstant, row.range, "", "");
        }
    }

    /// Whether a const-argument row (`const_with_non_constant_argument`
    /// or `invalid_constant`) already reports at exactly `span` — the
    /// positional arm of a resolved creation's argument check runs before
    /// the literal closes, and the invalid-constant faces must not twin
    /// it.
    fn const_arg_row_at(&self, span: SourceRange) -> bool {
        let infos = [
            SemanticCode::ConstWithNonConstantArgument.code_info(),
            SemanticCode::InvalidConstant.code_info(),
        ];
        self.report
            .diagnostics
            .iter()
            .any(|d| d.range == span && infos.contains(&d.code))
    }

    /// Whether the leaf at `offset` sits inside a resolved const
    /// creation's argument list: the innermost enclosing bracket is a `(`
    /// whose head (the identifier before it, a `.selector`'s container
    /// when one is written) resolves to a class carrying a const
    /// constructor. Pure source reading — the capture's folds carry no
    /// argument spans for the shapes the oracle keys on (value.dart
    /// `genericError`'s `ArgumentList`-parent branch).
    fn leaf_in_resolved_creation(&self, offset: u32) -> bool {
        let bytes = self.source.as_bytes();
        let mut i = self.byte_of(offset);
        let mut depth = 0usize;
        while i > 0 {
            let byte = bytes[i - 1];
            i -= 1;
            match byte {
                b')' | b']' | b'}' => depth += 1,
                b'(' | b'[' | b'{' => {
                    if depth == 0 {
                        return byte == b'(' && self.head_before_is_const_ctor(i);
                    }
                    depth -= 1;
                }
                b'\'' | b'"' => {
                    // A string literal's interior: walk to its opening
                    // quote so brackets inside never skew the depth.
                    let quote = byte;
                    while i > 0 {
                        let previous = bytes[i - 1];
                        i -= 1;
                        if previous == quote && bytes.get(i.wrapping_sub(1)) != Some(&b'\\') {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// The evaluated construct that absorbs an unresolved-callee creation
    /// staged at `span` (see [`CreationFailureSite`]). The walk reads the
    /// source backwards from the creation head and stops at the first
    /// bracket that encloses an evaluation of its own: a collection
    /// literal, a call head (`Head(` / `Head.selector(`), or — past the
    /// initializer's `=`, where every const declaration's expression
    /// starts — nothing (the root). Grouping parens (`-(Missing())`, a
    /// generic head's `<T>(`) enclose no evaluation and step out; an
    /// identifier head without a const constructor is the wholesale
    /// rejection (`f(Missing())`).
    pub(crate) fn creation_failure_site(&self, span: SourceRange) -> CreationFailureSite {
        let bytes = self.source.as_bytes();
        let mut i = self.byte_of(span.offset);
        let mut depth = 0usize;
        while i > 0 {
            let byte = bytes[i - 1];
            i -= 1;
            match byte {
                b')' | b']' | b'}' => depth += 1,
                b'=' => return CreationFailureSite::Root,
                b'(' | b'[' | b'{' => {
                    if depth > 0 {
                        depth -= 1;
                        continue;
                    }
                    let open = self.utf16_of(i);
                    if byte != b'(' {
                        return CreationFailureSite::Collection { open };
                    }
                    if self.ident_end_before(i).is_none() {
                        // A grouping paren encloses no evaluation of its
                        // own — the construct outside decides.
                        continue;
                    }
                    if self.head_before_is_const_ctor(i) {
                        return CreationFailureSite::ConstCreation { open };
                    }
                    return CreationFailureSite::Other;
                }
                b'\'' | b'"' => {
                    // A string literal's interior: walk to its opening
                    // quote so brackets inside never skew the depth.
                    let quote = byte;
                    while i > 0 {
                        let previous = bytes[i - 1];
                        i -= 1;
                        if previous == quote && bytes.get(i.wrapping_sub(1)) != Some(&b'\\') {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        CreationFailureSite::Root
    }

    /// Whether a const-context failure row (`const_with_non_constant_
    /// argument` — a known non-constant argument the creation's evaluation
    /// aborted on — or `const_initialized_with_non_constant_value`) already
    /// anchors strictly between the evaluated construct's opening bracket
    /// (`open`, UTF-16) and `limit`: that failure stopped the oracle's
    /// evaluation there (evaluation.dart returns the non-`isUnresolved`
    /// error immediately), so a staged creation at `limit` was never
    /// evaluated and its own error stays silent. An earlier
    /// `invalid_constant` never stops the walk — unresolved creations
    /// continue on their placeholder (`C(Missing(), Missing2())` reports
    /// both).
    pub(crate) fn const_failure_row_between(&self, open: u32, limit: u32) -> bool {
        let infos = [
            SemanticCode::ConstWithNonConstantArgument.code_info(),
            SemanticCode::ConstInitializedWithNonConstantValue.code_info(),
        ];
        self.report
            .diagnostics
            .iter()
            .any(|d| d.range.offset > open && d.range.offset < limit && infos.contains(&d.code))
    }

    /// Whether a staged failure span is call-shaped (ends in `)`): the
    /// unresolved-creation entries (`Missing()`, the root
    /// `Missing2<int>('x')`) span calls, while a known non-constant
    /// argument's span is its failing leaf (`v`, the binary head of
    /// `v + 1`). The leaf entries pair their argument face with the
    /// variable face unconditionally; the call-shaped ones key on the
    /// evaluation site.
    pub(crate) fn creation_span_is_call(&self, span: SourceRange) -> bool {
        let bytes = self.source.as_bytes();
        self.byte_of(span.end())
            .checked_sub(1)
            .and_then(|i| bytes.get(i))
            .copied()
            == Some(b')')
    }

    /// Whether any diagnostic row anchors inside `span` (`[offset, end)`):
    /// the root unresolved creation's variable face rides the callee's own
    /// resolution row (`undefined_function`, `invocation_of_non_function`,
    /// `undefined_method`), which sits at the span's head for a bare callee
    /// (`Gone(`) but at the selector for a qualified one (`C.named(` — the
    /// oracle's `const_with_undefined_constructor` world). A creation of a
    /// resolved-but-unindexed container (an SDK class without a const
    /// constructor) carries no row and stays silent, the oracle's resolved
    /// non-const posture.
    pub(crate) fn const_failure_row_within(&self, span: SourceRange) -> bool {
        self.report
            .diagnostics
            .iter()
            .any(|d| d.range.offset >= span.offset && d.range.offset < span.end())
    }

    /// Whether the argument containing `span` already anchored a const
    /// failure: the oracle evaluates one constant per argument and its
    /// single error sits at the innermost first-failing sub-expression
    /// (`C(missing + Missing())` anchors `invalid_constant` at `missing`
    /// and the nested creation's own failure never surfaces). The bound is
    /// the byte after the argument's `,`/opener at depth zero, walked back
    /// from the staged span.
    pub(crate) fn const_arg_failure_anchored(&self, span: SourceRange) -> bool {
        let infos = [
            SemanticCode::InvalidConstant.code_info(),
            SemanticCode::ConstWithNonConstantArgument.code_info(),
            SemanticCode::ConstInitializedWithNonConstantValue.code_info(),
        ];
        let anchored = |start: u32| {
            self.report.diagnostics.iter().any(|d| {
                d.range.offset >= start && d.range.offset < span.offset && infos.contains(&d.code)
            })
        };
        let bytes = self.source.as_bytes();
        let mut i = self.byte_of(span.offset);
        let mut depth = 0usize;
        while i > 0 {
            let byte = bytes[i - 1];
            i -= 1;
            match byte {
                b')' | b']' | b'}' => depth += 1,
                b',' if depth == 0 => return anchored(self.utf16_of(i + 1)),
                b'(' | b'[' | b'{' => {
                    if depth > 0 {
                        depth -= 1;
                    } else {
                        return anchored(self.utf16_of(i + 1));
                    }
                }
                b'\'' | b'"' => {
                    let quote = byte;
                    while i > 0 {
                        let previous = bytes[i - 1];
                        i -= 1;
                        if previous == quote && bytes.get(i.wrapping_sub(1)) != Some(&b'\\') {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        anchored(0)
    }

    /// Whether the identifier ending right before the bracket at byte
    /// `paren` heads a const creation: `Head(` answers on the container's
    /// const-ctor set, `Head.selector(` also requires the selector to
    /// name a declared constructor (the oracle's
    /// `ConstructorInvocation.isConst` reading — a non-const selector is
    /// not a const creation). A two-dot prefix shape under-reports.
    fn head_before_is_const_ctor(&self, paren: usize) -> bool {
        let bytes = self.source.as_bytes();
        let Some(ident_end) = self.ident_end_before(paren) else {
            return false;
        };
        let head_start = self.ident_start_before(ident_end);
        if head_start > 0 && bytes[head_start - 1] == b'.' {
            let Some(container_end) = self.ident_end_before(head_start - 1) else {
                return false;
            };
            if container_end > 0 && bytes[container_end - 1] == b'.' {
                return false;
            }
            let container_start = self.ident_start_before(container_end);
            let head_name = self.intern(&self.source[container_start..container_end]);
            let Some(decl) = self.const_ctor_decl(head_name) else {
                return false;
            };
            let selector = self.intern(&self.source[head_start..ident_end]);
            return decl.constructors.contains(&selector);
        }
        let head_name = self.intern(&self.source[head_start..ident_end]);
        self.const_ctor_target(head_name)
    }

    /// The byte index just past the identifier ending right before byte
    /// `at` (whitespace between), `None` when no identifier is there.
    fn ident_end_before(&self, at: usize) -> Option<usize> {
        let bytes = self.source.as_bytes();
        let mut end = at;
        while end > 0 && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if end == 0 {
            return None;
        }
        let byte = bytes[end - 1];
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' {
            Some(end)
        } else {
            None
        }
    }

    /// The byte index where the identifier ending at `end` starts.
    fn ident_start_before(&self, end: usize) -> usize {
        let bytes = self.source.as_bytes();
        let mut start = end;
        while start > 0 {
            let byte = bytes[start - 1];
            if !(byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$') {
                break;
            }
            start -= 1;
        }
        start
    }

    /// Whether an open argument level's callee is a constructor-initializer
    /// invocation head — a bare `this(`/`super(` keyword receiver, the
    /// `this.named(` member shape, or the folded `super.named(` selector
    /// ([`Engine::pending_init_super`], the same shapes
    /// [`Engine::init_super_callee`] decides at the arglist close).
    pub(crate) fn init_invocation_level(&self, level: &ArgLevel) -> bool {
        let Some(head) = level.callee_row.and_then(|index| self.rows.get(index)) else {
            return false;
        };
        match &head.shape {
            RowShape::Receiver(Receiver::Super { .. } | Receiver::This { .. })
            | RowShape::Member {
                recv: Receiver::This { .. },
            } => true,
            _ => {
                // The `this.named(`/`super.named(` selectors: the selector
                // row (`Bare(Plain)` for `this._`, the folded `Value` for
                // `super.name`) rides directly on the keyword receiver
                // row — `x.m(` in the same position keeps its own receiver
                // shape below, so the `callee_row - 1` probe is the
                // redirect/super signature.
                let on_keyword_receiver = level.callee_row.is_some_and(|index| {
                    index > 0
                        && self.rows.get(index - 1).is_some_and(|below| {
                            matches!(
                                below.shape,
                                RowShape::Receiver(Receiver::This { .. } | Receiver::Super { .. })
                            )
                        })
                });
                // Every bare name inside the initializer list carries the
                // `init_rows` stamp — `f(` callees included — so the arm
                // reads as "any bare call in an initializer folds
                // non-constant", which is the oracle's
                // `_reportNotPotentiallyConstants` verdict for a runtime
                // call (`x = f(y)` reports `invalid_constant` at the whole
                // call). The ONE admitted shape is the two-argument
                // `identical(a, b)` — the potentially-constant grammar's
                // only invocation — which must reach the generic close's
                // `Expr::Call` fold instead (#3992: `assert(identical(
                // o1, o2))` in `Utils/expect_common.dart` folded to
                // `Expr::Other` here and false-fired `invalid_constant`).
                let identical_call = head.name == self.identical_id && level.args.len() == 2;
                on_keyword_receiver
                    || (self.init_rows.contains_key(&head.range.offset) && !identical_call)
                    || self
                        .pending_init_super
                        .iter()
                        .rev()
                        .any(|c| c.invoked && c.head && c.selector_offset == head.range.offset)
            }
        }
    }

    /// Pushes a closed const literal's value onto the capture: its
    /// structural value when every element was constant and known
    /// (`payload` — the duplicate twins compare by value), the caller's
    /// `deferred` structure node when any element's value is unknown
    /// (evaluating to the same opaque-kind instance while keeping the
    /// element subnodes), else the
    /// failing element's leaf — `Verdict::Fault` is an allow-listed
    /// `constEval*` failure pushed as a fault leaf (the enclosing
    /// declaration's site default swaps to the fault's code at the
    /// element, never a const-init twin); `NotConstant`/`Unresolved` is a
    /// plain non-constant leaf. Inside a creation's argument list the
    /// failure stays local — the oracle's argument faces there are
    /// `invalid_constant`, which this face does not model.
    /// `end` is the literal's closing token (the value node's span);
    /// `literal` is the whole literal extent (`[`/`{` through the close)
    /// — a fault leaf's staged span, since the declaration's own verdict
    /// for a `for` element wraps the whole initializer upstream
    /// (`genericError(node.initializer)` → the site default anchors at
    /// the literal, not the element).
    pub(crate) fn push_literal_value(
        &mut self,
        failed: Option<Verdict>,
        end: SourceRange,
        literal: SourceRange,
        kind: crate::const_eval::InstanceKind,
        payload: Option<ConstantValue>,
        deferred: Option<crate::const_eval::Expr>,
    ) {
        // The ERROR-tier duplicate twins of a const literal (#2261): the
        // capture staged this literal's parts at its close; the oracle's
        // `visitSetOrMapLiteral` checks every `isConst` literal's keys
        // and elements — after the element faces, at the value push.
        if let Some((shape, _)) = self.const_capture.take_staged_dup(kind) {
            self.check_const_duplicates(shape);
        }
        match failed {
            Some(Verdict::Fault(fault, span)) if !self.const_capture.in_arguments() => {
                // A `for` element's fault anchors its leaf at the literal —
                // the declaration's own verdict wraps the whole initializer
                // (`genericError(node.initializer)`), so the var-level
                // const-init twin spans the literal, not the element. Every
                // other allow-listed fault keeps its entity span (the
                // element-check already reported the allow-listed code
                // there — the pushed leaf's re-eval lands the declaration
                // verdict on the same entity and the `(code, range)` dedup
                // collapses them to the oracle's single row).
                let anchor = match fault {
                    ConstFault::ForElement => literal,
                    _ => span,
                };
                self.const_capture.push_fault(anchor, fault);
            }
            Some(Verdict::NotConstant(span) | Verdict::Unresolved(span))
                if !self.const_capture.in_arguments() =>
            {
                self.const_capture.push_non_constant(span);
            }
            _ => {
                // The all-deferred fallback (`payload` unknown and no
                // element failed): the STRUCTURE node replaces the opaque
                // kind value — `eval` folds it to the identical
                // `Value(Instance(kind))`, while the element subnodes stay
                // reachable for the declaration-close walks (a silently
                // deferred non-const static read inside a const collection
                // still reports `const_initialized_with_non_constant_value`
                // at the read, probed 3.13.7 — probe40: list/set/map-value/
                // record-field shapes all pair the element face with the
                // declaration row).
                if let Some(expr) = deferred {
                    self.const_capture.push_expr(literal, expr);
                } else {
                    self.const_capture
                        .push_value(end, payload.unwrap_or(ConstantValue::Instance(kind)));
                }
            }
        }
    }

    /// The closed literal's structural value: every element's folded
    /// value in order, `None` when any element is of unknown value (the
    /// opaque-kind fallback — the duplicate twins must not decide on a
    /// partial value).
    pub(crate) fn literal_payload(
        &mut self,
        parts: &[crate::const_eval::Node],
    ) -> Option<Vec<ConstantValue>> {
        parts
            .iter()
            .map(|part| match eval(part, self) {
                Verdict::Value(value) => Some(value),
                _ => None,
            })
            .collect()
    }

    /// The map literal's structural value: every `Expr::Entry` piece's
    /// key/value pair in order, `None` when any side is of unknown value
    /// or a control element sits in the literal (the ordered parts carry
    /// `if`/`spread`/`for` shapes whose folded value a `Map` constant
    /// cannot list — upstream `_buildMapConstant` splices them, this
    /// face's structural answer stays unknown there).
    pub(crate) fn map_payload(
        &mut self,
        parts: &[crate::const_eval::Node],
    ) -> Option<ConstantValue> {
        parts
            .iter()
            .map(|part| match &part.expr {
                crate::const_eval::Expr::Entry(key, value) => {
                    match (eval(key, self), eval(value, self)) {
                        (Verdict::Value(key), Verdict::Value(value)) => Some((key, value)),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(|pairs| ConstantValue::Map(pairs.into()))
    }

    /// The record literal's structural value from its staged fields and
    /// the literal's named labels (the trailing `names.len()` fields are
    /// the named pairs, in source order — the same split
    /// `record_field_tys` reads): positional fields first, named pairs
    /// sorted by label so written order never decides equality (the
    /// upstream `RecordState` compares the named map). `None` when any
    /// field is of unknown value.
    pub(crate) fn record_payload(
        &mut self,
        fields: &[crate::const_eval::Node],
        names: &[StringId],
    ) -> Option<ConstantValue> {
        let named = names.len();
        if named > fields.len() {
            return None;
        }
        let mut values = self.literal_payload(fields)?;
        let tail = values.split_off(values.len() - named);
        let mut pairs: Vec<(StringId, ConstantValue)> = names.iter().copied().zip(tail).collect();
        pairs.sort_by_key(|(name, _)| *name);
        Some(ConstantValue::Record {
            positional: values.into(),
            named: pairs.into(),
        })
    }

    /// The const-literal twin of [`Self::check_equal_duplicates`] (the
    /// `visitSetOrMapLiteral` `isConst` branch,
    /// constant_verifier.dart:430-475): the same later-part repeat
    /// report under the ERROR-tier `equal_elements_in_const_set` /
    /// `equal_keys_in_const_map` codes, at the later element's span. The
    /// per-part [`Verdict::Value`] gate absorbs spread/if/for elements
    /// (their folded operands never value) — the oracle's key-equality
    /// walk skips them the same way.
    fn check_const_duplicates(&mut self, shape: crate::const_eval::SetOrMap) {
        use crate::const_eval::SetOrMap;
        let (code, parts): (SemanticCode, Vec<crate::const_eval::Node>) = match shape {
            SetOrMap::Set(elements) => (SemanticCode::EqualElementsInConstSet, elements),
            SetOrMap::Map(parts) => (
                SemanticCode::EqualKeysInConstMap,
                parts
                    .into_iter()
                    .filter_map(|part| match part.expr {
                        crate::const_eval::Expr::Entry(key, _) => Some(*key),
                        _ => None,
                    })
                    .collect(),
            ),
        };
        self.report_duplicate_elements(code, &parts);
    }

    /// The `_checkForDuplications` face (`best_practices_verifier.dart`
    /// :1077-1109) over a closed NON-const set/map literal: each
    /// top-level element (map: each entry's KEY) constant-evaluates; a
    /// value an earlier element already produced reports
    /// `equal_elements_in_set` / `equal_keys_in_map` at the later
    /// element's own span, in element order, one row per repeat. Any
    /// verdict but [`Verdict::Value`] is silent (the oracle skips the
    /// element when `computeConstantValue` fails), and a literal holding
    /// spread/if/for elements skips wholesale — the oracle's
    /// `whereType<Expression>` excludes those elements and their folded
    /// operands cannot be told apart here (under-report, the capture's
    /// deferral posture). Value equality is the const kernel's
    /// [`ConstantValue`] equality: ints and doubles never unify
    /// (`{1, 1.0}` is silent).
    pub(crate) fn check_equal_duplicates(
        &mut self,
        shape: crate::const_eval::SetOrMap,
        has_ctl_elements: bool,
    ) {
        use crate::const_eval::SetOrMap;
        if has_ctl_elements {
            return;
        }
        let (code, parts): (SemanticCode, Vec<crate::const_eval::Node>) = match shape {
            SetOrMap::Set(elements) => (SemanticCode::EqualElementsInSet, elements),
            SetOrMap::Map(parts) => (
                SemanticCode::EqualKeysInMap,
                parts
                    .into_iter()
                    .filter_map(|part| match part.expr {
                        crate::const_eval::Expr::Entry(key, _) => Some(*key),
                        _ => None,
                    })
                    .collect(),
            ),
        };
        self.report_duplicate_elements(code, &parts);
    }

    /// The shared duplicate walk behind
    /// [`Self::check_const_duplicates`] and [`Self::check_equal_duplicates`]
    /// — one row per later element an earlier element already produced,
    /// at the later element's span, in element order. Three value worlds:
    ///
    /// * structural [`ConstantValue`] equality (the oracle's
    ///   `DartObjectImpl ==` over the states the kernel folds);
    /// * [`InstanceRefKey`] equality for const-variable references whose
    ///   instance value this unit cannot fold (a foreign library's const —
    ///   the declaring model's [`crate::model::ConstCreationFace`] answers
    ///   through the survivor walk below);
    /// * same-text equality for opaque [`ConstantValue::Created`] elements
    ///   that carry no key (a direct creation): identical spellings in one
    ///   file denote one value, so the repeat is certain — different
    ///   spellings defer (under-report: `P(1)` beside `P(0+1)` stays
    ///   silent where the oracle's equal field maps would report).
    fn report_duplicate_elements(&mut self, code: SemanticCode, parts: &[crate::const_eval::Node]) {
        let mut seen: Vec<ConstantValue> = Vec::new();
        let mut seen_keys: Vec<InstanceRefKey> = Vec::new();
        let mut seen_created: Vec<SourceRange> = Vec::new();
        for part in parts {
            // The Instance sentinel is a value UNKNOWN to this subset
            // (a deferred field — a cross-file const the in-file index
            // cannot fold): equality over unknowns proves nothing, and
            // two distinct records `(N.x, 'ol')`/`(N.x, 'ul')` both
            // collapsing to it would report a duplicate the oracle's
            // resolved values never equal.
            let value = match eval(part, self) {
                Verdict::Value(v) if matches!(v, ConstantValue::Instance(_)) => continue,
                Verdict::Value(v) => Some(v),
                Verdict::Constant => None,
                _ => continue,
            };
            // A const-variable reference (bare or prefixed) folds no
            // instance value here — the key world answers for it.
            if let Some((prefix, name)) = const_var_ref(part)
                && let Some(key) = self.const_ref_key(prefix, name)
            {
                if seen_keys.contains(&key) {
                    self.emit_with_correction(code, part.span);
                } else {
                    seen_keys.push(key);
                }
                continue;
            }
            match value {
                Some(v) if !matches!(v, ConstantValue::Created(_)) => {
                    if seen.contains(&v) {
                        self.emit_with_correction(code, part.span);
                    } else {
                        seen.push(v);
                    }
                }
                // A creation this unit cannot see into, or a constant of
                // unknown value: only an identical spelling proves the
                // repeat — different spellings defer.
                _ => {
                    let text = self.source_slice(part.span);
                    if seen_created
                        .iter()
                        .any(|span| self.source_slice(*span) == text)
                    {
                        self.emit_with_correction(code, part.span);
                    } else {
                        seen_created.push(part.span);
                    }
                }
            }
        }
    }

    /// The source text under a (UTF-16) span — the same-text equality
    /// face of [`Self::report_duplicate_elements`]'s deferred world.
    fn source_slice(&self, span: SourceRange) -> &str {
        let start = self.byte_of(span.offset).min(self.source.len());
        let end = self.byte_of(span.end()).max(start).min(self.source.len());
        &self.source[start..end]
    }

    /// The written named arguments of a const creation that survive into
    /// the oracle's `GenericState` field map (value.dart:1528): a
    /// non-`super` formal's argument survives at its own field name; a
    /// bare `super.x` formal (the [`Ty::Never`] sentinel) forwards to the
    /// superclass constructor's same-named parameter
    /// (`evaluation.dart`'s implicit super-formal arguments) and recurses
    /// one hop up. Forwarded arguments evaporate wherever the chain stops
    /// evaluating — the superclass is absent (implicit `Object`:
    /// `_checkSuperConstructorCall` skips the whole block,
    /// evaluation.dart:3400-3414), unresolvable (a broken import:
    /// `lookUpConstructor` answers nothing), or declares no const
    /// constructor (the `isConst` gate); a forwarded label with no
    /// associated super parameter drops the same way. `None` defers: a
    /// written label the creation's own constructor cannot answer, or a
    /// chain past the depth cap.
    fn surviving_arg_labels(
        &self,
        hop_name: StringId,
        container: &Container,
        container_file: FileId,
        labels: &[StringId],
    ) -> Option<Vec<StringId>> {
        let mut pending: Vec<StringId> = labels.to_vec();
        let mut survivors: Vec<StringId> = Vec::new();
        let mut hop = (hop_name, container, container_file);
        let mut depth = 0;
        loop {
            if pending.is_empty() {
                break;
            }
            if depth == 8 {
                return None;
            }
            let formals = hop.1.ctor_formals.get(&hop.0)?;
            let mut forwarded: Vec<StringId> = Vec::new();
            for label in &pending {
                match formals.named_formal(*label) {
                    // A written label the CREATION's own constructor cannot
                    // answer is an invalid argument — defer (the oracle's
                    // evaluation stops on it). A FORWARDED label the super
                    // constructor cannot answer has no associated super
                    // parameter (the `super_formal_parameter_without_
                    // associated_named` drop): the argument evaporates.
                    None => {
                        if depth == 0 {
                            return None;
                        }
                    }
                    Some(formal) => {
                        if formal.ty == Ty::Never {
                            forwarded.push(*label);
                        } else {
                            survivors.push(*label);
                        }
                    }
                }
            }
            if forwarded.is_empty() {
                break;
            }
            // The super hop decides the forwarded set's fate. Where the
            // chain stops evaluating — an implicit `Object` superclass,
            // an unresolvable superclass name, or no const constructor —
            // every forwarded argument evaporates (`_checkSuperConstructorCall`
            // writes nothing), which ends the walk successfully.
            let Some(sup_name) = hop.1.superclass else {
                pending.clear();
                break;
            };
            let sup_lib = self.graph.library_of_file(hop.2)?;
            let Some((sup_container, sup_file)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                sup_lib,
                hop.2,
                sup_name,
            ) else {
                pending.clear();
                break;
            };
            if !sup_container.has_const_constructor {
                pending.clear();
                break;
            }
            pending = forwarded;
            hop = (sup_name, sup_container, sup_file);
            depth += 1;
        }
        (pending.is_empty()).then_some(survivors)
    }

    /// The instance key of a const-variable reference element (`name`
    /// bare, `p.name` prefixed): the declaring file (the texts compare
    /// only within one file — identical spellings share that file's
    /// library scope, so they denote identical values), the creation's
    /// container, the variable's declared type, and the surviving
    /// argument texts. `None` defers — not a top-level const variable,
    /// no captured face, or an undecided survivor walk.
    fn const_ref_key(&self, prefix: Option<StringId>, name: StringId) -> Option<InstanceRefKey> {
        let decl_file = match prefix {
            Some(prefix) => {
                match self
                    .namespaces
                    .lookup_prefixed(self.graph, self.lib, prefix, name)
                {
                    Lookup::Found(NamespaceEntry::Decl(file)) => file,
                    _ => return None,
                }
            }
            None => match self.namespaces.lookup(self.graph, self.lib, name) {
                Lookup::Found(NamespaceEntry::Decl(file)) => file,
                _ => return None,
            },
        };
        let elements = self.index.get(decl_file)?;
        let face = elements.const_creation(name)?;
        let var_ty = elements.decl(name)?.var_ty;
        let decl_lib = self.graph.library_of_file(decl_file)?;
        let (container, cont_file) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            decl_lib,
            decl_file,
            face.container,
        )?;
        if !container.has_const_constructor {
            return None;
        }
        let labels: Vec<StringId> = face.named.iter().map(|(label, _)| *label).collect();
        let survivors = self.surviving_arg_labels(face.container, container, cont_file, &labels)?;
        let fields: Box<[(StringId, StringId)]> = face
            .named
            .iter()
            .filter(|(label, _)| survivors.contains(label))
            .copied()
            .collect();
        Some(InstanceRefKey {
            file: decl_file,
            container: face.container,
            var_ty,
            fields,
        })
    }

    /// The annotation-argument faces (#2261): a RESOLVED const
    /// constructor called in annotation position validates its arguments
    /// (`constant_verifier.dart` `_validateConstantArguments` over the
    /// annotation's invocation) against the argument-constancy and
    /// argument-count faces, exactly like the initializer world's
    /// creation check. The capture never holds the head (a metadata
    /// reference is not an expression operand), so the drain re-reads
    /// the written head from the source before the open paren and
    /// resolves it through the container faces; the engine's still-open
    /// argument level supplies the written labels (a named argument
    /// whose label names no formal never pairs with the creation's
    /// argument check — the official's evaluation reports the leaf
    /// directly there, `invalid_constant` at the value).
    fn check_annotation_const_arguments(&mut self, close: SourceRange) {
        let Some((args, named)) = self.const_capture.take_annotation_args() else {
            return;
        };
        // The engine's argument level still holds this list: flush its
        // tail so the written labels align with the capture's arguments
        // (the generic close runs the same flushes right after, and both
        // are no-ops once flushed).
        self.flush_unclaimed();
        self.flush_argument();
        let labels: Vec<(Option<StringId>, SourceRange)> = self
            .arg_levels
            .last()
            .map(|level| {
                level
                    .args
                    .iter()
                    .map(|(name, _, _, range)| (*name, *range))
                    .collect()
            })
            .unwrap_or_default();
        // The literal-element leaves under this annotation's arguments are
        // consumed here unconditionally — a nested annotation's own drain
        // (its `(` closes first) already stripped its leaves, so what is
        // left belongs to this window even when the alignment check below
        // bails.
        let leaves = self.const_capture.take_annotation_leaves(0, close.end());
        let Some((_head_range, head_name, selector)) = self.annotation_head_before(close) else {
            return;
        };
        if labels.len() != args.len() {
            return;
        };
        let Some(decl) = self.const_ctor_decl(head_name) else {
            return;
        };
        let Some(formals) = decl.ctor_formals.get(&selector.unwrap_or(head_name)) else {
            return;
        };
        // The argument-count faces: upstream `resolveArgumentsToParameters`
        // (type_analyzer.dart) anchors a deficit at
        // `lastPositionalArgument?.endToken.next ?? leftParenthesis.next ??
        // rightParenthesis` — the argument list's close `)` for every
        // positional-arg shape this face sees — and a surplus at the first
        // unresolved (extra) positional argument's expression (the probed
        // `@P()` / `@P(1, 2, 3)` pairings: `)` and `3`).
        let given = named.iter().filter(|flag| !**flag).count();
        // Required-ness is the parser's classification (the formal's own
        // group: `[...]` is optional whether or not a default is written —
        // `const JS([this.name])` takes no argument), exactly the count the
        // invocation face reads.
        let required = formals.positional.iter().filter(|f| f.required).count();
        if given < required {
            let display = match selector {
                Some(id) => format!(
                    "{}.{}",
                    self.strings.resolve(head_name),
                    self.strings.resolve(id)
                ),
                None => format!("{}.new", self.strings.resolve(head_name)),
            };
            let (code, message) = if required == 1 {
                let code = SemanticCode::NotEnoughPositionalArguments(ArgCountShape::NameSingular);
                let message = code.message(&display, "");
                (code, message)
            } else {
                let code = SemanticCode::NotEnoughPositionalArguments(ArgCountShape::NamePlural);
                let message = code.message3(&required.to_string(), &given.to_string(), &display);
                (code, message)
            };
            self.emit_message_with_correction(code, close, message);
        } else if given > formals.positional.len() {
            let capacity = formals.positional.len();
            if let Some((arg, _)) = args
                .iter()
                .zip(named.iter())
                .filter(|(_, flag)| !**flag)
                .nth(capacity)
            {
                let code = SemanticCode::ExtraPositionalArguments;
                let message = code.message(&capacity.to_string(), &given.to_string());
                let surplus = self.literal_arg_span(arg.span);
                self.emit_message_with_correction(code, surplus, message);
            }
        }
        // The per-argument const check: each top-level argument evaluates
        // against `const_with_non_constant_argument` (the `@P(g)` pairing);
        // a named argument whose label names no formal reports
        // `invalid_constant` at the value instead (the probed
        // `Prop(name: Missing)` pairing), and a leading-dot shorthand stays
        // silent beside its resolution rows.
        for ((arg, flag), (label, _)) in args.iter().zip(named.iter()).zip(labels.iter()) {
            let label_resolved = label.is_some_and(|n| formals.named.iter().any(|f| f.name == n));
            let invalid_world = *flag && !label_resolved;
            match eval(arg, self) {
                Verdict::Value(_) | Verdict::Constant => {}
                Verdict::NotConstant(span) => {
                    if !self.const_arg_row_at(span) {
                        self.emit(SemanticCode::ConstWithNonConstantArgument, span, "", "");
                    }
                }
                Verdict::Unresolved(span) => {
                    if self.dot_shorthand_context(span.offset) || self.const_arg_row_at(span) {
                        continue;
                    }
                    let code = if invalid_world {
                        SemanticCode::InvalidConstant
                    } else {
                        SemanticCode::ConstWithNonConstantArgument
                    };
                    self.emit(code, span, "", "");
                }
                Verdict::Fault(fault, span) => self.emit_const_fault(fault, span),
            }
        }
        // The literal-element leaves the element check recorded under this
        // annotation's arguments (the `@W([Missing])` pairing): the
        // argument validation pairs its own face at each —
        // `invalid_constant` when the leaf's label names no formal, the
        // argument face otherwise. A leaf already owned by an
        // `invalid_constant` row (inside a nested creation) stays
        // single-reported, and a call-shaped set/map element face on a
        // type-declaration host is retracted — the oracle's evaluation
        // owns the report there (the probed `@T({Missing()}) class C {}`
        // vs the enum/parameter hosts keeping it).
        let host_is_type_declaration = self.annotation_host_is_type_declaration(close);
        for (leaf, retractable) in leaves {
            if retractable && host_is_type_declaration {
                let codes = [
                    SemanticCode::NonConstantSetElement.code_info(),
                    SemanticCode::NonConstantMapValue.code_info(),
                    SemanticCode::NonConstantMapKey.code_info(),
                ];
                self.report
                    .diagnostics
                    .retain(|d| !(d.range == leaf && codes.contains(&d.code)));
            }
            if self.const_arg_row_at(leaf) {
                continue;
            }
            let invalid_world = labels
                .iter()
                .rev()
                .find(|(_, range)| range.offset <= leaf.offset)
                .and_then(|(name, _)| *name)
                .is_some_and(|n| !formals.named.iter().any(|f| f.name == n));
            let code = if invalid_world {
                SemanticCode::InvalidConstant
            } else {
                SemanticCode::ConstWithNonConstantArgument
            };
            self.emit(code, leaf, "", "");
        }
    }

    /// The written span of a literal argument value: the staged operand
    /// anchors at the literal's close token, so the extension matches the
    /// span's own opener and walks back through any enclosing ones (the
    /// `[Missing()]` surplus anchor).
    fn literal_arg_span(&self, span: SourceRange) -> SourceRange {
        let bytes = self.source.as_bytes();
        let mut start = self.byte_of(span.offset);
        if start == 0 {
            return span;
        }
        let mut closer = match bytes[start] {
            b']' => b'[',
            b')' => b'(',
            b'}' => b'{',
            _ => return span,
        };
        loop {
            let mut depth = 0usize;
            let mut i = start;
            let open = loop {
                if i == 0 {
                    return span;
                }
                i -= 1;
                match bytes[i] {
                    b if b == closer && depth == 0 => break i,
                    b']' | b')' | b'}' => depth += 1,
                    b'[' | b'(' | b'{' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            };
            start = open;
            if start == 0 || !matches!(bytes[start - 1], b']' | b')' | b'}') {
                break;
            }
            closer = match bytes[start - 1] {
                b']' => b'[',
                b')' => b'(',
                _ => b'{',
            };
        }
        SourceRange {
            offset: self.utf16_of(start),
            length: span.end().saturating_sub(self.utf16_of(start)),
        }
    }

    /// The annotation head written before the argument list's open paren:
    /// the bracket-matched `(`'s preceding identifier, its UTF-16 span
    /// (the deficit face's anchor), and the written `.selector` when one
    /// follows (`@P.named(...)` — a two-dot prefix shape under-reports).
    fn annotation_head_before(
        &self,
        close: SourceRange,
    ) -> Option<(SourceRange, StringId, Option<StringId>)> {
        let bytes = self.source.as_bytes();
        let mut i = self.byte_of(close.offset);
        let mut depth = 0usize;
        let open = loop {
            if i == 0 {
                return None;
            }
            i -= 1;
            match bytes[i] {
                b')' | b']' | b'}' => depth += 1,
                b'(' => {
                    if depth == 0 {
                        break i;
                    }
                    depth -= 1;
                }
                b'[' | b'{' => {
                    if depth == 0 {
                        return None;
                    }
                    depth -= 1;
                }
                b'\'' | b'"' => i = self.string_open_before(i),
                _ => {}
            }
        };
        let Some(head_end) = self.ident_end_before(open) else {
            return None;
        };
        let mut head_start = self.ident_start_before(head_end);
        let mut head_text_end = head_end;
        let mut selector = None;
        if head_start > 0 && bytes[head_start - 1] == b'.' {
            selector = Some(self.intern(&self.source[head_start..head_end]));
            let Some(container_end) = self.ident_end_before(head_start - 1) else {
                return None;
            };
            if container_end > 0 && bytes[container_end - 1] == b'.' {
                return None;
            }
            head_text_end = container_end;
            head_start = self.ident_start_before(container_end);
        }
        let head_name = self.intern(&self.source[head_start..head_text_end]);
        let head_offset = self.utf16_of(head_start);
        Some((
            SourceRange {
                offset: head_offset,
                length: self.utf16_of(head_text_end).saturating_sub(head_offset),
            },
            head_name,
            selector,
        ))
    }

    /// The byte index of the opening quote ending the string literal whose
    /// closing quote sits at `i` (best-effort: a failed walk lands at 0).
    fn string_open_before(&self, mut i: usize) -> usize {
        let bytes = self.source.as_bytes();
        let quote = bytes[i];
        while i > 0 {
            i -= 1;
            if bytes[i] == quote && bytes.get(i.wrapping_sub(1)) != Some(&b'\\') {
                return i;
            }
        }
        0
    }
    /// Emits with the row's oracle `correctionMessage` (the plain
    /// [`Engine::emit`] passes none).
    pub(crate) fn emit_with_correction(&mut self, code: SemanticCode, range: SourceRange) {
        let correction = code.correction_message("");
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message("", ""),
            self.file,
            range,
            correction,
        ));
    }

    // —— capture event forwards (cheap no-ops with no window) ———

    pub fn const_int_literal(&mut self, token: Token) {
        let Some(text) = self.text_of(token) else {
            return;
        };
        match parse_int_literal(text) {
            Some(v) => self
                .const_capture
                .literal(token.range, ConstantValue::Int(v)),
            // Beyond i64: a valid constant of unobservable value — defer.
            None => self.const_capture.opaque_leaf(token.range),
        }
    }

    pub fn const_double_literal(&mut self, token: Token) {
        let Some(text) = self.text_of(token) else {
            return;
        };
        let cleaned: String = text.chars().filter(|c| *c != '_').collect();
        match cleaned.parse::<f64>() {
            Ok(v) => self
                .const_capture
                .literal(token.range, ConstantValue::Double(v)),
            Err(_) => self.const_capture.opaque_leaf(token.range),
        }
    }

    pub fn const_bool_literal(&mut self, token: Token) {
        let value = self.text_of(token).is_some_and(|t| t == "true");
        self.const_capture
            .literal(token.range, ConstantValue::Bool(value));
    }

    pub fn const_null_literal(&mut self, token: Token) {
        self.const_capture.literal(token.range, ConstantValue::Null);
    }

    /// The raw interior of an interpolation-free string literal (the
    /// begin token spans the whole literal, quotes included; escapes are
    /// not decoded — no diagnostic observes the contents). The interior
    /// cuts in BYTES: `SourceRange` offsets are UTF-16 code units while
    /// `source` slices by byte, so the span goes through [`Engine::byte_of`]
    /// — a direct slice drifted one byte per preceding multi-byte char and
    /// valuated later literals with junk payloads (#2187: the junk collided
    /// in the duplicate-key check, reporting phantom `equal_keys_in_map`
    /// rows over every multibyte-heavy corpus file).
    pub fn const_string_leaf(&mut self, begin: SourceRange, end: SourceRange) {
        let text = self
            .source
            .get(self.byte_of(begin.offset) + 1..self.byte_of(begin.end()).saturating_sub(1))
            .unwrap_or("");
        self.const_capture
            .end_literal_string(0, begin, end, ConstantValue::Str(text.into()));
    }

    pub fn const_string_end(&mut self, count: usize, begin: SourceRange, end: SourceRange) {
        self.const_capture
            .end_literal_string(count, begin, end, ConstantValue::Str("".into()));
    }
}

/// A report row the pure element verifier stages: a site-code leaf or
/// an allow-listed `constEval*`/`nonBoolCondition` fault.
enum ElementRow {
    Site(SourceRange, SemanticCode),
    Fault(SourceRange, ConstFault),
}

/// One collection element's verifier arm — the pure half of upstream
/// `_ConstLiteralVerifier.verify`: each shape contributes its report
/// rows (site-code leaves / allow-listed faults) and the literal's own
/// verdict. `clean` accumulates the spans folding clean whose subtrees
/// still owe the resolved-creation leaf sweep. The `engine` read-only
/// probes (`dot_shorthand_context`, `unresolved_call`) stay on `self`;
/// emission is the caller's drain.
fn verify_const_element(
    part: &crate::const_eval::Node,
    code: SemanticCode,
    lookup: &dyn ConstLookup,
    rows: &mut Vec<ElementRow>,
    clean: &mut Vec<SourceRange>,
    engine: &Engine<'_>,
) -> Option<Verdict> {
    use crate::const_eval::Expr;
    match &part.expr {
        Expr::IfElement { cond, then, els } => {
            // The condition evaluates with the element code (a non-bool
            // constant faults `non_bool_condition` at
            // `element.expression` — upstream `conditionValue == null`
            // → `InvalidConstant.forEntity(entity: element.expression)`).
            match eval(cond, lookup) {
                Verdict::Value(ConstantValue::Bool(b)) => {
                    // The dead branch's not-potentially-constant shapes
                    // each report the element code (upstream
                    // `_reportNotPotentialConstants` over the skipped
                    // element) but never fault the literal — the live
                    // branch alone decides the verdict.
                    // The live branch (`then` on true, `els` — which may
                    // be absent — on false) verifies like an element; the
                    // dead one scans its not-potentially-constant shapes.
                    if b {
                        if let Some(els) = els.as_deref() {
                            dead_branch_npc(els, code, lookup, rows, engine);
                        }
                        verify_const_element(then, code, lookup, rows, clean, engine)
                    } else {
                        dead_branch_npc(then, code, lookup, rows, engine);
                        els.as_deref().and_then(|els| {
                            verify_const_element(els, code, lookup, rows, clean, engine)
                        })
                    }
                }
                // A known non-bool condition faults `non_bool_condition`
                // at the condition (the oracle's entity is the
                // expression, not the whole element).
                Verdict::Value(_) => {
                    rows.push(ElementRow::Fault(cond.span, ConstFault::NonBoolCondition));
                    Some(Verdict::Fault(ConstFault::NonBoolCondition, cond.span))
                }
                // An unknown-but-constant condition scans BOTH branches'
                // not-potentially-constant shapes (upstream `isUnknown`
                // → unknown collection, each branch's shapes still
                // report their element-code rows); the literal's own
                // verdict defers (the value is unknown, not failed).
                Verdict::Constant => {
                    dead_branch_npc(then, code, lookup, rows, engine);
                    if let Some(els) = els.as_deref() {
                        dead_branch_npc(els, code, lookup, rows, engine);
                    }
                    None
                }
                Verdict::NotConstant(span) | Verdict::Unresolved(span) => {
                    if engine.dot_shorthand_context(span.offset) {
                        None
                    } else {
                        rows.push(ElementRow::Site(span, code));
                        Some(Verdict::NotConstant(span))
                    }
                }
                Verdict::Fault(fault, span) => {
                    rows.push(ElementRow::Fault(span, fault));
                    Some(Verdict::Fault(fault, span))
                }
            }
        }
        Expr::Spread { operand, .. } => match eval(operand, lookup) {
            Verdict::Value(_) | Verdict::Constant => None,
            Verdict::NotConstant(span) | Verdict::Unresolved(span) => {
                if engine.dot_shorthand_context(span.offset) {
                    None
                } else {
                    rows.push(ElementRow::Site(span, code));
                    Some(Verdict::NotConstant(span))
                }
            }
            Verdict::Fault(fault, span) => {
                rows.push(ElementRow::Fault(span, fault));
                Some(Verdict::Fault(fault, span))
            }
        },
        _ => {
            // A plain element (and a `ForElement` `Faulted` leaf — its
            // fault reaches the verdict arm below).
            match eval(part, lookup) {
                Verdict::Value(_) | Verdict::Constant => {
                    clean.push(part.span);
                    // An element folding clean but closing an
                    // unresolved callee's invocation (`[Undefined()]`)
                    // reports the element's site default there like an
                    // eval-failed element.
                    engine.unresolved_call(part.span).map(|span| {
                        rows.push(ElementRow::Site(span, code));
                        Verdict::NotConstant(span)
                    })
                }
                Verdict::NotConstant(span) | Verdict::Unresolved(span) => {
                    // A leading-dot shorthand resolves against the
                    // context type upstream — the bare-name verdict is
                    // ambiguous.
                    if engine.dot_shorthand_context(span.offset) {
                        None
                    } else {
                        rows.push(ElementRow::Site(span, code));
                        Some(Verdict::NotConstant(span))
                    }
                }
                Verdict::Fault(fault, span) => {
                    rows.push(ElementRow::Fault(span, fault));
                    Some(Verdict::Fault(fault, span))
                }
            }
        }
    }
}

/// The dead-branch / unknown-condition not-potentially-constant scan
/// (upstream `_reportNotPotentialConstants`): each shape the const
/// grammar can never value reports the element code at its own leaf,
/// but the literal's own verdict is untouched (upstream skips the
/// branch's contribution — `elementCode` rows are sibling reports,
/// not the collection's failure).
fn dead_branch_npc(
    node: &crate::const_eval::Node,
    code: SemanticCode,
    lookup: &dyn ConstLookup,
    rows: &mut Vec<ElementRow>,
    engine: &Engine<'_>,
) {
    let mut spans = Vec::new();
    crate::const_eval::collect_not_potentially_constant(node, lookup, &mut spans);
    for span in spans {
        if !engine.dot_shorthand_context(span.offset) {
            rows.push(ElementRow::Site(span, code));
        }
    }
}

#[cfg(test)]
mod face_tests {
    use crate::collector::collect;
    use crate::diagnose::{FileReport, StrictModes, diagnose_with};
    use crate::library::ElementIndex;
    use crate::model::MicroKind;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
    use dartq_scanner::ScanConfig;
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// The one-library rig: `source` is the sole analyzed file of
    /// `package:t`, no imports (the same rig `ticket_1351_tests` runs).
    fn unit_report(source: &str) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
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
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        // `dart:core` sits in every library's import namespace upstream —
        // without the seed, a `const C(Object? a)` formals type reports a
        // phantom `undefined_class` next to the rows under test (the engine
        // driver's posture, ticket_2362's rig).
        let core_file = files.intern(Path::new("<dart:core>"));
        builder.seed_implicit_core(
            lib,
            core_file,
            &["Object", "String", "int", "bool", "double", "num"],
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut index = ElementIndex::default();
        index.insert(file, elements);
        diagnose_with(
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
        )
    }

    fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name.as_ref())
            .collect()
    }

    /// `!`/`-`/`~` on `null` and on a created primitive instance
    /// (`bool.fromEnvironment`'s `BoolState`): the `NullState` operator
    /// throws `constEvalThrowsException` where the assert gates used to
    /// fault the operand's type; a created bool/int/num of unknown value
    /// answers a valid constant (the drift `const _enable = !_releaseMode`
    /// shape). `created_state` keys on the container's interned name — a
    /// file-local `class bool`/`class int` models the core containers.
    #[test]
    fn unary_operator_states() {
        // `!null`/`~null`/`-null`: `NullState.logicalNot` throws directly;
        // the base `negated`/`bitNot` admit null through the assert and
        // then throw (analyzer source
        // pkg/analyzer/lib/src/dart/constant/value.dart:2830/:1967/:1724).
        let rep = unit_report("const a = !null; const b = -null; const c = ~null;");
        assert_eq!(
            code_names(&rep),
            [
                "const_eval_throws_exception",
                "const_eval_throws_exception",
                "const_eval_throws_exception",
            ],
            "{:?}",
            code_names(&rep)
        );
        // A created bool of unknown value: `!`, `&&`, `||`, `?:`, and the
        // interpolation gate all admit `BoolState`. A created int answers
        // `-`/`~`; a created non-state container still faults its gate.
        // The file-local `class int` shadows the dart:core `int` spelling,
        // so `i`'s static type is that declaration — which declares no
        // `unary-`/`~` — and the operator face reports both applications
        // (oracle-adjudicated: `dart analyze` on this fixture emits
        // `undefined_operator` at `-i` and `~i`).
        let rep = unit_report(
            "class V { const V(); }\n\
             class bool { final V? v; const bool.fromEnvironment(this.v); }\n\
             class int { final V? v; const int.fromEnvironment(this.v); }\n\
             class C { const C(); }\n\
             const f = bool.fromEnvironment(const V());\n\
             const a = !f;\n\
             const b = f && true;\n\
             const c = f || false;\n\
             const d = f ? 1 : 2;\n\
             const s = 'v${f}e';\n\
             const i = int.fromEnvironment(const V());\n\
             const n = -i;\n\
             const w = ~i;\n\
             const g = C();\n\
             const z = !g;",
        );
        assert_eq!(
            code_names(&rep),
            [
                "const_eval_type_bool",
                "undefined_operator",
                "undefined_operator",
            ],
            "{:?}",
            code_names(&rep)
        );
        // Negative controls: the operand-type gates still fault non-state
        // operands; `&&`'s non-bool right faults at the right operand. The
        // `-'a'`/`~'a'` applications additionally report `undefined_operator`
        // — `String` declares no `unary-`/`~` member (oracle probe 3.13.7 on
        // this exact shape: `CONST_EVAL_TYPE_NUM` + `UNDEFINED_OPERATOR` at
        // `-'a'`, `CONST_EVAL_TYPE_INT` + `UNDEFINED_OPERATOR` at `~'a'`,
        // and `CONST_EVAL_TYPE_BOOL` + `NON_BOOL_OPERAND` for `f && 1` —
        // the one-lib rig projects the last pair to a single
        // `non_bool_operand` at the `1` operand: `f`'s static type IS the
        // rig's own `bool` spelling, so only the right operand faults,
        // while the real-library oracle faults both operands).
        let rep = unit_report(
            "class V { const V(); }\n\
             class bool { final V? v; const bool.fromEnvironment(this.v); }\n\
             const f = bool.fromEnvironment(const V());\n\
             const a = !1;\n\
             const b = -'a';\n\
             const c = ~'a';\n\
             const d = f && 1;\n",
        );
        assert_eq!(
            code_names(&rep),
            [
                "const_eval_type_bool",
                "const_eval_type_num",
                "const_eval_type_int",
                "non_bool_operand",
                "const_eval_type_bool",
                "undefined_operator",
                "undefined_operator",
            ],
            "{:?}",
            code_names(&rep)
        );
    }

    /// The `ConstantVerifier` validity set at the initializer site (brief
    /// #2969): the shapes the oracle accepts stay silent — `identical`
    /// (a constant call, evaluation.dart `visitStaticMethodInvocation`),
    /// string interpolation of constants, unary `-` on a const, and `is`
    /// type tests (evaluation.dart:1106 `visitIsExpression` — no invalid
    /// arm of its own; zero oracle rows anchor at the operator). A
    /// locally declared `identical` stands in for the dart:core function
    /// the one-library rig does not import (the fold keys on the name).
    #[test]
    fn const_initializer_validity_pins() {
        // The silence asserts below pin THIS ticket's faces (the const
        // initializer/element codes) only — an unrelated operator-
        // resolution row on the same source belongs to another lane's
        // deliverable and must not fail this pin.
        let const_init = |rep: &FileReport| -> Vec<&str> {
            code_names(rep)
                .into_iter()
                .filter(|c| c.starts_with("const_") || c.starts_with("non_constant"))
                .collect()
        };
        let rep =
            unit_report("bool identical(a, b) { return true; }\nconst x = identical(1, 1);\n");
        assert!(
            const_init(&rep).is_empty(),
            "identical: {:?}",
            const_init(&rep)
        );
        let rep = unit_report("const y = 'a${2}';\n");
        assert!(
            const_init(&rep).is_empty(),
            "interp: {:?}",
            const_init(&rep)
        );
        // Unary `-` on a constant: valid (ConstantVerifier prefix arm).
        let rep = unit_report("const k = 1;\nconst z = -k;\n");
        assert!(const_init(&rep).is_empty(), "neg: {:?}", const_init(&rep));
        // `is` type tests defer as valid constants (both operands
        // constant): the capture folds the operator to an opaque leaf.
        let rep = unit_report(
            "class A { const A(); }\n\
             const k = 1;\n\
             const w = k is String;\n\
             const v = const A() is int;\n",
        );
        let rows: Vec<&str> = code_names(&rep)
            .into_iter()
            .filter(|c| c.starts_with("const_") || c.starts_with("non_constant"))
            .collect();
        assert!(rows.is_empty(), "{:?}", rows);
        // A non-const variable reference reports at the variable.
        let rep = unit_report("var v = 1;\nconst x = v;\n");
        let rows: Vec<(u32, u32)> = rep
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "const_initialized_with_non_constant_value")
            .map(|d| (d.range.offset, d.range.length))
            .collect();
        assert_eq!(rows, [(21, 1)], "{:?}", code_names(&rep));
        // A const-literal spread of a non-constant reports through the
        // element face at the spread operand, NOT through this ticket's
        // initializer code (oracle shape: `non_constant_map_element`
        // only — corpus case NonConstantMapElementTest/
        // test_spreadElement_notConst expects that single row, at the
        // `notConst` operand). The element face now emits it, so the pin
        // holds exactly that row and no const-init twin.
        let rep = unit_report(
            "void main() {\n\
             var notConst = {};\n\
             const {1: null, ...notConst};\n\
             }\n",
        );
        let rows: Vec<(&str, u32, u32)> = rep
            .diagnostics
            .iter()
            .filter(|d| {
                d.code.name.starts_with("const_") || d.code.name.starts_with("non_constant")
            })
            .map(|d| (d.code.name, d.range.offset, d.range.length))
            .collect();
        assert_eq!(
            rows,
            [("non_constant_map_element", 52, 8)],
            "spread: {rows:?}"
        );
    }

    /// A `new` creation in a const initializer reports at the whole
    /// `InstanceCreationExpression` — `new` through the argument list's
    /// closing paren (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/constant/evaluation.dart:1035,
    /// `visitInstanceCreationExpression`: `InvalidConstant.genericError(
    /// node: node)` on the creation node; corpus rows span `new C()` /
    /// `new Object()` whole). The capture folds the creation to its
    /// keyword token, so the emit site re-reads the extent — also for a
    /// dead conditional arm (`false ? 1 : new A()`) and an eager `??`
    /// right operand (the corpus `lazy_*` and `questionQuestion_null_
    /// invalid` shapes).
    #[test]
    fn new_creation_reports_whole_span() {
        let rep = unit_report(
            "class A { const A(); }\n\
             const a = new A();\n\
             const b = false ? 1 : new A();\n\
             const c = null ?? new A();\n",
        );
        let rows: Vec<(u32, u32)> = rep
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "const_initialized_with_non_constant_value")
            .map(|d| (d.range.offset, d.range.length))
            .collect();
        assert_eq!(rows, [(33, 7), (64, 7), (91, 7)], "{:?}", code_names(&rep));
    }

    /// `constantContext2` (ast.dart:17927-17983) returns `null` inside a
    /// `this(...)`/`this.named(...)`/`super(...)`/`super.named(...)`
    /// argument list — a literal there is never `isConst`, so its element
    /// faces do not run; the init-invocation arglist close's
    /// not-potentially-constant scan is the only verdict (the riverpod
    /// `AsyncData` `this._((value, kind: kind, ...))` shape).
    #[test]
    fn elements_silent_inside_init_invocation_arguments() {
        let rep = unit_report(
            "class V { const V(); }\n\
             class A {\n\
             final V? v, e, l;\n\
             const A._(this.v, {required this.e, required this.l});\n\
             const A(V? value, V? kind) : this._((value, kind: kind, source: null), e: [kind], l: {value});\n\
             }",
        );
        assert_eq!(
            code_names(&rep),
            Vec::<&str>::new(),
            "{:?}",
            code_names(&rep)
        );
        // A `super(...)` invocation's argument list suppresses the same
        // faces.
        let rep = unit_report(
            "class V { const V(); }\n\
             class B { const B([V? a, V? b]); }\n\
             class C extends B { const C(V? x) : super((x,), {x: x}); }",
        );
        assert_eq!(
            code_names(&rep),
            Vec::<&str>::new(),
            "{:?}",
            code_names(&rep)
        );
        // Negative control: the same literals in a const initializer keep
        // the element faces.
        let rep = unit_report("void m() { var l = 1; const a = (l,); }");
        assert_eq!(
            code_names(&rep),
            [
                "unused_local_variable",
                "non_constant_record_field",
                "const_initialized_with_non_constant_value",
            ],
            "{:?}",
            code_names(&rep)
        );
    }

    /// Dot shorthands in const contexts (`.new(...)`, `.named(...)`,
    /// `.name`): the shorthand resolves against the context type upstream
    /// (`visitDotShorthandConstructorInvocation2` /
    /// `visitDotShorthandNameExpression`, evaluation.dart:880-916) — the
    /// capture folds the selector to a bare name and the verdict
    /// under-reports the resolution it cannot perform, so the valid
    /// shorthands stay silent beside the genuinely bare misses.
    #[test]
    fn dot_shorthands_stay_silent() {
        let rep = unit_report(
            "class V { const V(); }\n\
             class P {\n\
             final V? a, b;\n\
             const P({required this.a, required this.b});\n\
             const P._(this.a, this.b);\n\
             static const P empty = .new(a: [], b: []);\n\
             static const P origin = ._([], null);\n\
             }\n\
             enum E { low, high }\n\
             const E picked = .low;",
        );
        assert_eq!(
            code_names(&rep),
            Vec::<&str>::new(),
            "{:?}",
            code_names(&rep)
        );
        // `const .named(...)` — the explicit-const spelling takes the same
        // shorthand path.
        let rep = unit_report(
            "class V { const V(); }\n\
             class Q {\n\
             final V? x;\n\
             const Q._(this.x);\n\
             const Q.named() : this._(const V());\n\
             }\n\
             const q = const Q.named();\n\
             class R { static const Q r = .named(); }",
        );
        assert_eq!(
            code_names(&rep),
            Vec::<&str>::new(),
            "{:?}",
            code_names(&rep)
        );
        // Negative controls: a bare missing callee and a bare missing name
        // keep their declaration-root verdicts beside the resolution rows.
        let rep = unit_report("const a = Missing(); const b = missing;");
        assert_eq!(
            code_names(&rep),
            [
                "const_initialized_with_non_constant_value",
                "const_initialized_with_non_constant_value",
                "undefined_function",
                "undefined_identifier",
            ],
            "{:?}",
            code_names(&rep)
        );
    }

    /// Consecutive named arguments of a const creation flush per argument
    /// (the capture's named event owns its value's flush): the values
    /// never pool into the compound-recovery node the argument faces
    /// would read as non-constant. `P(a: 1, b: 2)` folds for the oracle —
    /// silent here too — while the single-named and positional-only
    /// spellings keep their established faces.
    #[test]
    fn consecutive_named_args_of_const_creation_stay_const() {
        let rep = unit_report(
            "class V { const V(); }\n\
             class P {\n\
             final V? a, b;\n\
             const P({this.a, this.b});\n\
             }\n\
             const p1 = P(a: V(), b: V());\n\
             const p2 = P(a: V());\n\
             const p3 = P(b: V());",
        );
        assert_eq!(
            code_names(&rep),
            Vec::<&str>::new(),
            "{:?}",
            code_names(&rep)
        );
    }

    /// #2289: `genericError`'s ArgumentList-parent rule (upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/constant/value.dart): only a
    /// failing node that is a DIRECT child of the const constructor's
    /// argument list takes `const_with_non_constant_argument` — a bare
    /// identifier spanning the whole argument. Named values, member
    /// selectors, parenthesized operands and composites collect
    /// `invalid_constant` at the leaf instead. Enum-constant arguments
    /// keep `const_with_non_constant_argument` for every non-allow-listed
    /// leaf (constant_verifier.dart `_validateConstantArguments` passes it
    /// as the default code).
    #[test]
    fn const_arg_code_parent_rule() {
        // `C(missing)` — direct bare-ident arg keeps the argument face;
        // `C(missing.name)` / `C(missing + 1)` / `C(!missing)` /
        // `C((missing))` nest the leaf and take `invalid_constant`.
        let rep = unit_report(
            "class C { const C(Object? a); }\n\
             const a = C(missing);\n\
             const b = C(missing.name);\n\
             const c = C(missing + 1);\n\
             const d = C(!missing);\n\
             const e = C((missing));",
        );
        let count = |name: &str| {
            rep.diagnostics
                .iter()
                .filter(|d| d.code.name == name)
                .count()
        };
        assert_eq!(
            count("const_with_non_constant_argument"),
            1,
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(count("invalid_constant"), 4, "{:?}", code_names(&rep));
        // Every `missing` leaf reports exactly one resolution row (the
        // oracle probe: five `UNDEFINED_IDENTIFIER`s — the prefixed head's
        // `undefined_class`/`undefined_identifier` pick is the resolution
        // lane's, owned by its own tests).
        assert_eq!(
            count("undefined_identifier") + count("undefined_class"),
            5,
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            count("const_initialized_with_non_constant_value"),
            0,
            "{:?}",
            code_names(&rep)
        );
        // A genuinely non-constant leaf: direct bare-ident keeps the
        // argument face beside the declaration's `const_init`; composites
        // flip to `invalid_constant` (the declaration verdict still fires
        // — the leaf is known-invalid, not unknown).
        let rep = unit_report(
            "class C { const C(Object? a); }\n\
             class D { const D({Object? name}); }\n\
             void m() {\n\
             var v = 2;\n\
             const a = C(v);\n\
             const b = C(v + 1);\n\
             const c = D(name: v);\n\
             }",
        );
        let count = |name: &str| {
            rep.diagnostics
                .iter()
                .filter(|d| d.code.name == name)
                .count()
        };
        assert_eq!(
            count("const_with_non_constant_argument"),
            1,
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(count("invalid_constant"), 2, "{:?}", code_names(&rep));
        assert_eq!(
            count("const_initialized_with_non_constant_value"),
            3,
            "{:?}",
            code_names(&rep)
        );
        // The drift shape: an unresolved prefixed member as a positional
        // argument collects `invalid_constant`, not the argument face. The
        // head's own resolution row (`UNDEFINED_IDENTIFIER` per the oracle
        // probe) belongs to the resolution lane's tests.
        let rep = unit_report(
            "class C { const C(Object? a); }\n\
             const x = C(DriftSqlType.dateTime);",
        );
        let count = |name: &str| {
            rep.diagnostics
                .iter()
                .filter(|d| d.code.name == name)
                .count()
        };
        assert_eq!(count("invalid_constant"), 1, "{:?}", code_names(&rep));
        assert_eq!(
            count("const_with_non_constant_argument"),
            0,
            "{:?}",
            code_names(&rep)
        );
        // Enum-constant arguments keep `const_with_non_constant_argument`
        // for the same leaf shapes (`_validateConstantArguments`'s
        // default code), including a member-selector leaf.
        let rep = unit_report(
            "enum E {\n\
             a(Missing.name),\n\
             b(missing);\n\
             const E(Object? a);\n\
             }",
        );
        let count = |name: &str| {
            rep.diagnostics
                .iter()
                .filter(|d| d.code.name == name)
                .count()
        };
        assert_eq!(
            count("const_with_non_constant_argument"),
            2,
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(count("invalid_constant"), 0, "{:?}", code_names(&rep));
        assert_eq!(count("undefined_identifier"), 2, "{:?}", code_names(&rep));
    }

    /// #2289: one `const_initialized_with_non_constant_value` per
    /// *initializer*, keyed by declaration site — two `const event`
    /// declarations in sibling branches each report their own row (the
    /// name alone collided them before). The callee's own resolution rows
    /// (`undefined_*` family) belong to the resolution lane's tests.
    #[test]
    fn same_named_const_initializers_each_report() {
        let rep = unit_report(
            "class C {\n\
             void start() {\n\
             if (true) {\n\
             const event = Missing<int>('connect');\n\
             event;\n\
             } else {\n\
             const event = Missing<int>('message');\n\
             event;\n\
             }\n\
             }\n\
             }",
        );
        let inits = rep
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "const_initialized_with_non_constant_value")
            .map(|d| d.range.offset)
            .collect::<Vec<_>>();
        assert_eq!(inits.len(), 2, "{:?}", code_names(&rep));
        assert_ne!(inits[0], inits[1], "{:?}", rep.diagnostics);
        // A single initializer still reports once: two failing creations
        // (one unresolved callee per list element) collapse to the first
        // row, anchored at the first failing node.
        let rep = unit_report("const x = [Missing(), Missing2()];");
        let inits = rep
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "const_initialized_with_non_constant_value")
            .count();
        assert_eq!(inits, 1, "{:?}", code_names(&rep));
    }

    /// #2289: an unresolved-callee creation's failure face keys on WHERE
    /// it sits in its initializer's evaluation. Inside a resolved const
    /// creation's arguments each such creation reports `invalid_constant`
    /// at its own span (`C(Missing(), Missing2())` reports both — the
    /// oracle's `_valueOf` reports every `isUnresolved` argument error);
    /// a root creation and a root literal's first failing element take
    /// the variable face instead; a failure that aborted the evaluation
    /// first (`C(v, Missing())`'s known non-constant `v`) silences the
    /// later creation; `Missing(Missing2())` anchors the variable face at
    /// the whole root creation and leaves the argument silent.
    #[test]
    fn unresolved_creation_face_by_evaluation_site() {
        let count = |rep: &FileReport, name: &str| -> usize {
            rep.diagnostics
                .iter()
                .filter(|d| d.code.name == name)
                .count()
        };
        // Each nested argument reports its own `invalid_constant`.
        let rep = unit_report(
            "class C { const C(Object? a, [Object? b]); }\n\
             const x = C(Missing(), Missing2());",
        );
        assert_eq!(count(&rep, "invalid_constant"), 2, "{:?}", code_names(&rep));
        assert_eq!(
            count(&rep, "const_initialized_with_non_constant_value"),
            0,
            "{:?}",
            code_names(&rep)
        );
        // A known non-constant argument aborts the evaluation first: the
        // later unresolved creation stays silent.
        let rep = unit_report(
            "class C { const C(Object? a, [Object? b]); }\n\
             void m() {\n\
             var v = 2;\n\
             const x = C(v, Missing());\n\
             }",
        );
        assert_eq!(count(&rep, "invalid_constant"), 0, "{:?}", code_names(&rep));
        assert_eq!(
            count(&rep, "const_with_non_constant_argument"),
            1,
            "{:?}",
            code_names(&rep)
        );
        // Grouping parens enclose no evaluation: the operand still reports
        // inside the resolved creation.
        let rep = unit_report(
            "class C { const C(Object? a); }\n\
             const x = C(-(Missing()));",
        );
        assert_eq!(count(&rep, "invalid_constant"), 1, "{:?}", code_names(&rep));
        // Root literal: the variable face anchors at the first failing
        // element only; a root creation anchors at its whole span and
        // silences its argument.
        let rep = unit_report("const x = [Missing(), Missing2()];");
        assert_eq!(
            count(&rep, "const_initialized_with_non_constant_value"),
            1,
            "{:?}",
            code_names(&rep)
        );
        let first = rep
            .diagnostics
            .iter()
            .find(|d| d.code.name == "const_initialized_with_non_constant_value")
            .map(|d| d.range.offset);
        assert_eq!(first, Some(11), "{:?}", rep.diagnostics);
        let rep = unit_report("const x = Missing(Missing2());");
        let roots: Vec<_> = rep
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "const_initialized_with_non_constant_value")
            .map(|d| (d.range.offset, d.range.length))
            .collect();
        assert_eq!(roots, vec![(10, 19)], "{:?}", rep.diagnostics);
    }

    /// #2289: a known non-constant argument pairs its argument face with
    /// the variable face at the same leaf (`C(v)` reports
    /// `const_with_non_constant_argument` +
    /// `const_initialized_with_non_constant_value`, both at `v`) and aborts
    /// the evaluation — the later unresolved creation stays silent — while
    /// a failed const's later read is silent (the oracle's
    /// `avoidReporting`): `const h = C(g)` over a failed `g` reports
    /// nothing at `h`.
    #[test]
    fn const_arg_failure_faces_by_leaf_shape() {
        let count = |rep: &FileReport, name: &str| -> usize {
            rep.diagnostics
                .iter()
                .filter(|d| d.code.name == name)
                .count()
        };
        let rep = unit_report(
            "class C { const C(Object? a, [Object? b]); }\n\
             void m() {\n\
             var v = 2;\n\
             const x = C(v, Missing());\n\
             }",
        );
        assert_eq!(count(&rep, "invalid_constant"), 0, "{:?}", code_names(&rep));
        assert_eq!(
            count(&rep, "const_with_non_constant_argument"),
            1,
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(
            count(&rep, "const_initialized_with_non_constant_value"),
            1,
            "{:?}",
            code_names(&rep)
        );
        let rep = unit_report(
            "class C { const C(Object? a); }\n\
             const g = Missing2<int>('x');\n\
             const h = C(g);",
        );
        assert_eq!(
            count(&rep, "const_with_non_constant_argument"),
            0,
            "{:?}",
            code_names(&rep)
        );
        assert_eq!(count(&rep, "invalid_constant"), 0, "{:?}", code_names(&rep));
        // `g`'s own declaration still carries the variable face.
        assert_eq!(
            count(&rep, "const_initialized_with_non_constant_value"),
            1,
            "{:?}",
            code_names(&rep)
        );
    }
}

#[cfg(test)]
#[path = "ticket_3163_tests.rs"]
mod ticket_3163_tests;

#[cfg(test)]
#[path = "ticket_3842_tests.rs"]
mod ticket_3842_tests;
