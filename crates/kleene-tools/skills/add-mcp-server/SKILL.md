---
name: add-mcp-server
description: Connect an MCP server so its tools appear in the catalog as table functions and CALL statements, and check it with /mcp
---

# Add an MCP server

Kleene speaks the Model Context Protocol over stdio. Each server's tools
join the catalog under `<server>_<tool>`: a tool whose annotations say it
is read-only is a table function used in `FROM`; every other tool is a
side effect and runs as `CALL`.

## Configure

Servers are listed in `mcp.json`, per user at
`$KLEENE_CONFIG_DIR/mcp.json` (default `~/.config/kleene/mcp.json`) or per
project at `.kleene/mcp.json`; both load, the project file wins on a name
clash. The format is the one every MCP client uses:

```json
{
  "mcpServers": {
    "fs": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."] },
    "gh": { "command": "github-mcp-server", "args": ["stdio"], "env": { "GITHUB_TOKEN": "..." } }
  }
}
```

From the TUI, `/mcp add fs npx -y @modelcontextprotocol/server-filesystem .`
writes the user file and reloads; `/mcp remove fs` drops it; `/mcp` lists
every server with its state and tools. From the shell: `kleene mcp add`,
`kleene mcp remove`, `kleene mcp list`.

## Use

```sql
SELECT text FROM fs_read_file('README.md');
CALL gh_create_issue('owner/repo', 'title', 'body');
```

Arguments are positional, in the order of the tool's required parameters
and then its optional ones; the catalog line shows the signature. Each
result row is one text item the server returned; a structured result
arrives as JSON text.

A server that fails to start is listed by `/mcp` with the error and its
tools are simply absent; nothing else changes.
