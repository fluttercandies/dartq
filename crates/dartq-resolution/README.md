# dartq-resolution

URI and import resolution for dartq: `import`/`export`/`part` targets,
conditional imports, and namespace computation for a library's scope.

## Role in the workspace

Resolves against packages (`dartq-package`, `dartq-pub`), the SDK
(`dartq-sdk`), and content (`dartq-source`), reporting unresolvable
references through `dartq-diagnostics`; consumed by the index and semantic
phases.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
