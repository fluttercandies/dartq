//! The #2470 pins: the multiplexed parse must feed `DeclarationCollector`
//! every event the element model consumes — `end_type_variable` (the
//! extension `on`-face's `type_param_bounds` latch) and the
//! `*_formal_parameter_default_value_expression` pair (the
//! `formal_default_depth` window that keeps a default literal's type
//! references out of the armed slot).
//!
//! Both events were absent from `MultiplexListener`'s forwarding set — a
//! trait event with no forward falls through to the listener's default
//! no-op (the mux's documented starvation mode), so the engine's
//! parse-once fan-out (`l1.rs`) produced `FileElements` whose extension
//! faces carried `Dynamic` placeholders for every declared bound and whose
//! default-value type references could arm the slot the next consumer
//! took. The in-process rigs parse the same unit through
//! `dartq_semantic::collect` directly — a solo listener, no mux — so the
//! bounds latched there and the divergence only ever surfaced on the
//! CLI/engine world path (the #2442 `AMBIGUOUS_EXTENSION_MEMBER_ACCESS`
//! census rows: two bound-excluded faces both staying applicable).
//!
//! Oracle provenance (`dart analyze` 3.13.1 over the /tmp/probe2470
//! fixtures): the bound-rejecting faces drop out of the pool — the
//! `List<AccessorFragment>`/`List<InducingFragment>` reads report zero
//! issues while the raw-`List` receiver's `undefined_getter` stands — and
//! the equally-bounded twins stay ambiguous.

mod common;

use std::path::Path;

use dartq_diagnostics::Diagnostic;
use dartq_engine::Engine;

use common::Fixture;

/// Two bounded sibling extensions racing one member — each bound accepts
/// exactly one of the two receiver shapes (the #2442 `amb4` probe
/// verbatim). The oracle stays silent on both reads: `T extends
/// AccessorFragment` rejects `InducingFragment` and vice versa.
const BOUNDED_PAIR: &str = r#"class Fragment {}
class AccessorFragment extends Fragment {}
class InducingFragment extends Fragment {}
extension AccessorExt<T extends AccessorFragment> on List<T> {
  Iterable<T> get withOrigin => const [];
}
extension InducingExt<T extends InducingFragment> on List<T> {
  Iterable<T> get withOrigin => const [];
}
void useAccessors(List<AccessorFragment> xs) {
  xs.withOrigin;
}
void useInducing(List<InducingFragment> xs) {
  xs.withOrigin;
}
"#;

/// The same pair plus a raw `List` receiver: `List` instantiates to
/// `List<dynamic>` and `dynamic` fails every concrete bound — both faces
/// drop, no extension applies, the access reports `undefined_getter`
/// (the /tmp/probe2431 b.dart :38 verdict, verbatim).
const RAW_RECEIVER: &str = r#"class Fragment {}
class AccessorFragment extends Fragment {}
class InducingFragment extends Fragment {}
extension AccessorExt<T extends AccessorFragment> on List<T> {
  Iterable<T> get withOrigin => const [];
}
extension InducingExt<T extends InducingFragment> on List<T> {
  Iterable<T> get withOrigin => const [];
}
void useRaw(List xs) {
  xs.withOrigin;
}
"#;

/// The over-domination guard: two faces bounded by the SAME type project
/// to identical instantiate-to-bounds types — mutually unordered, the
/// access stays ambiguous (the specificity step orders only a strict
/// projection subtype; the #2431 `equally_bounded_twin_faces_stay_
/// ambiguous` shape carried through the engine path).
const EQUAL_BOUNDS: &str = r#"class Num {}
extension B1<T extends Num> on List<T> {
  int get probe => 1;
}
extension B2<T extends Num> on List<T> {
  int get probe => 2;
}
int use(List<Num> xs) => xs.probe;
"#;

/// A formal default carrying type references (`= const <String, String>{}`)
/// must not leak its literal's type arguments into the armed slot: without
/// the `*_formal_parameter_default_value_expression` forwards the
/// `formal_default_depth` window never opens under the mux, `handle_type`
/// inside the literal arms `String`, and `end_formal_parameter`'s
/// `take_armed` captures it as the formal's declared type — the
/// starved-mux failure mode. The element model itself carries the
/// observable (`Formal.ty`), so this pin reads the facts directly rather
/// than waiting on a downstream diagnostic.
const DEFAULT_LITERAL: &str = r#"int pick([Map<String, String> m = const <String, String>{}]) {
  return m.length;
}
"#;

