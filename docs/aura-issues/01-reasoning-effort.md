# Add a `reasoning` option to `CreateResponseRequest`

## Why
Kleene sets a per-call effort level (`low` for mapper and verifier
sub-calls, `high` or `xhigh` for the root session) and relies on it for cost
control. Through the Open Responses endpoint there is no field to carry it,
so `EFFORT` becomes a no-op when a gateway is in the path.

## Proposal
- `CreateResponseRequest.reasoning: Option<ReasoningOptions>` with
  `effort: Option<Effort>` (`minimal | low | medium | high | xhigh | max`)
  and, for providers that expose it, `summary: Option<Summary>`.
- Provider mapping: Anthropic -> `output_config.effort` plus adaptive
  thinking; OpenAI -> `reasoning.effort`; others ignore with a warning in
  `metadata.aura`.
- Echo the effective effort in `metadata.aura` so clients can verify it.

## Acceptance
- A request with `reasoning.effort = "low"` to an Anthropic-backed model
  produces a Messages API request carrying `output_config.effort = "low"`.
- Unsupported providers accept the field and report `reasoning_ignored`.
