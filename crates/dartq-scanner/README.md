# dartq-scanner

Full Dart 3.15 lexical scanning: UTF-8 source in, a linear token table plus
syntax diagnostics out. A hand-written state machine tuned for throughput
and for error recovery that matches the official analyzer's observable
behavior.

## Role in the workspace

Sits on `dartq-source` (content and line indexing) and `dartq-language`
(language-version gating). The token table feeds `dartq-ast` and
`dartq-parser`; scanner-phase diagnostics are carried in
`dartq-diagnostics`' data model.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
