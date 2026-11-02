//! The pre-3.0 (legacy) switch verifier: the rows upstream's
//! `ConstantVerifier`/`ErrorVerifier`/shared `TypeAnalyzer` emit only when
//! the file's language version leaves `Feature.patterns` disabled
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/constant/constant_verifier.dart:1145-1182 —
//! `_validateSwitchStatement_nullSafety` gated on
//! `!featureSet.isEnabled(Feature.patterns)` — plus
//! pkg/analyzer/lib/src/generated/error_verifier.dart's
//! `_checkForMissingEnumConstantInSwitch` and
//! pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart's
//! `caseExpressionTypeMismatch` arm). A patterns-era switch takes the
//! exhaustiveness faces instead; this whole pass never runs there.
//!
//! The engine records each legacy case head's evaluated constant into
//! [`crate::switch_tracker::SwitchCtx::legacy_cases`] at
//! `end_case_expression`; [`Engine::report_legacy_switch`] consumes the
//! closed context's entries.

use super::*;
use crate::switch_tracker::SwitchCtx;
use dartq_source::StringId;

impl<'a> Engine<'a> {
    /// The deferred-import face of a legacy case head (upstream
    /// `evaluation.dart`'s `_getDeferredLibraryError` →
    /// `nonConstantCaseExpressionFromDeferredLibrary`): a constant
    /// reference through a `deferred` import's prefix fails const-eval
    /// at the identifier, and the row anchors at the prefix token. The
    /// captured expression tree is walked for the outermost identifier
    /// that names a deferred prefix of the diagnosing library; its span
    /// is the emit site (the listener emits the row at the head's close,
    /// before the generic non-constant arm could anchor the whole
    /// expression instead). `None` when no deferred prefix is named —
    /// the common path, no import-edge walk at all.
    pub(crate) fn legacy_deferred_span(&self, node: &crate::const_eval::Node) -> Option<SourceRange> {
        // A deferred library's constants are reachable only through its
        // prefix — `d.c` is `Member{recv:Ident(d)}`; a bare identifier can
        // never spell a deferred reference (deferred imports bind no
        // unprefixed names).
        fn named_prefix(node: &crate::const_eval::Node) -> Option<(StringId, SourceRange)> {
            if let Expr::Member { recv, .. } = &node.expr
                && let Expr::Ident(prefix) = &recv.expr
            {
                return Some((*prefix, recv.span));
            }
            None
        }
        fn walk(node: &crate::const_eval::Node, hit: &mut Option<(StringId, SourceRange)>) {
            if hit.is_some() {
                return;
            }
            if let Some(found) = named_prefix(node) {
                *hit = Some(found);
            }
            match &node.expr {
                Expr::Member { recv, .. } => walk(recv, hit),
                Expr::Binary { left, right, .. } => {
                    walk(left, hit);
                    walk(right, hit);
                }
                Expr::Unary { operand, .. } => walk(operand, hit),
                Expr::Cond { cond, then, els } => {
                    walk(cond, hit);
                    walk(then, hit);
                    walk(els, hit);
                }
                Expr::Interp { parts } | Expr::Adjacent(parts) => {
                    for part in parts {
                        walk(part, hit);
                    }
                }
                Expr::List(parts) | Expr::Set(parts) | Expr::Record(parts) => {
                    for part in parts {
                        walk(part, hit);
                    }
                }
                Expr::Map(parts) => {
                    for part in parts {
                        walk(part, hit);
                    }
                }
                Expr::Call { args, .. } => {
                    for arg in args {
                        walk(arg, hit);
                    }
                }
                Expr::Entry(key, val) => {
                    walk(key, hit);
                    walk(val, hit);
                }
                Expr::IfElement { cond, then, els } => {
                    walk(cond, hit);
                    walk(then, hit);
                    if let Some(els) = els {
                        walk(els, hit);
                    }
                }
                Expr::Spread { operand, .. } => walk(operand, hit),
                Expr::Value(_)
                | Expr::Ident(_)
                | Expr::Opaque
                | Expr::Other
                | Expr::Faulted(_) => {}
            }
        }
        let mut hit = None;
        walk(node, &mut hit);
        let (name, span) = hit?;
        let deferred = self
            .graph
            .import_edges(self.lib)
            .iter()
            .any(|edge| edge.deferred && edge.prefix == Some(name));
        deferred.then_some(span)
    }

