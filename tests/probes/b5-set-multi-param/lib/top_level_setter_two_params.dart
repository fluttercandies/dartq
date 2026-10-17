// B5 top-level face: top-level setter with two parameters.
//
// The official `_ensureSetterFormalParameter` call site for top-level
// declarations (upstream Dart SDK source,
// pkg/analyzer/lib/src/fasta/ast_builder.dart:3439) applies the same
// once-per-setter rule as the class-member arm. This file exercises the
// top-level path, which is a separate emit site in the dartq parser, so
// the pinned oracle row count covers both parser paths.
set top(a, b) {}
