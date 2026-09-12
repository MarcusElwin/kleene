# Split `aura-core::provider` into an embeddable `aura-providers` crate

## Why
The provider adapters (OpenAI, Anthropic, Google, Mistral, Together,
Fireworks, Ollama, HF TGI, Bedrock) are the part of Aura other Rust programs
want to embed when no gateway process is running. Today they come with
`aura-db` (SQLx/Postgres), Redis and the AWS SDK because `aura-core` depends
on all of them.

## Proposal
- New crate `crates/aura-providers` containing `Provider`, `ProviderError`,
  `EventStream`, the adapters and the wire mapping, depending only on
  `aura-types`, `reqwest`, `serde`, `tokio`, `futures-util`, `async-trait`.
- Feature flags per provider (`anthropic`, `openai`, `bedrock`, ...); Bedrock
  behind its feature so the AWS SDK is opt-in.
- `aura-core` re-exports it, so nothing changes for `aura-proxy`.
- Publish `aura-types` and `aura-providers` to crates.io once stable.

## Acceptance
- `cargo build -p aura-providers --no-default-features --features anthropic`
  compiles without SQLx, Redis or AWS crates in the lockfile.