    /// The deferred-import face of a PATTERN constant (upstream
    /// `evaluation.dart`'s `_getDeferredLibraryError` →
    /// `patternConstantFromDeferredLibrary`, the `SwitchPatternCase` arm at
    /// :2130-2132): a constant expression that reaches a value through a
    /// `deferred` import's prefix fails const-eval at the referenced name,
    /// and the row anchors there (`errorTarget` — the identifier of a
    /// `PrefixedIdentifier`, the name of a `NamedType`). The same walk as
    /// [`Self::legacy_deferred_span`]; a pattern-era switch takes this code
    /// instead of the legacy case-expression one.
    pub(crate) fn pattern_deferred_span(
        &self,
        node: &crate::const_eval::Node,
    ) -> Option<SourceRange> {
        // The pattern arm's `errorTarget` is the referenced NAME, not the
        // prefix: `visitPrefixedIdentifier` passes `node.identifier` and
        // `visitNamedType` passes `node.name` (`evaluation.dart`), so the
        // row anchors at `c` of `a.c` (probed 3.13.7: `case a.c` reports
        // at the `c` column, len 1).
        fn named_prefix(node: &crate::const_eval::Node) -> Option<(StringId, StringId)> {
            if let Expr::Member { recv, name } = &node.expr
                && let Expr::Ident(prefix) = &recv.expr
            {
                return Some((*prefix, *name));
            }
            None
        }
        let mut hit: Option<(StringId, StringId)> = None;
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            if hit.is_none()
                && let Some(found) = named_prefix(n)
            {
                hit = Some(found);
            }
            match &n.expr {
                Expr::Member { recv, .. } => stack.push(recv),
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
                Expr::Interp { parts } | Expr::Adjacent(parts) => stack.extend(parts.iter()),
                Expr::List(parts) | Expr::Set(parts) | Expr::Record(parts) => {
                    stack.extend(parts.iter());
                }
                Expr::Map(parts) => stack.extend(parts.iter()),
                Expr::Call { args, .. } => stack.extend(args.iter()),
                Expr::Entry(key, val) => {
                    stack.push(key);
                    stack.push(val);
                }
                Expr::IfElement { cond, then, els } => {
                    stack.push(cond);
                    stack.push(then);
                    if let Some(els) = els {
                        stack.push(els);
                    }
                }
                Expr::Spread { operand, .. } => stack.push(operand),
                Expr::Value(_)
                | Expr::Ident(_)
                | Expr::Opaque
                | Expr::Other
                | Expr::Faulted(_) => {}
            }
        }
        fn member_span(node: &crate::const_eval::Node) -> Option<SourceRange> {
            if let Expr::Member { .. } = &node.expr {
                return Some(node.span);
            }
            None
        }
        let (prefix, name) = hit?;
        let deferred = self
            .graph
            .import_edges(self.lib)
            .iter()
            .any(|edge| edge.deferred && edge.prefix == Some(prefix));
        if !deferred {
            return None;
        }
        // The member node's span ends at the selector's end (`dot_access`
        // builds receiver-through-selector), so the identifier's own extent
        // is the span TAIL. Dart identifiers are ASCII-only here, so the
        // interned text's length is its UTF-16 width.
        let span = member_span(node)?;
        let width = self.strings.resolve(name).len() as u32;
        Some(SourceRange {
            offset: span.end().saturating_sub(width),
            length: width,
        })
    }

    /// Runs the legacy switch checks over the closed context the caller
    /// pops (statement switches only — a switch expression cannot exist
    /// before 3.0). The upstream ordering is: per-head checks in source
    /// order (primitive equality, then the type-mismatch gate, both inside
    /// `validateExpression`'s single eval — here the eval already happened
    /// at the head's close), then the switch-level enum-constant coverage
    /// (`error_verifier.dart`'s members walk).
    pub(crate) fn report_legacy_switch(&mut self) {
        let Some(ctx) = self.switch.switches.pop_discard() else {
            return;
        };
        // Upstream's `expressionValue is! DartObjectImpl` early-return: a
        // non-constant or unresolved head already reported its own row at
        // the head's span, and the opaque `Verdict::Constant` carries no
        // readable value — only a folded `ConstantValue` answers the
        // per-head questions.
        for case in &ctx.legacy_cases {
            if case.deferred.is_some() {
                continue;
            }
            let Some(value) = &case.value else {
                continue;
            };
            if !self.legacy_primitive_equality(value) {
                let ty_text = self.ty_display(self.folded_ty(value));
                self.emit(
                    SemanticCode::CaseExpressionTypeImplementsEquals,
                    case.span,
                    &ty_text,
                    "",
                );
            }
            // `caseExpressionTypeMismatch` (type_analyzer.dart's
            // `analyzeConstantPattern` legacy arm): the constant's static
            // type must be a subtype of the scrutinee's matched type.
            // `folded_ty` is the value's declared instantiation; an
            // unfaced shape folds `Unknown` — the name-level lattice's
            // unknown answers nothing and under-reports, matching
            // upstream's `InvalidType`-blind spot.
            let case_ty = self.folded_ty(value);
            if case_ty != Ty::UNKNOWN
                && ctx.scrutinee != Ty::Unknown
                && !self.name_subtype(case_ty, ctx.scrutinee)
            {
                let a0 = self.ty_display(case_ty);
                let a1 = self.ty_display(ctx.scrutinee);
                self.emit(
                    SemanticCode::CaseExpressionTypeIsNotSwitchExpressionSubtype,
                    case.span,
                    &a0,
                    &a1,
                );
            }
        }
        self.legacy_missing_enum_constants(&ctx);
    }

    /// `_checkForMissingEnumConstantInSwitch`
    /// (error_verifier.dart:5870-5937): a no-`default` switch over an enum
    /// type reports one row per uncovered enum constant — and a `null`
    /// row for a nullable enum scrutinee no `case null` covers. Upstream
    /// anchors `statement.offset` to `rightParenthesis.end` (the whole
    /// `switch (e)` head); the context carries only the `switch` keyword,
    /// which keeps the same row head under the `(severity, code, file)`
    /// diff face.
    fn legacy_missing_enum_constants(&mut self, ctx: &SwitchCtx) {
        if ctx.is_expression || ctx.default_kw.is_some() {
            return;
        }
        let Ty::Named {
            name: enum_name,
            nullable,
            ..
        } = ctx.scrutinee
        else {
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
        if container.kind != ContainerKind::Enum {
            return;
        }
        let mut missing: Vec<StringId> = container.enum_constants.iter().copied().collect();
        let mut has_case_null = false;
        for case in &ctx.legacy_cases {
            match &case.value {
                // Upstream removes the written constant NAME
                // (`_getConstantName` — the identifier's tail), which the
                // evaluated enum value carries exactly.
                Some(ConstantValue::Enum { container, name }) if *container == enum_name => {
                    missing.retain(|constant| constant != name);
                }
                Some(ConstantValue::Null) => has_case_null = true,
                _ => {}
            }
        }
        // One row per missing constant, in the declaration's own order
        // (upstream's `Set`-iteration order follows the field walk — the
        // declaration order `enum_constants` carries).
        let names: Vec<String> = missing
            .iter()
            .map(|name| self.strings.resolve(*name).to_string())
            .collect();
        for name in names {
            self.emit(
                SemanticCode::MissingEnumConstantInSwitch,
                ctx.switch_kw,
                &name,
                "",
            );
        }
        // The nullable tail (error_verifier.dart:5922-5931): a nullable
        // enum scrutinee without a `case null` reports a `null` row.
        if nullable && !has_case_null {
            self.emit(
                SemanticCode::MissingEnumConstantInSwitch,
                ctx.switch_kw,
                "null",
                "",
            );
        }
    }

    /// `DartObjectImpl.hasPrimitiveEquality` at the name level
    /// (value.dart — the per-state answers): the primitive and
    /// enum/symbol/type-literal states accept unconditionally;
    /// collections and records accept when every element does; a `Created`
    /// instance's container must resolve `==`/`hashCode` concretely to
    /// `Object`'s ([`Self::legacy_created_primitive_equality`]); the
    /// opaque `Instance` accepts only on the function tear-off shape (the
    /// upstream `FunctionState` answer — `GenericState` answers `false`
    /// and the engine folds no finer face).
    pub(crate) fn legacy_primitive_equality(&self, value: &ConstantValue) -> bool {
        match value {
            ConstantValue::Null
            | ConstantValue::Bool(_)
            | ConstantValue::Int(_)
            | ConstantValue::Double(_)
            | ConstantValue::Str(_)
            | ConstantValue::Enum { .. }
            | ConstantValue::Type(_)
            | ConstantValue::Symbol(_) => true,
            ConstantValue::List(elems) | ConstantValue::Set(elems) => elems
                .iter()
                .all(|elem| self.legacy_primitive_equality(elem)),
            ConstantValue::Map(pairs) => pairs.iter().all(|(key, val)| {
                self.legacy_primitive_equality(key) && self.legacy_primitive_equality(val)
            }),
            ConstantValue::Record { positional, named } => positional
                .iter()
                .chain(named.iter().map(|(_, val)| val))
                .all(|elem| self.legacy_primitive_equality(elem)),
            ConstantValue::Created(name) => self.legacy_created_primitive_equality(*name),
            ConstantValue::Instance(kind) => matches!(kind, InstanceKind::Function),
        }
    }

    /// `GenericState.hasPrimitiveEquality` at the name level: the
    /// container's `==` resolves concretely to `Object`'s — "no override
    /// anywhere" (`Walk::Missing`) or "the found face is `Object`'s".
    /// Pre-3.0 the official checks `==` alone (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/constant/value.dart — the `hashCode` half
    /// is the `Feature.patterns` arm, never reached here). A found face
    /// on any other container fails the gate; a broken chain stays
    /// undecidable and keeps the under-report.
    fn legacy_created_primitive_equality(&self, container: StringId) -> bool {
        let object = self.intern("Object");
        match self.walk_members(container, self.intern("==")) {
            Walk::Missing => true,
            Walk::Found(_) => self.member_faces.last_origin() == Some(object),
            Walk::ChainBroken => false,
        }
    }
}
