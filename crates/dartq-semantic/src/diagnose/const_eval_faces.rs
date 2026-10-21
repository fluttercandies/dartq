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
                // A failed const binding never re-reports: the declaring
                // initializer carried the error (`avoidReporting`), so a
                // later read folds as a deferring constant (`const h =
                // C(g)` over a failed `g` stays silent).
                Some(ConstBinding::Failed) => return IdentVerdict::Constant,
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
            // A qualified creation arrives named for its SELECTOR
            // (`C.named(` closes with the selector as the callee), which
            // resolves to no container: the written head at the span is
            // the container the verifier answers.
            let text = self
                .source
                .get(span.offset as usize..span.end() as usize)
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
            .get(span.offset as usize..span.end() as usize)
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
            .get(span.offset as usize..span.end() as usize)
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
                .map(|rest| {
                    rest.split(['(', ' ', '?', '\n', '\t']).next().unwrap_or("")
                })
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
        before.is_ascii_alphanumeric() || before == b'_'
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
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
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
        // The annotation window's own argument list closes through the
        // same event (#2261): its head never became a capture operand (a
        // metadata reference is not an expression), so the generic close
        // drops the level unreported — the drain takes it here, ahead of
        // the capture's own `end_arguments`, while the engine's argument
        // level still holds the labels.
        self.check_annotation_arguments(end);
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
    fn ctor_init_formals(&self) -> Vec<StringId> {
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
                self.index
                    .get(self.file)
                    .and_then(|f| f.container(container))?
                    .ctor_formals
                    .get(&ctor_name)
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
        let verdicts: Vec<Verdict> = if formals.is_empty() {
            parts.iter().map(|part| eval(part, self)).collect()
        } else {
            let mut env: FxHashMap<StringId, CtorBinding> = FxHashMap::default();
            for name in formals {
                env.insert(name, CtorBinding::Opaque);
            }
            let scope = CtorScope {
                engine: self,
                env: &env,
            };
            parts.iter().map(|part| eval(part, &scope)).collect()
        };
        let mut failed = None;
        for (part, verdict) in parts.iter().zip(verdicts) {
            let failed_at = match verdict {
                // An element folding clean but closing an unresolved
                // callee's invocation (`[Undefined()]`): the oracle's
                // evaluation is known-invalid at the invocation
                // (`undefined_function` is not override-listed,
                // `constant_verifier.dart` `_reportError`), so the element's
                // site default fires there like an eval-failed element.
                Verdict::Value(_) | Verdict::Constant => self.unresolved_call(part.span),
                Verdict::NotConstant(span) | Verdict::Unresolved(span) => {
                    // A leading-dot shorthand resolves against the context
                    // type upstream — the bare-name verdict is ambiguous.
                    if self.dot_shorthand_context(span.offset) {
                        None
                    } else {
                        Some(span)
                    }
                }
                Verdict::Fault(fault, span) => {
                    self.emit_const_fault(fault, span);
                    failed.get_or_insert(span);
                    None
                }
            };
            // An element folding clean can still carry unresolved leaves
            // deeper inside a resolved const creation's arguments (the
            // corpus's `GenerateNodeProperty('element', type:
            // ExtensionElementImpl)`): the oracle's creation evaluation
            // reports the leaf itself and nothing above it (#2261).
            self.check_invalid_constant_leaves(part.span);
            if let Some(span) = failed_at {
                let info = code.code_info();
                let repeat = self
                    .report
                    .diagnostics
                    .iter()
                    .rev()
                    .any(|d| d.range == span && d.code == info);
                if !repeat {
                    if self.leaf_in_resolved_creation(span.offset) {
                        // The failing leaf sits inside a resolved const
                        // creation's argument list: the oracle reports the
                        // leaf directly there and the literal's element
                        // face never substitutes — the positional arm's
                        // `const_with_non_constant_argument` already owns
                        // the span, everything else is `invalid_constant`.
                        if !self.const_arg_row_at(span) {
                            self.emit(SemanticCode::InvalidConstant, span, "", "");
                        }
                    } else {
                        self.emit(code, span, "", "");
                        // A leaf under an open annotation window joins the
                        // argument faces' pairing log (#2261); a set/map
                        // element's own face rides retractable — the
                        // type-declaration hosts' evaluation owns the
                        // report there, decided at the annotation's close
                        // (the element check has no host view).
                        let retractable = matches!(
                            code,
                            SemanticCode::NonConstantSetElement
                                | SemanticCode::NonConstantMapValue
                                | SemanticCode::NonConstantMapKey
                        ) && self.span_ends_call(span);
                        self.const_capture.note_annotation_leaf(span, retractable);
                    }
                }
                failed.get_or_insert(span);
            }
        }
        failed
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
    fn init_invocation_level(&self, level: &ArgLevel) -> bool {
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
    /// structural value when every element was constant and known
    /// (`payload` — the duplicate twins compare by value), the
    /// opaque-kind instance when any element's value is unknown, else a
    /// non-constant leaf anchored at the first failing element (so the
    /// enclosing initializer's verdict reports there). Inside a
    /// creation's argument list the failure stays local — the oracle's
    /// argument faces there are `invalid_constant`, which this face does
    /// not model.
    pub(crate) fn push_literal_value(
        &mut self,
        failed: Option<SourceRange>,
        end: SourceRange,
        kind: crate::const_eval::InstanceKind,
        payload: Option<ConstantValue>,
    ) {
        // The ERROR-tier duplicate twins of a const literal (#2261): the
        // capture staged this literal's parts at its close; the oracle's
        // `visitSetOrMapLiteral` checks every `isConst` literal's keys
        // and elements — after the element faces, at the value push.
        if let Some((shape, _)) = self.const_capture.take_staged_dup(kind) {
            self.check_const_duplicates(shape);
        }
        match failed {
            Some(span) if !self.const_capture.in_arguments() => {
                self.const_capture.push_non_constant(span);
            }
            _ => self
                .const_capture
                .push_value(end, payload.unwrap_or(ConstantValue::Instance(kind))),
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

    /// The map literal's structural value: every entry's key/value pair
    /// in order, `None` when any side is of unknown value (the
    /// opaque-kind fallback).
    pub(crate) fn map_payload(
        &mut self,
        entries: &[(crate::const_eval::Node, crate::const_eval::Node)],
    ) -> Option<ConstantValue> {
        entries
            .iter()
            .map(|(key, value)| match (eval(key, self), eval(value, self)) {
                (Verdict::Value(key), Verdict::Value(value)) => Some((key, value)),
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
        let mut pairs: Vec<(StringId, ConstantValue)> =
            names.iter().copied().zip(tail).collect();
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
            SetOrMap::Map(entries) => (
                SemanticCode::EqualKeysInConstMap,
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
    fn check_annotation_arguments(&mut self, close: SourceRange) {
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
        let Some((head_range, head_name, selector)) = self.annotation_head_before(close) else {
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
        // The argument-count faces: a deficit anchors at the annotation
        // head, a surplus at the first extra positional argument (the
        // probed `@P()` / `@P(1, 2, 3)` pairings).
        let given = named.iter().filter(|flag| !**flag).count();
        // Required-ness derives from the default value, not the stored
        // flag alone: a required positional never carries one and an
        // optional always does, while initializing formals (`this.a`)
        // lose the parser's required classification upstream — the flag
        // stays in the disjunction for the plain-formal worlds.
        let required = formals
            .positional
            .iter()
            .filter(|f| f.required || f.default_value.is_none())
            .count();
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
            self.emit_message_with_correction(code, head_range, message);
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
