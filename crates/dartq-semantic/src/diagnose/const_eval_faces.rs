use super::*;
use crate::const_eval::CreatedState;

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
                None => {}
            }
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
                _ => IdentVerdict::Constant,
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
                if self.guard.ignore(None, name) {
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
    /// provable non-const set (in-file, declaration order).
    pub fn const_decl_name(&mut self, name: StringId, field_context: bool) {
        if self.const_group {
            self.const_group_names.push(name);
        } else if field_context && self.frames.is_empty() {
            self.nonconst_top_level.insert(name);
        }
    }

    pub fn disarm_const_group(&mut self) {
        self.const_group = false;
        self.const_group_names.clear();
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
        self.pending_init_names.push(name);
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
        let Some(name) = self.pending_init_names.pop() else {
            return;
        };
        if !self.const_capture.open() {
            return;
        }
        let root = self.const_capture.close_initializer();
        let verdict = root
            .as_ref()
            .map_or(Verdict::Constant, |node| eval(node, self));
        self.resolve_const_verdict(&verdict);
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
                self.bind_var_type(name, folded);
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
        } else if let Some(scope) = self.const_scopes.last_mut() {
            scope.remove(&name);
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
                self.emit(
                    SemanticCode::ConstInitializedWithNonConstantValue,
                    *span,
                    "",
                    "",
                );
            }
            // An unresolved root (`const a = x`) reports like a plain
            // non-constancy: the oracle's evaluation of the unresolvable
            // reference is known-invalid at the variable. A leading-dot
            // shorthand (`.blue` — `DotShorthandNameExpression` resolves
            // against the context type) reads unresolved through the
            // bare-name path the capture folds it to: the oracle answers
            // `invalid_constant` only for the member the type cannot
            // resolve, and the valid shorthands outnumber it — the
            // under-report verdict stays silent.
            Verdict::Unresolved(span) => {
                if !self.dot_shorthand_context(span.offset) {
                    self.emit(
                        SemanticCode::ConstInitializedWithNonConstantValue,
                        *span,
                        "",
                        "",
                    );
                }
            }
            Verdict::Fault(fault, span) => self.emit_const_fault(*fault, *span),
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
        };
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
            ConstantValue::Type(_) | ConstantValue::Instance(_) => Ty::UNKNOWN,
        }
    }

    /// The folded declared type of an in-file const top-level variable
    /// (the type-folding face of #279: `const x = 1;` folds `x` to
    /// `int`).
    pub fn const_top_level_ty(&self, name: StringId) -> Option<Ty> {
        match self.const_scopes.first()?.get(&name)? {
            ConstBinding::Value(v) => Some(self.folded_ty(v)),
            ConstBinding::InProgress => None,
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
            return false;
        };
        let text = self
            .source
            .get(span.offset as usize..span.end() as usize)
            .unwrap_or("");
        let head = self.strings.resolve(container);
        let selector = text
            .strip_prefix(head)
            .and_then(|rest| rest.strip_prefix('.'))
            .map(|rest| {
                rest.split(['(', ' ', '?', '\n', '\t'])
                    .next()
                    .unwrap_or("")
            })
            .unwrap_or("");
        if selector.is_empty() || selector == "new" {
            return true;
        }
        decl.constructors.contains(&self.intern(selector))
    }

    /// The callee classification a closed bare-identifier call resolves
    /// to (`evaluation.dart` `_invalidConstantForMethodInvocation`). The
    /// `&mut` receiver stages the `C.named(` implicit-const creation the
    /// qualified-selector split detects — the tail pass needs the
    /// whole-file const-ctor set before it can verdict.
    pub(crate) fn classify_call(&mut self, name: StringId) -> CallClass {
        // The dot-shorthand constructor invocation (`.new(...)` /
        // `.named(...)`): a bare selector callee reaches here only through
        // the shorthand's `_tryRewriteNewToIdentifier` identifier event —
        // `C.new(...)` parses the constructor-reference path and
        // `x.new(...)` folds its member before the group opens. The oracle
        // resolves the shorthand against the context type
        // (`visitDotShorthandConstructorInvocation2`, evaluation.dart
        // :880-916) — a resolution this pass cannot perform — so the
        // under-report verdict is the valid-creation face: its arguments
        // still answer the const-argument check, and a static-method
        // shorthand (`.create()` — the oracle's `constEvalMethodInvocation`
        // when it reaches a call) stays silent beside it.
        if let Some(offset) = self
            .invokes
            .last()
            .and_then(|slot| *slot)
            .and_then(|index| self.rows.get(index))
            .map(|row| row.range.offset)
            && self.dot_shorthand_context(offset)
        {
            return CallClass::ConstCtor;
        }
        if self.const_ctor_target(name) {
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
        let recv = self
            .invokes
            .last()
            .and_then(|slot| *slot)
            .filter(|&callee| {
                self.rows
                    .get(callee)
                    .is_some_and(|row| row.qualified)
            })
            .and_then(|callee| {
                callee
                    .checked_sub(1)
                    .and_then(|index| self.rows.get(index))
            });
        if let Some(recv) = recv {
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
                    let is_const_ctor = decl_type
                        && {
                            let container = self.strings.resolve(recv.name);
                            let selector = self.strings.resolve(name);
                            CORE_CONST_CTOR.iter().any(|(c, sels)| {
                                *c == container && sels.iter().any(|s| *s == selector)
                            })
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
                    if matches!(
                        target,
                        RefTarget::Local
                            | RefTarget::This
                            | RefTarget::Member { .. }
                            | RefTarget::Opaque
                    ) || (matches!(target, RefTarget::Decl { .. })
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
        if self.frames.iter().any(|f| f.locals.contains(&name))
            || self.const_scopes.iter().any(|s| s.contains_key(&name))
        {
            return CallClass::Invocable;
        }
        match self.top_level_decl(name).map(|d| d.kind) {
            Some(MicroKind::TopLevelFn | MicroKind::TopLevelVar) => CallClass::Invocable,
            _ => CallClass::Unknown,
        }
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
            let verdict = eval(arg, self);
            match verdict {
                Verdict::Value(_) | Verdict::Constant => {}
                Verdict::NotConstant(span) => {
                    self.emit(SemanticCode::ConstWithNonConstantArgument, span, "", "");
                    if let Some(init) = self.pending_init_names.last().copied() {
                        self.open_const_creations
                            .push((init, span, ConstCreationEntry::ArgFailure));
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
                    if !self.dot_shorthand_context(span.offset) {
                        self.emit(SemanticCode::ConstWithNonConstantArgument, span, "", "");
                    }
                }
                Verdict::Fault(fault, span) => {
                    self.emit_const_fault(fault, span);
                    return;
                }
            }
        }
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
        let Some(inits) = self.ctor_inits.get(&(container, container)) else {
            return;
        };
        if inits.is_empty() {
            return;
        }
        // Positional formals bind the arguments in order (the capture
        // drops named-argument labels — a named-only call binds its
        // values positionally, which only ever moves the walk toward
        // silence, the under-report direction).
        let mut env: FxHashMap<StringId, CtorBinding> = FxHashMap::default();
        let formals = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.ctor_formals.get(&container));
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

    // —— #749: non_const_call_to_literal_constructor ————————————————————
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
    pub fn check_const_elements(
        &mut self,
        code: SemanticCode,
        parts: &[crate::const_eval::Node],
    ) -> Option<SourceRange> {
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
        let mut failed = None;
        for part in parts {
            match eval(part, self) {
                Verdict::Value(_) | Verdict::Constant => {}
                Verdict::NotConstant(span) | Verdict::Unresolved(span) => {
                    // A leading-dot shorthand resolves against the context
                    // type upstream — the bare-name verdict is ambiguous.
                    if self.dot_shorthand_context(span.offset) {
                        continue;
                    }
                    let info = code.code_info();
                    let repeat = self
                        .report
                        .diagnostics
                        .iter()
                        .rev()
                        .any(|d| d.range == span && d.code == info);
                    if !repeat {
                        self.emit(code, span, "", "");
                    }
                    failed.get_or_insert(span);
                }
                Verdict::Fault(fault, span) => {
                    self.emit_const_fault(fault, span);
                    failed.get_or_insert(span);
                }
            }
        }
        failed
    }

    /// Whether an open argument level's callee is a constructor-initializer
    /// invocation head — a bare `this(`/`super(` keyword receiver, the
    /// `this.named(` member shape, or the folded `super.named(` selector
    /// ([`Engine::pending_init_super`], the same shapes
    /// [`Engine::init_super_callee`] decides at the arglist close).
    fn init_invocation_level(&self, level: &ArgLevel) -> bool {
        let Some(head) = level
            .callee_row
            .and_then(|index| self.rows.get(index))
        else {
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
                                RowShape::Receiver(
                                    Receiver::This { .. } | Receiver::Super { .. }
                                )
                            )
                        })
                });
                on_keyword_receiver
                    || self.init_rows.contains_key(&head.range.offset)
                    || self
                        .pending_init_super
                        .iter()
                        .rev()
                        .any(|c| c.invoked && c.head && c.selector_offset == head.range.offset)
            }
        }
    }

    /// Pushes a closed const literal's value onto the capture: its
    /// instance when every element was constant, else a non-constant leaf
    /// anchored at the first failing element (so the enclosing
    /// initializer's verdict reports there). Inside a creation's argument
    /// list the failure stays local — the oracle's argument faces there
    /// are `invalid_constant`, which this face does not model.
    pub(crate) fn push_literal_value(
        &mut self,
        failed: Option<SourceRange>,
        end: SourceRange,
        kind: crate::const_eval::InstanceKind,
    ) {
        match failed {
            Some(span) if !self.const_capture.in_arguments() => {
                self.const_capture.push_non_constant(span);
            }
            _ => self
                .const_capture
                .push_value(end, ConstantValue::Instance(kind)),
        }
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
            SetOrMap::Map(entries) => (
                SemanticCode::EqualKeysInMap,
                entries.into_iter().map(|(key, _)| key).collect(),
            ),
        };
        let mut seen: Vec<ConstantValue> = Vec::new();
        for part in &parts {
            if let Verdict::Value(v) = eval(part, self) {
                if seen.contains(&v) {
                    self.emit_with_correction(code, part.span);
                } else {
                    seen.push(v);
                }
            }
        }
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
            .get(
                self.byte_of(begin.offset) + 1
                    ..self.byte_of(begin.end()).saturating_sub(1)
            )
            .unwrap_or("");
        self.const_capture
            .end_literal_string(0, begin, end, ConstantValue::Str(text.into()));
    }

    pub fn const_string_end(&mut self, count: usize, begin: SourceRange, end: SourceRange) {
        self.const_capture
            .end_literal_string(count, begin, end, ConstantValue::Str("".into()));
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
            ["const_eval_type_bool"],
            "{:?}",
            code_names(&rep)
        );
        // Negative controls: the operand-type gates still fault non-state
        // operands; `&&`'s non-bool right faults at the right operand.
        let rep = unit_report(
            "class V { const V(); }\n\
             class bool { final V? v; const bool.fromEnvironment(this.v); }\n\
             const f = bool.fromEnvironment(const V());\n\
             const a = !1;\n\
             const b = -'a';\n\
             const c = ~'a';\n\
             const d = f && 1;",
        );
        assert_eq!(
            code_names(&rep),
            [
                "const_eval_type_bool",
                "const_eval_type_num",
                "const_eval_type_int",
                "const_eval_type_bool",
            ],
            "{:?}",
            code_names(&rep)
        );
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
        assert_eq!(code_names(&rep), Vec::<&str>::new(), "{:?}", code_names(&rep));
        // A `super(...)` invocation's argument list suppresses the same
        // faces.
        let rep = unit_report(
            "class V { const V(); }\n\
             class B { const B([V? a, V? b]); }\n\
             class C extends B { const C(V? x) : super((x,), {x: x}); }",
        );
        assert_eq!(code_names(&rep), Vec::<&str>::new(), "{:?}", code_names(&rep));
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
}