/// The code names reported for the file whose path ends with `tail`,
/// sorted and deduped.
fn codes_for(diagnostics: &[Diagnostic], engine: &Engine, tail: &str) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = diagnostics
        .iter()
        .filter(|diagnostic| {
            engine
                .file_table()
                .path(diagnostic.file)
                .is_some_and(|path: &Path| path.ends_with(tail))
        })
        .map(|diagnostic| diagnostic.code.name)
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

/// Feeds `source` as the workspace's only file through the real engine
/// path — `set_content` + `analyze`, the same multiplexed parse the CLI
/// runs — and returns the diagnostics.
fn analyze(source: &str) -> (Vec<Diagnostic>, Engine) {
    let fixture = Fixture::new(&[("lib/main.dart", source)]);
    let mut engine = fixture.engine();
    engine.set_content(&fixture.path("lib/main.dart"), source.as_bytes());
    let diagnostics = engine
        .analyze(
            &[],
            &mut |_| {},
            &dartq_engine::CancellationToken::default(),
        )
        .expect("analyze completes");
    (diagnostics, engine)
}

/// The headline divergence: bound-excluded sibling faces must drop out of
/// the application pool on the engine path exactly as they do in the
/// collector-only rigs — each read resolves through its own face and the
/// oracle reports nothing.
#[test]
fn bound_rejecting_faces_drop_on_the_engine_path() {
    let (diagnostics, engine) = analyze(BOUNDED_PAIR);
    assert_eq!(
        codes_for(&diagnostics, &engine, "main.dart"),
        Vec::<&'static str>::new(),
        "the bound-gated faces resolve silently: {diagnostics:?}"
    );
}

/// The bound gate's keep-direction: a `dynamic` instantiation fails every
/// concrete bound, both faces drop, and the access keeps its own
/// `undefined_getter` — the same verdict the oracle reports.
#[test]
fn raw_receiver_keeps_its_undefined_getter() {
    let (diagnostics, engine) = analyze(RAW_RECEIVER);
    assert_eq!(
        codes_for(&diagnostics, &engine, "main.dart"),
        vec!["undefined_getter"],
        "the raw receiver drops both faces and reports its own miss: {diagnostics:?}"
    );
}

/// The specificity tail still orders on bounds through the engine path:
/// equally-bounded twins remain mutually unordered and stay ambiguous —
/// the bounds arriving must be real types, not just non-empty (a
/// placeholder face would mis-dominate or mis-drop, never produce this
/// exact verdict).
#[test]
fn equally_bounded_twins_stay_ambiguous_on_the_engine_path() {
    let (diagnostics, engine) = analyze(EQUAL_BOUNDS);
    assert_eq!(
        codes_for(&diagnostics, &engine, "main.dart"),
        vec!["ambiguous_extension_member_access"],
        "equal bounds project equal — the ambiguity stands: {diagnostics:?}"
    );
}

/// The `formal_default_depth` window survives the mux: a default literal's
/// type arguments never reach the armed slot, so the formal's captured
/// type takes the same under-report reading the solo parse produces —
/// [`Ty::Unknown`] — and never the literal's last argument (`String`,
/// the starved-mux leak).
#[test]
fn default_literal_types_stay_out_of_the_formal_face() {
    let fixture = Fixture::new(&[("lib/main.dart", DEFAULT_LITERAL)]);
    let mut engine = fixture.engine();
    let main = fixture.path("lib/main.dart");
    engine.set_content(&main, DEFAULT_LITERAL.as_bytes());
    engine
        .analyze(
            &[],
            &mut |_| {},
            &dartq_engine::CancellationToken::default(),
        )
        .expect("analyze completes");
    let file = engine.file_table().try_file_id(&main).expect("file id");
    let facts = engine.file_facts(file).expect("facts");
    let strings = engine.strings();
    let pick = strings.intern("pick");
    let decl = facts
        .elements
        .declarations()
        .find(|(name, _)| *name == pick)
        .map(|(_, decl)| decl)
        .expect("pick declared");
    let formals = decl.formals.as_deref().expect("pick has formals");
    assert!(
        matches!(formals.positional[0].ty, dartq_semantic::model::Ty::Unknown),
        "the literal's type arguments never arm the formal's slot: {:?}",
        formals.positional[0]
            .ty
            .render(strings, facts.elements.type_args())
    );
}
