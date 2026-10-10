# dartq-cli

The `dartq` command-line tool: fast Dart static analysis from the terminal.
The package is `dartq-cli`; the binary it ships is named `dartq`.

## Usage

```text
dartq check ...     # differential face against `dart analyze`
dartq analyze ...   # full engine analysis
dartq --help        # full command list
```

## Role in the workspace

A thin front end: argument parsing over the command cores in
`dartq-commands`, the daemon shell in `dartq-daemon`, and diagnostic
rendering from `dartq-diagnostics` and `dartq-engine`.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
