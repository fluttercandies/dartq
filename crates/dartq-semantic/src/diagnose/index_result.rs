//! The index read's static type (`c[i]`): the receiver's `[]` operator
//! declared return with the receiver's written arguments substituted for
//! the declaring container's parameters — `commands[name]` over a
//! `Map<String, Cmd>` reads `Cmd?`, so the trailing-`!` write
//! `command = commands[name]!` stages `Cmd` into the flow and the local
//! promotes (the official `IndexExpression.staticType`). Only a `[]` the
//! receiver's own container declares answers: an inherited operator's
//! environment composes through the clause hops the name-level model
//! strips, and that face stays the opaque value row (the under-report
//! direction). A raw receiver reads the top instantiation.
//!
//! #2522 widens the face past the bang shape: a `var`-inferred
//! declarator whose initializer is a `List<E>`/`Map<K, V>` index read
//! binds the element face (`E`/`V?`) through the same result row the
//! bang form already carried — the assertion never decided the type,
//! only its nullability. Five downstream faces that bind stands up are
//! the analyzer corpus's own var-index chains:
//!
//! - `reference.dart`'s `var reference = _referenceByIndex[index]` arm
//!   writes a `Reference?` local, and each switch arm's `reference = …`
//!   re-decision promotes it by the written `Reference` face — the local
//!   needs the `Reference?` the `Map`-`[]` result stages (upstream Dart
//!   SDK source, pkg/analyzer/lib/src/summary2/reference.dart:680, arm
//!   writes at :684-707);
//! - `evaluation.dart`'s `_checkParameters` loop binds
//!   `var argumentValue = _argumentValueMap[baseParameter]` — a
//!   `DartObjectImpl?` face the `argumentValue == null` guard then
//!   promotes away, the oracle's own null-check arm (upstream Dart SDK
//!   source, pkg/analyzer/lib/src/dart/constant/evaluation.dart:3335,
//!   guard at :3341);
//! - `replacement_visitor.dart`'s formal-parameter walk binds
//!   `var parameter = node.formalParameters[i]` and
//!   `var type = parameter.type` — the `List<E>` element face carries
//!   the parameter's own member reads (`InternalFormalParameterElement`,
//!   upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/element/replacement_visitor.dart:251/256);
//! - `ffi_verifier.dart`'s `?.`-chain arm reads a `var` local bound
//!   from a core-list index (`var f = invocation.argumentList.arguments2[0]`
//!   over the `List<Expression>` face), where the NON-nullable element
//!   face makes the `?.` continuation's receiver never-null and the
//!   null-aware row fires at the `?.` (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/generated/ffi_verifier.dart:1806; probed
//!   dart analyze 3.13.7);
//! - `typed_literal_resolver.dart`'s context walk binds
//!   `var elementType = literalType.typeArguments[0]` and reads the
//!   `iterableType` param face off it — a `List<TypeImpl>` element that
//!   decides the `CollectionLiteralContext` arm's parameter (upstream
//!   Dart SDK source,
//!   pkg/analyzer/lib/src/dart/resolver/typed_literal_resolver.dart:166).
//!
//! Every face is the same `V?`/`E` result the `[]` fold already stages
//! on the value row; the window gate that binds it into the declarator
//! reads the result row's own face — the `!`-asserted, the plainly
//! nullable, and the `?[` shorted forms each carry their correct
//! static type, so the bind is the read's own (the oracle's
//! `IndexExpression.staticType` composes `V?` before the `!`, and the
//! flow's own write/promotion machinery decides nullness from there).

use super::*;

