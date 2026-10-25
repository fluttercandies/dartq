# Installing the dartq skill package

What you get: seven agent skills covering the dartq toolchain (AI-first
Dart/Flutter analysis, test driving, monorepo sessions, debugging, MCP),
plus the `dartq mcp` stdio server registration for hosts that support MCP.
One package, every harness — install it separately per harness if you use
more than one.

Prerequisites for every path below:

- The `dartq` binary on `PATH` (`dartq version` answers)
- A Dart SDK discoverable (`$DART_SDK`, or `--sdk`/`--sdk-path` per command)

In the commands, `<this-package>` is this directory (the one holding this
INSTALL.md — it contains `skills/` and `adapters/`). If you host this
package on a git remote, substitute the clone URL where a path is shown.

## Contents

- [Claude Code](#claude-code)
- [Codex](#codex)
- [Cursor](#cursor)
- [Gemini CLI](#gemini-cli)
- [Kimi Code](#kimi-code)
- [Antigravity](#antigravity)
- [Devin](#devin)
- [Hermes Agent](#hermes-agent)
- [Muse](#muse)
- [OpenCode](#opencode)
- [pi](#pi)
- [oh-my-pi (omp)](#oh-my-pi-omp)
- [Kiro](#kiro)
- [Any other harness (AGENTS.md convention)](#any-other-harness-agentsmd-convention)
- [Troubleshooting](#troubleshooting)

The MCP registration snippet for your harness (where supported) is part of
each section; the universal stdio shape is:

```json
{ "mcpServers": { "dartq": { "command": "dartq", "args": ["mcp"] } } }
```

Run it from inside the worktree you want bound — one server per worktree.

## Claude Code

Skills ship as a plugin (manifest: `adapters/claude-plugin/`):

```bash
claude plugin marketplace add <this-package>/adapters/claude-plugin
claude plugin install dartq@dartq
```

If your Claude Code version rejects the manifest's relative skills path,
assemble a flat plugin dir instead:

```bash
mkdir -p /tmp/dartq-claude && cd /tmp/dartq-claude
cp -R <this-package>/adapters/claude-plugin/.claude-plugin .
cp -R <this-package>/skills .
claude plugin marketplace add /tmp/dartq-claude
claude plugin install dartq@dartq
```

MCP:

```bash
claude mcp add dartq -- dartq mcp
```

Verify: ask "list your dartq skills" — a working install names `using-dartq`.

## Codex

Manifest: `adapters/codex/plugin.json` (`"skills": "../../skills"`). Point
Codex's plugin install at the adapter directory, or assemble the flat
layout (manifest at a root beside `skills/`) if your build requires it:

```bash
mkdir -p /tmp/dartq-codex && cd /tmp/dartq-codex
mkdir -p .codex-plugin && cp <this-package>/adapters/codex/plugin.json .codex-plugin/
cp -R <this-package>/skills .
# then install /tmp/dartq-codex through Codex's plugin interface
```

MCP (`~/.codex/config.toml`):

```toml
[mcp_servers.dartq]
command = "dartq"
args = ["mcp"]
```

## Cursor

Manifest: `adapters/cursor/plugin.json`. Install from a flat layout the
same way as Codex above (copy `.cursor-plugin/` beside `skills/`), or use
the plugin search in Cursor Agent chat and point it at this package.

MCP (`.cursor/mcp.json` or `~/.cursor/mcp.json`):

```json
{ "mcpServers": { "dartq": { "command": "dartq", "args": ["mcp"] } } }
```

## Gemini CLI

Extension manifest: `adapters/gemini/gemini-extension.json` with context
file `GEMINI.md` (it @-imports the `using-dartq` index skill). Install:

```bash
gemini extensions install <this-package>/adapters/gemini
```

MCP:

```bash
gemini mcp add dartq -- dartq mcp
```

## Kimi Code

Manifest: `adapters/kimi/plugin.json` — it points at the shared skills and
sets the session-start skill to `using-dartq`. Install through Kimi Code's
plugin manager (`/plugins`) from this package (or a hosted git URL), or
copy the skills directory into your Kimi skills location.

MCP: register `dartq mcp` in Kimi Code's MCP settings (stdio:
command `dartq`, args `["mcp"]`).

## Antigravity

Metadata manifest: `adapters/antigravity/plugin.json`. Install from a
hosted git URL of this package through Antigravity's plugin install; the
manifest lists the skills through the package layout. Reinstall with the
same command to update.

MCP: add a stdio server `dartq` with args `["mcp"]` in Antigravity's MCP
settings.

## Devin

Metadata manifest: `adapters/devin/plugin.json`. Install from a hosted git
URL of this package:

```bash
devin plugins install <git-url-of-this-package>
```

Update later: `devin plugins update dartq`.

## Hermes Agent

Manifest: `adapters/hermes/plugin.yaml` (skills-only; no hooks). Install
from a checkout:

```bash
hermes plugins install <this-package>/adapters/hermes --enable
```

Restart active Hermes sessions after installing.

## Muse

Native manifest: `adapters/muse/.muse-plugin/plugin.json` (explicit skill
paths) and `marketplace.json` beside it. Install from a checkout:

```bash
muse plugins install <this-package>/adapters/muse
muse plugins approve dartq
```

Update later: `muse plugins update dartq`. Restart active sessions so the
skills register.

## OpenCode

See `adapters/opencode/INSTALL.md` (assembles a flat plugin dir with the
manifest beside `skills/`, registers it in the OpenCode config, and adds
the MCP entry).

## pi

pi natively discovers `skills/` inside an installed package — the
conventional directory is all this package needs. Install it (local path,
git URL, or npm spec all work):

```bash
pi install <this-package>
```

For development, load it for one session without installing:

```bash
pi -e <this-package>
```

pi lists each skill's name, description, and path in the system prompt;
invoke one with `/skill:using-dartq` or let the model load the matching
skill for the task. MCP: register `dartq mcp` per pi's MCP configuration
(stdio: command `dartq`, args `["mcp"]`).

## oh-my-pi (omp)

omp natively discovers `skills/` inside installed packages; the root
`package.json` carries the `omp` manifest that marks the package as an
omp plugin. Install from a checkout:

```bash
omp install <this-package>
```

Installs from a local path are linked, so edits flow through — start a
new session to pick them up. Verify offline (no session needed):

```bash
omp skill list   # the dartq skills should be listed
```

MCP: in an omp session run `/mcp add dartq -- dartq mcp`.

## Kiro

Kiro consumes steering files from `.kiro/steering/` in the workspace (or
`~/.kiro/steering/` globally). Copy the steering document into your
project:

```bash
mkdir -p .kiro/steering
cp <this-package>/adapters/kiro/steering/dartq.md .kiro/steering/dartq.md
```

The file carries `inclusion: always` frontmatter (allowed values:
`always`, `fileMatch` + `fileMatchPattern`, `manual`, `auto`), so Kiro
loads it into every interaction; it summarizes the dartq contract with
pointers to the skill files. For the skills themselves, Kiro reads
markdown guidance — the steering file links the set; hosts that support
the skills convention can consume `skills/` directly.

MCP: `.kiro/settings/mcp.json` (workspace) or `~/.kiro/settings/mcp.json`
(user) — open it via the command palette ("Kiro: Open workspace MCP
config (JSON)") or the Open MCP Config icon in the Kiro panel:

```json
{ "mcpServers": { "dartq": { "command": "dartq", "args": ["mcp"] } } }
```

Configs hot-reload on save.

## Any other harness (AGENTS.md convention)

- Skills: point the harness at `<this-package>/skills` (each
  `<name>/SKILL.md` follows the agentskills.io convention: YAML frontmatter
  with `name` + `description`).
- Guidance: drop `adapters/agents-md/AGENTS.md` at your project root (or
  reference it from your existing AGENTS.md).
- MCP: the universal stdio shape above.

## Troubleshooting

- **Verify the binary before anything else:** `dartq version`.
- **Verify skills landed:** ask the agent "list your dartq skills" — the
  `using-dartq` index should be named. On omp you can verify offline with
  `omp skill list`.
- **Relative skills paths** (`../../skills` in the claude/codex/cursor/
  kimi manifests): the flat-dir assembly steps exist because some host
  versions do not traverse upward from a manifest. If a host rejects the
  path, use the assembly variant shown in that section.
- **Kiro note:** verified against kiro.dev docs — steering frontmatter
  `inclusion` takes `always` (default), `fileMatch` + `fileMatchPattern`,
  `manual`, or `auto` (requires `name` + `description`); steering lives in
  `.kiro/steering/` (workspace) or `~/.kiro/steering/` (global), and MCP
  servers are configured in `.kiro/settings/mcp.json` /
  `~/.kiro/settings/mcp.json` (workspace wins on merge). Note: custom
  Kiro agents do not include steering files automatically — add
  `"resources": ["file://.kiro/steering/**/*.md"]` to the agent config.
- **omp/pi note:** verified end to end against a local omp 18.10.4
  install — both agents discover a conventional `skills/` directory
  inside an installed package natively (omp reports the skills with
  source `omp-plugins`); no extension code is needed. The root
  `package.json`'s `omp` manifest is required for omp (`omp plugin
  doctor` flags its absence); pi reads the same layout convention and
  ignores the `omp` field.
- **Wrong workspace over MCP:** the server binds the worktree it was
  started in — restart `dartq mcp` inside the right tree, and check the
  `dartq://workspace-status` resource.
