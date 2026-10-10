# dartq-index

dartq's symbol index: the unlinked declaration surface and the per-library
reference surface that power symbol lookup, navigation, and impact queries
without re-resolving the world.

## Role in the workspace

Built over parsed and resolved inputs (`dartq-parser`, `dartq-resolution`,
`dartq-scanner`, `dartq-source`); consumed by `dartq-ai`'s agent-facing
queries and by the engine's query API.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
