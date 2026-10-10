# dartq-source

dartq's content-addressed source layer. Owns file content storage, stable
file identity (`FileId`), content hashing, and line indexes; every other
crate in the workspace refers to files through these handles instead of raw
paths or strings.

## Role in the workspace

A leaf crate: it depends on no other `dartq-*` crate, and nearly every
sibling builds on it (scanner, parser, diagnostics, resolution, engine,
CLI, daemon). An optional `serde` feature enables serializing its value
types.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
