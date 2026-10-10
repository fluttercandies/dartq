# dartq-language

Single source of truth for Dart language feature versioning in dartq. Maps
an SDK constraint and a library's language version to the enabled feature
set, so all later phases — scanning, parsing, analysis — agree on which
language version a library is analyzed under.

## Role in the workspace

A leaf crate (no `dartq-*` runtime dependencies) consumed by the scanner,
AST, SDK metadata, pub artifact, and parser crates. Version tables are
generated at build time from versioned YAML data.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
