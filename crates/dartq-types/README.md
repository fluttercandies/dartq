# dartq-types

The Dart type system's analysis surface in dartq: the ten-shape type
skeleton, nullability as a dimension, canonical interning, and the type
queries the semantic phase needs.

## Role in the workspace

A near-leaf crate (no `dartq-*` dependencies) so type values stay cheap to
intern and share; consumed by `dartq-semantic`.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
