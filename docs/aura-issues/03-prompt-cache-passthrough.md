# Pass through prompt-cache controls and report `cached_tokens` per request

## Why
Kleene keeps a frozen system prefix (schema catalog and rules) and puts
one `cache_control` breakpoint on it. With Aura in the path the breakpoint
is dropped, so every call pays full input price, and the client cannot see
cache hits even though `Usage.cached_tokens` exists.

## Proposal
- Allow `cache_control: { type: "ephemeral", ttl?: "5m" | "1h" }` on input
  items and on `instructions`; map to Anthropic `cache_control`, to OpenAI's
  automatic caching (no-op), and ignore elsewhere.
- Populate `usage.cached_tokens` from the provider response
  (`cache_read_input_tokens` on Anthropic, `prompt_tokens_details.cached_tokens`
  on OpenAI) on every response, streaming included.
- Make sure Aura's own response cache key does not include the breakpoint
  metadata, so semantically equal requests still hit.

## Acceptance
- Two consecutive requests with the same cached prefix show
  `cached_tokens > 0` on the second, on Anthropic.