impl Engine<'_> {
    /// The `[]` read's static type on `receiver`, or `Unknown` when the
    /// receiver's own container declares no `[]` or the receiver is not a
    /// named type.
    pub(crate) fn index_result_ty(&self, receiver: Ty) -> Ty {
        let Ty::Named { name, args, .. } = receiver else {
            return Ty::UNKNOWN;
        };
        let Some((container, owner)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) else {
            return self.core_index_result_ty(name, args);
        };
        let Some(entry) = container.members.get(&self.intern("[]")) else {
            return Ty::UNKNOWN;
        };
        if entry.kind != MemberKind::Method {
            return Ty::UNKNOWN;
        }
        let declared = self.boundary.ty(owner, entry.return_ty);
        let params = &container.type_param_names;
        if params.is_empty() {
            return self.fold_alias_ty(declared);
        }
        let written: Vec<Ty> = self.boundary.pool().get(args).to_vec();
        let keys: FxHashMap<StringId, Ty> = if written.is_empty() {
            params.iter().map(|&param| (param, Ty::Dynamic)).collect()
        } else if written.len() == params.len() {
            params.iter().copied().zip(written).collect()
        } else {
            return Ty::UNKNOWN;
        };
        self.fold_alias_ty(self.substitute_ty(declared, &keys))
    }

    /// The core generics' `[]` face on the unindexed SDK heads (#2521):
    /// `Map<K, V>` declares `V? operator [](Object? key)` and `List<E>`
    /// declares `E operator [](int index)` (upstream Dart SDK source,
    /// sdk/lib/core/map.dart:288, list.dart:375), so the read substitutes
    /// the receiver's written arguments — `numberFormatSymbols[name]!`
    /// types `NumberSymbols` and the `var`-inferred local carries the
    /// real type downstream. The map's return is `V?` whatever `V`
    /// spells; the list's returns `E` as written. `Set` and `Iterable`
    /// declare no `[]` upstream, so their receivers keep the unknown
    /// face (the under-report direction), and a receiver without
    /// written arguments answers nothing.
    fn core_index_result_ty(&self, name: StringId, args: ArgListId) -> Ty {
        let written = self.boundary.pool().get(args).to_vec();
        if self.intern("Map") == name && written.len() == 2 {
            return Self::nullable_form(written[1]);
        }
        if self.intern("List") == name && written.len() == 1 {
            return written[0];
        }
        Ty::UNKNOWN
    }
}

#[cfg(test)]
mod tests {
    //! The #2522 non-bang index-result binds: a `var`-inferred
    //! declarator whose initializer is a `List<E>`/`Map<K, V>` index
    //! read binds the element face (`E`/`V?`) — the `!`-assertion
    //! never decided the type, only its nullability, and the `[]`
    //! fold already stages the read's own static type on the result
    //! row. The five armed faces are the analyzer corpus's own
    //! var-index chains, each carrying the upstream citation of the
    //! `.dart` line the face lights up.

    use crate::diagnose::FileReport;

    /// The canonical single-file rig (`super::test_rig`): the fixture rides
    /// the shared `dart:core` stub and the resolved-type snapshot, so the
    /// core heads (`int`, `List`, `Map`, `String`) resolve exactly like the
    /// product path. The private copy this module used to carry built its
    /// own graph against the no-ops `diagnose_with` entry and lost every
    /// core head (`undefined_class 'int'` on each pin).
    fn unit_report(source: &str) -> FileReport {
        super::test_rig::report_ops(source)
    }

