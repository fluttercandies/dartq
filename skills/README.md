# dartq agent skills

An AI-agent skill package for dartq — the AI-first, drop-in toolchain for
Dart and Flutter analysis, testing, and debugging.
Skills teach coding agents the dartq workflow; the `dartq mcp` stdio server
exposes the same engine as 28 MCP tools.

## What is in here

- `skills/` — the skill set (agentskills.io convention: one
  `<name>/SKILL.md` per skill, YAML frontmatter with `name` +
  `description`, optional `references/` for depth)
- `adapters/` — thin per-harness manifests for 14 agent hosts
  (Claude Code, Codex, Cursor, Gemini CLI, Kimi Code, Antigravity, Devin,
  Hermes, Muse, OpenCode, pi, oh-my-pi, Kiro, and the generic AGENTS.md
  convention); every adapter points at the shared skill set — no
  duplicated content

## The skill

One skill, `dartq`, is the single entry point: the core contract, the verb
surface, and a router into per-face reference docs under
`skills/dartq/references/` (hosts inject one description per turn — one
product means one skill entry, depth lives in references):

| Reference | Depth |
| --- | --- |
| `references/getting-started.md` | install, first run, orienting in an unfamiliar repo |
| `references/test-driving.md` | running tests, cache questions, `--impacted` edit-test loops |
| `references/analysis-loop.md` | diagnose → understand → scope → execute → verify on analyzed code |
| `references/sessions-and-monorepos.md` | resident sessions, nested worktrees, monster monorepos |
| `references/debugging.md` | launching/attaching, breakpoints, hot reload — no IDE |
| `references/mcp.md` | wiring `dartq mcp` into an agent host, CLI vs MCP |

Four deeper references sit beside them: `cache-and-markers.md`,
`formats-and-tiers.md`, `session-model.md`, and `tool-catalog.md`.

## Install

See [INSTALL.md](INSTALL.md) — one section per harness with exact commands
and the MCP registration step.
