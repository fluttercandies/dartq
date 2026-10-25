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

## The skill set

| Skill | Load when |
| --- | --- |
| `using-dartq` | working in any Dart/Flutter repo — the index and the core contract |
| `dartq-getting-started` | installing, first run, orienting in an unfamiliar repo |
| `dartq-test-driving` | running tests, cache questions, `--impacted` edit-test loops |
| `dartq-analysis-loop` | diagnose → understand → scope → execute → verify on analyzed code |
| `dartq-sessions-and-monorepos` | resident sessions, nested worktrees, monster monorepos |
| `dartq-debugging` | launching/attaching, breakpoints, hot reload — no IDE |
| `dartq-mcp` | wiring `dartq mcp` into an agent host, CLI vs MCP |

## Install

See [INSTALL.md](INSTALL.md) — one section per harness with exact commands
and the MCP registration step.
