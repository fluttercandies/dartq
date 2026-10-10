# dartq-commands

The per-command cores shared by the dartq CLI and the daemon RPC faces:
plain parameters in, typed results out, one implementation per command.

## Role in the workspace

Sits above nearly every analysis crate (parser, semantic, lints, index,
ai, and friends) and is consumed by `dartq-cli` (argument parsing) and
`dartq-daemon` (RPC dispatch), so the two faces cannot drift apart.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
