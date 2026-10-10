# dartq-flow

Flow analysis for dartq: type promotion and demotion, definite assignment
(including `late` variables), and reachability, computed over the parser's
listener event stream.

## Role in the workspace

Consumes parse events from `dartq-parser` over `dartq-scanner` tokens and
`dartq-source` content; its results feed `dartq-semantic`'s diagnosis pass.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
