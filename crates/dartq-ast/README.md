# dartq-ast

dartq's compact arena syntax tree. Nodes hold token-range references and
never stored offsets: spans derive from the token table at access time,
which keeps nodes small and the tree cheap to build and retain at scale.

## Role in the workspace

Built on the scanner's token table (`dartq-scanner`) over content from
`dartq-source`, with language gating from `dartq-language`; consumed by
analysis phases that walk syntax, currently `dartq-lints`.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
