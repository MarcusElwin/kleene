# Aura issues to file

Drafts of upstream issues for `UmaiTech/aura-llm-gateway`, one per gap the
Open Responses gateway adapter in Kleene hits. File them when the adapter
work starts (M2); link the issue number back here.

| # | Title | Kleene feature blocked |
|---|---|---|
| 01 | Reasoning effort on the request | `EFFORT` per call through a gateway |
| 02 | JSON-schema structured output | `LLM_JSON`, `LLM_BOOL` without emulation |
| 03 | Prompt-cache pass-through and `cached_tokens` | cheap frozen prefix, cache-hit metric in the TUI |
| 04 | Embeddable `aura-providers` crate | in-process use of Aura's adapters with no gateway |
