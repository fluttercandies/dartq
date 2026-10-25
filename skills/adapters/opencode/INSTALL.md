# Installing dartq skills for OpenCode

## Prerequisites

- [OpenCode](https://opencode.ai) installed (V2 requires 2.0.4 or later)
- The `dartq` binary on `PATH` (for the skills' commands and `dartq mcp`)

## Skills

The skill files live in the shared `skills/` directory one level up from
this adapter (`adapters/opencode/`). OpenCode discovers skills through
plugin packages, so assemble a flat plugin directory (manifest beside a
`skills/` folder) and register it:

```bash
dartq-skills=/path/to/this/package          # holds INSTALL.md, skills/, adapters/
mkdir -p ~/.local/share/dartq-opencode
cp "$dartq-skills"/adapters/opencode/package.json \
   ~/.local/share/dartq-opencode/package.json
cp -R "$dartq-skills"/skills \
   ~/.local/share/dartq-opencode/skills
```

Then register the local plugin in your OpenCode configuration:

```json
{
  "plugins": ["~/.local/share/dartq-opencode"]
}
```

Restart OpenCode. (Publishing the package to a git URL and registering
`"<name>@git+<url>"` works the same way once you host it.)

## MCP (optional, recommended)

Register `dartq mcp` as a stdio MCP server in your OpenCode configuration:

```json
{
  "mcp": {
    "dartq": {
      "type": "local",
      "command": ["dartq", "mcp"]
    }
  }
}
```

## Verify

```bash
dartq version
```

Then ask OpenCode: "List your dartq skills." A working install answers
with the `using-dartq` index and the topic skills.
