# dartq-lints

The built-in lint rule engine: the syntax-visible slice of the
`package:lints` recommended and core sets, complete to the member level.
One traversal dispatches enabled rules over the scan/parse substrate and
applies `// ignore:` / `// ignore_for_file:` suppression at the sink.

## Role in the workspace

Reads the substrate facts produced by `dartq-scanner`, `dartq-parser`, and
`dartq-ast`'s arena AST; every rule cites its official `pkg/linter`
implementation and message template. `dartq-commands` wires the enabled
set into the `analyze`, `lints`, and `fix` faces.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
