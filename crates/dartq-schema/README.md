# dartq-schema

Machine-readable JSON schemas for dartq's versioned output faces, plus the
lightweight structural validator that CI runs against real produced output.
The crate has zero runtime dependencies on other `dartq-*` crates: the
schemas are the contract, and producers are validated against them in
tests.

## Role in the workspace

Depends only on `serde_json`. Every face that emits versioned JSON (daemon
session answers, engine outputs) is pinned to the schemas published here.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
