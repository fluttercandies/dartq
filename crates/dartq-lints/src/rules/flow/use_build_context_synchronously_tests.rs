use std::path::{Path, PathBuf};

use crate::engine::{AsyncContextUse, Lint};
use crate::testsupport::{Fixture, one_rule};

/// One unguarded row anchored at `offset` (width 3, the shape of a
/// `context` use).
fn unguarded(offset: u32) -> AsyncContextUse {
    AsyncContextUse {
        range: offset..offset + 3,
        guarded: false,
        unrelated_mounted: false,
    }
}

/// Runs the rule with `rows` mounted (no workspace facts — the
/// test-directory gate stays out).
fn run(f: &Fixture, rows: &[AsyncContextUse]) -> Vec<Lint> {
    f.run_with(
        &one_rule("use_build_context_synchronously"),
        None,
        None,
        Some(rows),
    )
}

/// The upstream async-use message pair, verbatim from
/// `diagnostic.g.dart`.
#[test]
fn unguarded_row_reports_async_use() {
    let f = Fixture::parse("void f() {}\n");
    let lints = run(&f, &[unguarded(8)]);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Don't use 'BuildContext's across async gaps."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some(
            "Try rewriting the code to not use the 'BuildContext', or guard the use with a 'mounted' check."
        )
    );
    assert_eq!(lints[0].range, 8..11);
}

/// A guarded verdict never reports; the unrelated-guarded verdict
/// upgrades to the `wrong_mounted` message pair.
#[test]
fn guarded_stays_silent_unrelated_guard_upgrades() {
    let f = Fixture::parse("void f() {}\n");
    let guarded = AsyncContextUse {
        range: 8..11,
        guarded: true,
        unrelated_mounted: false,
    };
    assert!(run(&f, &[guarded]).is_empty());
    let unrelated = AsyncContextUse {
        range: 8..11,
        guarded: false,
        unrelated_mounted: true,
    };
    let lints = run(&f, &[unrelated]);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Don't use 'BuildContext's across async gaps, guarded by an unrelated 'mounted' check."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some(
            "Guard a 'State.context' use with a 'mounted' check on the State, and other BuildContext use with a 'mounted' check on the BuildContext."
        )
    );
}

/// Rows report in face order at their own spans.
#[test]
fn rows_report_in_order_at_their_spans() {
    let f = Fixture::parse(
        "void f() {}\n\n// The row anchors below must sit inside the source.\n\n",
    );
    let lints = run(&f, &[unguarded(8), unguarded(40), unguarded(64)]);
    assert_eq!(lints.len(), 3, "{lints:?}");
    assert_eq!(lints[0].range.start, 8);
    assert_eq!(lints[1].range.start, 40);
    assert_eq!(lints[2].range.start, 64);
}

/// The registration bail: files under the package's test directories
/// (`workspace.dart` `_testDirectoryNames`) emit nothing; the same
/// rows outside them report.
#[test]
fn test_directories_bail() {
    let root = Path::new("/pkg");
    let facts = crate::WorkspaceFacts {
        own_package: "t".to_owned(),
        dependencies: ["t".to_owned()].into_iter().collect(),
        dev_dependencies: rustc_hash::FxHashSet::default(),
        root: root.to_owned(),
        lib_dir: root.join("lib"),
    };
    for directory in ["test", "integration_test", "test_driver", "testing"] {
        let mut f = Fixture::parse("void f() {}\n");
        f.path = PathBuf::from(format!("/pkg/{directory}/a.dart"));
        let lints = f.run_with(
            &one_rule("use_build_context_synchronously"),
            Some(&facts),
            None,
            Some(&[unguarded(8)]),
        );
        assert!(lints.is_empty(), "{directory}: {lints:?}");
    }
    let mut f = Fixture::parse("void f() {}\n");
    f.path = PathBuf::from("/pkg/lib/a.dart");
    let lints = f.run_with(
        &one_rule("use_build_context_synchronously"),
        Some(&facts),
        None,
        Some(&[unguarded(8)]),
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// The element-identity gate: a same-file type declaration named
/// `BuildContext` silences the whole unit.
#[test]
fn build_context_shadow_silences_the_unit() {
    let f = Fixture::parse("class BuildContext {}\n\nvoid f() {}\n");
    assert!(run(&f, &[unguarded(28)]).is_empty());
    // A use of the NAME in a non-declaration position is not a
    // shadow: the unit still reports.
    let f = Fixture::parse("void f(BuildContext? context) {}\n");
    let lints = run(&f, &[unguarded(8)]);
    assert_eq!(lints.len(), 1, "{lints:?}");
}

/// A typedef carries the type-name spelling the same as a class.
#[test]
fn build_context_typedef_shadow_silences_the_unit() {
    let f = Fixture::parse("typedef BuildContext = Object;\n\nvoid f() {}\n");
    assert!(run(&f, &[unguarded(38)]).is_empty());
}