    fn codes(report: &FileReport) -> Vec<&str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// reference.dart's switch-arm write-promotion face: `var
    /// reference = _referenceByIndex[index]` binds the map's `V?`
    /// face (`Reference?`), and each switch arm's `reference = …`
    /// write promotes it by the written `Reference` — the switch
    /// body's member reads stay silent exactly like the oracle's
    /// promoted local (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/summary2/reference.dart:680 — the read —
    /// with the arm writes at :684-707).
    ///
    /// The bind rides the #3085 gate flip in `listener.rs`
    /// (`end_variable_initializer`'s index-result arm now admits the
    /// nullable `V?` static type); the fixture is `final`/non-`const`
    /// so the only row under test is the inference face (oracle 3.13.1:
    /// zero rows for this source).
    #[test]
    fn map_index_binds_nullable_then_switch_arm_promotes() {
        let source = r#"
class Reference {
  int get id => 0;
}

final Map<int, Reference> _referenceByIndex = {0: Reference()};

int f(int index, int tag) {
  var reference = _referenceByIndex[index];
  switch (tag) {
    case 1:
      reference = Reference();
    default:
      reference = Reference();
  }
  return reference.id;
}
"#;
        let report = unit_report(source);
        assert_eq!(
            codes(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// evaluation.dart's optional-parameter guard face: `var
    /// argumentValue = _argumentValueMap[baseParameter]` binds
    /// `DartObjectImpl?`, and the `argumentValue == null` guard's
    /// else arm promotes it — the member read below the guard
    /// carries the non-null face, so no unchecked-use verdict fires
    /// where the oracle stays silent (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/constant/evaluation.dart:3335 — the
    /// read — with the `:3341` null guard).
    ///
    /// Lifted by the same #3085 listener gate flip; oracle 3.13.1:
    /// zero rows for this source.
    #[test]
    fn map_index_binds_nullable_then_guard_promotes() {
        let source = r#"
class DartObjectImpl {
  int get value => 0;
}

final Map<String, DartObjectImpl> _argumentValueMap = {
  'x': DartObjectImpl(),
};

class Parameter {
  Parameter(this.type);
  final int type;
}

class Config {
  Config(int arg);
}

void f(Parameter parameter) {
  var argumentValue = _argumentValueMap['x'];
  if (argumentValue == null) {
    return;
  }
  Config(argumentValue.value);
}
"#;
        let report = unit_report(source);
        assert_eq!(
            codes(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// replacement_visitor.dart:251/256's List-element member face:
    /// `var parameter = node.formalParameters[i]` binds the
    /// `List<Parameter>` element face `Parameter`, and `var type =
    /// parameter.type` reads the element's own member — the second
    /// bind carries the declared member type, so `type` is `int` and
    /// the argument verdict stays silent (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/replacement_visitor.dart:
    /// 251/256).
    #[test]
    fn list_index_binds_element_then_member_face() {
        let source = r#"
class Parameter {
  Parameter(this.type);
  final int type;
}

class Node {
  List<Parameter> get formalParameters => [];
}

class Config {
  Config(int arg);
}

void f(Node node, int i) {
  var parameter = node.formalParameters[i];
  var type = parameter.type;
  Config(type);
}
"#;
        let report = unit_report(source);
        assert_eq!(
            codes(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// ffi_verifier.dart:1806's `?.`-chain null-propagation face:
    /// `var f = invocation.argumentList.arguments2[0]` binds the
    /// `List<Expression>` element face `Expression`, and `f?.name`
    /// reads the member through the null-aware latch — the `?.`
    /// chain carries the element face the oracle's own face reports on:
    /// `List<Expression>[0]` binds the NON-nullable `Expression` element,
    /// so the `f?.name` continuation's receiver can't be null and the
    /// verifier's `_checkForUnnecessaryNullAware` row fires at the `?.`
    /// (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/generated/ffi_verifier.dart:1806's chain shape;
    /// probed dart analyze 3.13.7: one INVALID_NULL_AWARE_OPERATOR
    /// WARNING at `?.`, nothing else — the silent-prior record was wrong).
    #[test]
    fn list_index_binds_element_then_null_aware_chain() {
        let source = r#"
class Expression {
  String get name => '';
}

class ArgumentList {
  List<Expression> get arguments2 => [];
}

class Invocation {
  ArgumentList get argumentList => ArgumentList();
}

String? f(Invocation invocation) {
  var f = invocation.argumentList.arguments2[0];
  return f?.name;
}
"#;
        let report = unit_report(source);
        assert_eq!(
            codes(&report),
            vec!["invalid_null_aware_operator"],
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            report.diagnostics[0].message.to_string(),
            "The receiver can't be null, so the null-aware operator '?.' is unnecessary."
        );
    }

    /// typed_literal_resolver.dart:166's iterableType param face:
    /// `var elementType = literalType.typeArguments[0]` binds the
    /// `List<TypeImpl>` element face `TypeImpl`, and the
    /// `iterableType(elementType)` send's formal reads it — the
    /// param's declared `TypeImpl` shape carries the element type
    /// into the call's result, so `result.kind` reads `int` and
    /// stays silent (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/typed_literal_resolver.
    /// dart:166).
    #[test]
    fn list_index_binds_element_then_iterable_param() {
        let source = r#"
class TypeImpl {
  int get kind => 0;
}

class LiteralType {
  List<TypeImpl> get typeArguments => [];
}

class TypeProvider {
  TypeImpl iterableType(TypeImpl element) => element;
}

int f(TypeProvider provider, LiteralType literalType) {
  var elementType = literalType.typeArguments[0];
  var result = provider.iterableType(elementType);
  return result.kind;
}
"#;
        let report = unit_report(source);
        assert_eq!(
            codes(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// The bind-is-real control: a `List<Reference>` index result
    /// binds the non-null `Reference`, so `reference?.id` reads the
    /// dead `?.` — the oracle's `invalid_null_aware_operator` arm on
    /// a strictly non-nullable receiver (the bind carries the read's
    /// own face, not a widened `Unknown` that would stay silent).
    #[test]
    fn list_index_bind_dead_null_aware_reports() {
        let source = r#"
class Reference {
  int get id => 0;
}

List<Reference> byIndex() => [];

// The `int?` return keeps the dead `?.` the only row under test — an `int`
// return would add an incidental `return_of_invalid_type` (`reference?.id`
// is `int?`) that the oracle also emits but this pin does not target.
int? f(int index) {
  var reference = byIndex()[index];
  return reference?.id;
}
"#;
        let report = unit_report(source);
        assert_eq!(
            codes(&report),
            ["invalid_null_aware_operator"],
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }
}

/// The #3160 `invalid_null_aware_operator` family pins (the
/// short-circuit-variant message, the `?[`/`?..`/`...?` anchor spans, the
/// static-qualifier `C?.x` face, and the collection element/map-entry
/// markers), kept as a standalone module so the shared `pins` rig stays
/// untouched while its lane is in flight.
#[cfg(test)]
#[path = "ticket_3160_tests.rs"]
mod ticket_3160_tests;
