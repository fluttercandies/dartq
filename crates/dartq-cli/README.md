# dartq-cli

The `dartq` command-line tool: fast Dart static analysis from the terminal.
The package is `dartq-cli`; the binary it ships is named `dartq`.

## Usage

```text
dartq check ...     # delegates to `dart analyze` (the default backend)
dartq analyze ...   # same delegation; `--engine` selects the in-process engine
dartq --help        # full command list
```

## Role in the workspace

A thin front end: argument parsing over the command cores in
`dartq-commands`, the daemon shell in `dartq-daemon`, and diagnostic
rendering from `dartq-diagnostics` and `dartq-engine`.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
