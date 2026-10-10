# dartq-pub

Readers for pub's resolution artifacts — `package_config.json`,
`pubspec.yaml`, `pubspec.lock` — plus workspace discovery and the freshness
rules that decide when a configuration must be re-read (strict mtime
ordering; equal timestamps count as stale).

## Role in the workspace

Parses artifacts over the content layer (`dartq-source`) with
language-version support from `dartq-language`. `dartq-package` composes
its typed identity model on top, and `dartq-resolution` consumes both.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
