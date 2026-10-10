# dartq-plugin

The dartq plugin protocol: the IPC contract between the Rust core and the
per-plugin Dart host process — wire framing, handshake, capability
negotiation, and diagnostic/fix messages (newline-delimited JSON).

## Role in the workspace

Defines the protocol as Rust types over `dartq-diagnostics` and
`dartq-source`; this crate is the single source of truth for the contract.
The host side of the same contract lives in the Dart package
`crates/dartq-plugin-host` (deliberately not part of the Cargo workspace),
and the daemon runs plugin sessions on top of this protocol.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
