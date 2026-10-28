//! Ticket #2730: a raw generic receiver (`Cmd c` under `class Cmd<T
//! extends Foo<int>>`) whose declaring container lives in a sibling unit
//! must not panic — the instantiate-to-bounds environment projects the
//! declaration's bounds across the owner unit's `ArgPool` before any
//! walk; a foreign `ArgListId` read through the diagnosing pool was the
//! `model.rs` index-OOB the CLI then could not propagate (the sibling
//! fan-out held rayon's job join open).
//!
//! The pin runs the exact shape end-to-end through [`Engine::analyze`]:
//! `a.dart` declares the bounded generic (`T extends Foo<int>` — a bound
//! whose interned argument list lives in `a.dart`'s own pool), `b.dart`
//! reads a member through a raw `Cmd` spelling. Before the fix the pass
//! panicked at `ArgPool::get` (len 1, index 1); after it the run
//! completes.

mod common;

use common::Fixture;
use dartq_engine::CancellationToken;

/// A raw receiver over a foreign bounded generic: `b.dart`'s `c.current`
/// instantiates `Cmd`'s `T extends Foo<int>` from `a.dart` — the bound's
/// `Foo<int>` argument list interns in `a.dart`'s pool, so reading it
/// through `b.dart`'s diagnosing pool indexed another universe's slot.
/// Completion (not a panic) is the pin.
#[test]
fn raw_receiver_over_foreign_bounded_generic_completes() {
    let fixture = Fixture::new(&[]);
    let mut engine = fixture.engine();
    engine.set_content(
        &fixture.path("lib/a.dart"),
        b"class Foo<T> {\n  T? value;\n}\nclass Cmd<T extends Foo<int>> {\n  T? last;\n  T? get current => last;\n}\n",
    );
    engine.set_content(
        &fixture.path("lib/b.dart"),
        b"import 'a.dart';\nvoid f(Cmd c) {\n  var x = c.current;\n}\n",
    );
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("the foreign-pool projection completes, no panic");
}

/// The same-file twin stays correct as well: `Cmd` declared and read in
/// one unit — the bound interned in the diagnosing pool itself, the
/// pre-existing path the foreign case now mirrors.
#[test]
fn raw_receiver_over_own_bounded_generic_completes() {
    let fixture = Fixture::new(&[]);
    let mut engine = fixture.engine();
    engine.set_content(
        &fixture.path("lib/c.dart"),
        b"class Foo<T> {}\nclass Cmd<T extends Foo<int>> {\n  T? get current => null;\n}\nvoid f(Cmd c) {\n  var x = c.current;\n}\n",
    );
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("the own-pool path completes");
}
