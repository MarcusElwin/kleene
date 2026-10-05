# Configuration

Nothing that touches a model runs until one credential is set. Kleene speaks the wire formats directly over HTTPS, with no SDK and no gateway required.

## The wizard

```bash
kleene setup
```

It asks which providers to use (a model provider, and optionally the service behind `web_search`), takes the keys with the input masked, and writes them to the config file. The TUI runs the same wizard on its first start without a provider, and `/setup` at the prompt reopens it; saving there asks the running daemon to reload, so the next run uses the new keys without a restart.

Non-interactive forms for scripts:

```bash
kleene setup --anthropic-key sk-ant-...
kleene setup --openai-base-url http://localhost:11434/v1 --openai-model llama3
kleene setup --router ~/.config/kleene/router.toml
kleene setup --web-search-provider brave --web-search-key BSA...   # or tavily, exa, linkup
kleene setup --show          # what is configured, keys masked, and from where
```

## The config file

`config.toml` in `$KLEENE_CONFIG_DIR`, else `$XDG_CONFIG_HOME/kleene`, else `~/.config/kleene`, written owner-readable only:

```toml
[anthropic]
api_key = "sk-ant-..."

[openai_compat]
base_url = "http://localhost:11434/v1"
model = "llama3"

[web_search]
provider = "brave"      # or "tavily", "exa", "linkup"
api_key = "BSA..."
```

Environment variables win over the file, field by field, so a one-off `ANTHROPIC_API_KEY=... kleene run` keeps working and CI never needs the file.

## Environment variables

| Variable | Meaning |
|---|---|
| `ANTHROPIC_API_KEY` (or `ANTHROPIC_AUTH_TOKEN`) | Anthropic key; `ANTHROPIC_BASE_URL` overrides the endpoint |
| `OPENAI_API_KEY` | OpenAI key |
| `OPENAI_BASE_URL` | any OpenAI-compatible endpoint; a local server needs only this, no key |
| `OPENAI_MODEL` | the model every alias resolves to on that endpoint (default `gpt-5.4-mini`) |
| `KLEENE_ROUTER_TOML` | a router file for your own aliases, failover and pricing |
| `KLEENE_WEB_SEARCH_API_KEY`, `KLEENE_WEB_SEARCH_PROVIDER` | the service behind `web_search`: `brave` (default), `tavily`, `exa` or `linkup` |
| `KLEENE_CONFIG_DIR` | where `config.toml` lives |

With only Anthropic configured the default routing is: `root` on Opus 5.5 at high effort, `worker` and `judge` on Sonnet 5, `proxy` on Haiku 4.5 at low effort, prompt prefix cached everywhere, priced from the first-party rate card. With only an OpenAI-compatible endpoint, every alias resolves to `OPENAI_MODEL` and nothing is priced, so dollar budgets and estimates read zero until you add pricing.

## Aliases and the router file

Model calls name an **alias** (`root`, `worker`, `proxy`, `judge`, or your own), never a model id. `SET model.default = 'worker'`, `CREATE FUNCTION ... MODEL 'proxy'` and `CREATE AGENT ... MODEL 'worker'` all refer to aliases. A router file maps each alias to an ordered list of candidates with options and prices:

```toml
[aliases.root]
candidates = [{ provider = "anthropic", model = "claude-opus-5-5" }]
options = { effort = "high", cache_prefix = true }

[aliases.worker]
candidates = [
  { provider = "anthropic", model = "claude-sonnet-5" },
  { provider = "openai_compat", model = "gpt-5.4-mini" },   # failover
]
options = { effort = "low" }

[aliases.proxy]
candidates = [{ provider = "anthropic", model = "claude-haiku-4-5" }]

[aliases.judge]
candidates = [{ provider = "anthropic", model = "claude-sonnet-5" }]

[pricing."claude-opus-5-5"]
input_per_mtok = 4.0
output_per_mtok = 20.0
cache_read_per_mtok = 0.4
cache_write_per_mtok = 5.0
```

Candidates are tried in order; one whose calls keep failing is skipped until its circuit breaker cools down. `options` takes `effort`, `cache_prefix`, `temperature` and provider-specific `extra`.

## Web search

`web_search(q [, n])` calls Brave, Tavily, Exa or Linkup with the configured key and returns `rank, title, url, snippet`, ten rows unless `n` says otherwise, at most twenty. Without a key every search fails with `no web search backend configured`.

## MCP servers and skills

`kleene mcp` lists connected MCP servers and their tools, and `kleene mcp add <name> <cmd> ...` adds one to `mcp.json`; their tools appear in the catalog as `<server>_<tool>`. `kleene skills` lists the skills sessions can read (`SKILL.md` files, six built in, plus per-user and per-project ones), and an `AGENTS.md` in the workspace is read as project instructions.

Full reference: [`docs/CLI.md`, "Configuring a model provider"](https://github.com/MarcusElwin/kleene/blob/main/docs/CLI.md#configuring-a-model-provider).
