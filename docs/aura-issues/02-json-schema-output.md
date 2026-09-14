# Structured output: JSON-schema constrained responses

## Why
Kleene's `LLM_JSON(prompt, schema)` and `LLM_BOOL(prompt)` need the
model's output to validate against a schema. Anthropic exposes
`output_config.format`, OpenAI exposes `text.format` with `json_schema`.
Open Responses through Aura has no equivalent, so the client falls back to a
single forced function tool and validates the arguments itself.

## Proposal
- `CreateResponseRequest.text: Option<TextOptions>` with
  `format: { type: "json_schema", name, schema, strict }`.
- Provider mapping to each vendor's native structured-output feature; where
  none exists, emulate with a forced function tool inside the provider and
  return the parsed arguments as the message text.
- Report `metadata.aura.structured_output = native | emulated | unsupported`.

## Acceptance
- A request with a schema returns output that validates against it on
  Anthropic and OpenAI backends; emulation works on at least one other.
