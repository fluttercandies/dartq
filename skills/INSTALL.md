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

The pi extension `adapters/pi/extensions/dartq.ts` registers the shared
skills directory through `resources_discover`. Package it (or point a
package at this checkout) and install:

```bash
pi install <this-package>/adapters/pi
```

For development, load it temporarily:

```bash
pi -e <this-package>/adapters/pi
```

The extension is dependency-free, so it runs without installing agent
types. MCP: register `dartq mcp` per pi's MCP configuration.

## oh-my-pi (omp)

omp package: `adapters/omp/` — `package.json` declares
`omp.extensions: ["./extensions/dartq.ts"]`, and that extension registers
the shared skills directory (the skills live outside the adapter, so the
extension resolves them explicitly). Install:

```bash
omp install <this-package>/adapters/omp
```

or for development: `omp --extension <this-package>/adapters/omp`. Start a
new session after install. MCP: add `dartq mcp` as a stdio server in your
omp MCP configuration.

## Kiro

Kiro consumes steering files. Copy the steering document into your
project:

```bash
mkdir -p .kiro/steering
cp <this-package>/adapters/kiro/steering/dartq.md .kiro/steering/dartq.md
```

The file
carries `inclusion: always` frontmatter and summarizes the dartq contract
with pointers to the skill files. For the skills themselves, Kiro reads
markdown guidance — the steering file links the set; hosts that support
the skills convention can consume `skills/` directly. MCP: register
`dartq mcp` in Kiro's MCP settings (stdio: command `dartq`, args
`["mcp"]`).

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
  `using-dartq` index should be named.
- **Relative skills paths** (`../../skills` in the claude/codex/cursor/
  kimi manifests): the flat-dir assembly steps exist because some host
  versions do not traverse upward from a manifest. If a host rejects the
  path, use the assembly variant shown in that section.
- **Kiro confidence note:** the steering markdown (with `inclusion`
  frontmatter) is the documented extension point; no config-file shape for
  MCP is shipped — register the server through Kiro's settings UI.
- **omp/pi confidence note:** the extensions follow the documented
  `resources_discover` → `skillPaths` registration (pi) and
  `omp.extensions` manifest field (omp); skills are registered explicitly
  because the package keeps them in a shared directory rather than the
  conventional root-level `skills/` of the adapter.
- **Wrong workspace over MCP:** the server binds the worktree it was
  started in — restart `dartq mcp` inside the right tree, and check the
  `dartq://workspace-status` resource.
