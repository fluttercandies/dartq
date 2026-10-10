# dartq-ai

Agent-facing analysis surfaces for dartq: symbol queries, impact analysis,
explain, compatibility checks, and fix suggestions, exposed as plain typed
results.

## Role in the workspace

Composes `dartq-index` (declaration and reference surfaces) over
`dartq-source`; the command cores in `dartq-commands` expose these queries
to both the CLI and the daemon, so both faces answer identically.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
