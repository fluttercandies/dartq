# dartq-parser

Fault-tolerant recursive-descent parser for Dart: the scanner's token table
in, parser-listener events out. Parse functions mirror the official CFE
parser one-to-one, every error flows through listener events (the parser
collects no diagnostics itself, so speculation stays free of phantom
errors), and recovery is structurally guaranteed to terminate on any input.

## Role in the workspace

Sits on `dartq-scanner` (token table), `dartq-source` (spans and line
indexing), `dartq-language` (language-version gating), and
`dartq-diagnostics` (range types). The event stream feeds `dartq-semantic`,
`dartq-lints`, `dartq-flow`, and `dartq-index`.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
