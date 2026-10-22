//! A cascade section's argument resolves its own member chain against its
//! own receiver (the `args` corpus shape `StringBuffer()..writeln(x)
//! ..writeln(argParser.usage)`): the section's argument `argParser.usage`
//! reads `ArgParser.usage`, never a `usage` on the cascade target.
//!
//! Oracle provenance (`dart analyze` 3.13.1): silent.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

#[test]
fn cascade_argument_member_reads_its_own_receiver() {
    let report = report_ops(
        "class P { String get usage => ''; int get width => 0; }\n\
         class B { void writeln(Object? o) {} void write(Object? o) {} }\n\
         String f(P argParser, String prefix) {\n\
           var buffer = B()\n\
             ..writeln(prefix + argParser.usage)\n\
             ..writeln(argParser.usage);\n\
           buffer.write(argParser.width);\n\
           return '';\n\
         }\n",
    );
    assert_silent(&report);
}

/// `var set = SplayTreeSet<E>(cmp); return set;` against a `Set<E>` return:
/// the inferred local carries the written `<E>`.
#[test]
fn explicit_creation_args_ride_the_inferred_local() {
    let report = report_ops(
        "class TreeSet<E> extends Set<E> { TreeSet(int Function(E, E) cmp); }\n\
         class Q<E> {\n\
           final int Function(E, E) comparison;\n\
           Q(this.comparison);\n\
           Set<E> toSet() {\n\
             var set = TreeSet<E>(comparison);\n\
             return set;\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

/// `const JS([this.name])`: an optional initializing formal makes the
/// constructor callable with no arguments (`@JS()`).
#[test]
fn optional_initializing_formal_is_optional() {
    let report = report_ops(
        "class JS { final String? name; const JS([this.name]); }\n\
         @JS()\n\
         class X {}\n\
         const j = JS();\n",
    );
    assert_silent(&report);
}

/// `Object`'s members read on a nullable receiver without a null check
/// (`hashCode`, `runtimeType`, `toString`, `noSuchMethod`) — the oracle
/// resolves them through `Null`'s own interface.
#[test]
fn object_members_read_on_nullable_receivers() {
    let report = report_ops(
        "int f(Object? x, String? s) {\n\
           var t = x.runtimeType;\n\
           var u = s.toString();\n\
           return x.hashCode + u.length + t.hashCode;\n\
         }\n",
    );
    assert_silent(&report);
}

#[test]
fn optional_initializing_formal_annotation_shapes() {
    let report = report_ops(
        "class Tag { final String? name; const Tag([this.name]); }\n\
         class Tag2 { final String? name; const Tag2([String? name]) : name = name; }\n\
         @Tag()\n\
         class X {}\n\
         @Tag2()\n\
         class Y {}\n\
         @Tag('a')\n\
         class Z {}\n",
    );
    assert_silent(&report);
}

/// A promoted local read inside a cascade section's ARGUMENT list keeps its
/// promotion (`tracker()..markOpened(file.path, ...)` — the argument list
/// is ordinary expression context, so the `.` merge takes the
/// promoted-receiver latch like any plain access; only the section
/// selector itself resolves against the latched cascade target,
/// type_analyzer.dart:611-618). The push-time face kept the declared
/// nullable type when the latch was gated on the whole cascade, and the
/// promoted read reported `unchecked_use_of_nullable_value`.
///
/// Oracle provenance (`dart analyze` 3.13.1): silent besides the unused
/// field warning.
#[test]
fn cascade_argument_promoted_local_keeps_promotion() {
    let report = report_ops(
        "class F { String get path => ''; }\n\
         class T { void markOpened(String p, Object? db) {} }\n\
         T tracker() => T();\n\
         Object? _trackedBy;\n\
         void f(F? file) {\n\
           if (file != null) {\n\
             _trackedBy = tracker()..markOpened(file.path, null);\n\
           }\n\
         }\n",
    );
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| *name == "unchecked_use_of_nullable_value"),
        "{:?}",
        rows(&report)
    );
}
