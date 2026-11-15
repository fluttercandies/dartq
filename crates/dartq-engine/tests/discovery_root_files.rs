//! Discovery root-level pins (goal #324): a `*.dart` file directly in the
//! package root is interned like any surface file. The walk queue holds
//! only directories — queueing a root file would make `read_dir` fail with
//! ENOTDIR and silently drop the very entry files single-file (bin-style)
//! packages analyze. The options exclude applies at the root too: a
//! root-level file matching an exclude glob never enters the analysis set.

#![allow(
    clippy::useless_conversion,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::type_complexity,
    clippy::format_push_string,
    clippy::manual_let_else,
    clippy::default_trait_access,
    clippy::ptr_arg,
    clippy::needless_pass_by_value,
    clippy::match_same_arms,
    clippy::assert_is_empty,
    clippy::used_underscore_binding,
    clippy::items_after_statements,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::doc_lazy_continuation,
    clippy::unnecessary_get_then_check,
    clippy::match_wildcard_for_single_variants,
    clippy::naive_bytecount,
    clippy::sliced_string_as_bytes,
    clippy::nonminimal_bool,
    clippy::semicolon_if_nothing_returned
)]
mod common;

use common::Fixture;

#[test]
fn root_level_dart_files_are_discovered_and_excludable() {
    let fixture = Fixture::new(&[
        // A member-level expression error: the same minimal shape the
        // reproducibility corpus pins as surfacing diagnostically at L1
        // (the diet parser skips block bodies, so an unclosed brace at EOF
        // stays silent).
        ("main.dart", "int broken() => ;\n"),
        ("tool.dart", "void tool() {}\n"),
    ]);
    std::fs::write(
        fixture.path("analysis_options.yaml"),
        "analyzer:\n  exclude:\n    - \"tool.dart\"\n",
    )
    .expect("options");

    let mut engine = fixture.engine();
    let stats = engine.discover_workspace().expect("discovery");
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &dartq_engine::CancellationToken::new())
        .expect("analyze");

    assert_eq!(stats.file_count, 1, "only main.dart enters the set");
    let main_id = engine
        .file_table()
        .try_file_id(&fixture.path("main.dart"))
        .expect("root-level main.dart must be interned");
    assert_eq!(
        engine.content(main_id),
        Some("int broken() => ;\n".as_bytes()),
        "root file bytes resident"
    );
    assert!(
        engine
            .file_table()
            .try_file_id(&fixture.path("tool.dart"))
            .is_none(),
        "excluded root-level file must not be interned"
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            engine
                .file_table()
                .path(diagnostic.file)
                .is_some_and(|path| path.ends_with("main.dart"))
        }),
        "the root file's syntax error surfaces: {diagnostics:?}"
    );
}
