//! Fixture-backed providers.
//!
//! [`RecordingProvider`] wraps a real provider and writes every response to a
//! fixture directory keyed by the request fingerprint. [`ReplayProvider`] reads
//! them back, so the whole task suite runs offline and a demo query costs
//! nothing the second time. The same directory holds decision fixtures:
//! [`RecordingDecisions`] records a [`DecisionProvider`] and
//! [`ReplayProvider`] replays those too (a decision fingerprint never
//! collides with a completion's, they hash different prefixes).

use crate::decision::{DecisionProvider, DecisionRequest, DecisionResponse};
use crate::types::{Capabilities, CompletionRequest, CompletionResponse, ProviderError};
use crate::Provider;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Serves recorded responses; errors on anything unrecorded.
pub struct ReplayProvider {
    dir: PathBuf,
}

impl ReplayProvider {
    /// Replay fixtures from `dir` (one `<fingerprint>.json` per response).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path_for(dir: &Path, fp: &str) -> PathBuf {
        dir.join(format!("{fp}.json"))
    }

    /// Read a fixture if present.
    pub async fn lookup(&self, req: &CompletionRequest) -> Option<CompletionResponse> {
        let path = Self::path_for(&self.dir, &req.fingerprint());
        let bytes = tokio::fs::read(&path).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

#[async_trait::async_trait]
impl Provider for ReplayProvider {
    fn name(&self) -> &str {
        "replay"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: false,
            tools: true,
            json_schema: true,
            prompt_cache: false,
            reasoning_control: true,
            cost_reported: true,
        }
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let fp = req.fingerprint();
        self.lookup(&req).await.ok_or(ProviderError::NoFixture(fp))
    }
}

#[async_trait::async_trait]
impl DecisionProvider for ReplayProvider {
    fn name(&self) -> &str {
        "replay"
    }

    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, ProviderError> {
        let fp = req.fingerprint();
        let path = Self::path_for(&self.dir, &fp);
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|_| ProviderError::NoFixture(fp.clone()))?;
        serde_json::from_slice(&bytes).map_err(|e| ProviderError::Malformed(e.to_string()))
    }
}

/// Wraps a decision provider and records every response as a fixture, next
/// to the completions a [`RecordingProvider`] on the same directory writes.
pub struct RecordingDecisions {
    inner: Arc<dyn DecisionProvider>,
    dir: PathBuf,
}

impl RecordingDecisions {
    /// Record `inner`'s answers into `dir`.
    pub fn new(inner: Arc<dyn DecisionProvider>, dir: impl Into<PathBuf>) -> Self {
        Self {
            inner,
            dir: dir.into(),
        }
    }
}

#[async_trait::async_trait]
impl DecisionProvider for RecordingDecisions {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, ProviderError> {
        let fp = req.fingerprint();
        let resp = self.inner.decide(req).await?;
        write_fixture(&self.dir, &fp, &resp).await?;
        Ok(resp)
    }
}

async fn write_fixture<T: serde::Serialize>(
    dir: &Path,
    fp: &str,
    value: &T,
) -> Result<(), ProviderError> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| ProviderError::Other(e.to_string()))?;
    let path = ReplayProvider::path_for(dir, fp);
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|e| ProviderError::Other(e.to_string()))?;
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| ProviderError::Other(e.to_string()))
}

/// Wraps a provider and records every response as a fixture.
pub struct RecordingProvider {
    inner: Arc<dyn Provider>,
    dir: PathBuf,
}

impl RecordingProvider {
    /// Record `inner`'s responses into `dir`.
    pub fn new(inner: Arc<dyn Provider>, dir: impl Into<PathBuf>) -> Self {
        Self {
            inner,
            dir: dir.into(),
        }
    }
}

#[async_trait::async_trait]
impl Provider for RecordingProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let fp = req.fingerprint();
        let resp = self.inner.complete(req).await?;
        write_fixture(&self.dir, &fp, &resp).await?;
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, Message, ProviderOptions, StopReason, Usage};
    use kleene_core::ModelAlias;

    struct Canned;

    #[async_trait::async_trait]
    impl Provider for Canned {
        fn name(&self) -> &str {
            "canned"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        async fn complete(
            &self,
            req: CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            Ok(CompletionResponse {
                model: req.model,
                content: vec![ContentBlock::Text {
                    text: "canned answer".into(),
                }],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            })
        }
    }

    fn req() -> CompletionRequest {
        CompletionRequest {
            alias: ModelAlias::worker(),
            model: "m".into(),
            system: String::new(),
            messages: vec![Message::user("hi")],
            tools: vec![],
            output_schema: None,
            max_tokens: 16,
            options: ProviderOptions::default(),
        }
    }

    #[tokio::test]
    async fn record_then_replay() {
        let dir = tempfile::tempdir().unwrap();
        let rec = RecordingProvider::new(Arc::new(Canned), dir.path());
        let first = rec.complete(req()).await.unwrap();
        let replay = ReplayProvider::new(dir.path());
        let second = replay.complete(req()).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(second.text(), "canned answer");
    }

    struct CannedDecisions;

    #[async_trait::async_trait]
    impl DecisionProvider for CannedDecisions {
        fn name(&self) -> &str {
            "canned"
        }
        async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, ProviderError> {
            Ok(DecisionResponse {
                model: "jev-test".into(),
                answers: req
                    .questions
                    .keys()
                    .map(|k| (k.clone(), crate::decision::Answer::Noul { noul: 0.75 }))
                    .collect(),
                usage: Usage::default(),
            })
        }
    }

    #[tokio::test]
    async fn decisions_record_then_replay_beside_completions() {
        let dir = tempfile::tempdir().unwrap();
        let req = || {
            DecisionRequest::single(
                serde_json::json!("state"),
                crate::decision::Question::noul("ok?"),
            )
        };
        let rec = RecordingDecisions::new(Arc::new(CannedDecisions), dir.path());
        let first = rec.decide(req()).await.unwrap();
        RecordingProvider::new(Arc::new(Canned), dir.path())
            .complete(self::req())
            .await
            .unwrap();
        let replay = ReplayProvider::new(dir.path());
        let second = DecisionProvider::decide(&replay, req()).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(second.single().unwrap().as_f64(), 0.75);
        assert!(matches!(
            DecisionProvider::decide(
                &replay,
                DecisionRequest::single(
                    serde_json::json!("other"),
                    crate::decision::Question::noul("ok?")
                )
            )
            .await,
            Err(ProviderError::NoFixture(_))
        ));
    }

    #[tokio::test]
    async fn replay_reports_missing_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let replay = ReplayProvider::new(dir.path());
        assert!(matches!(
            replay.complete(req()).await,
            Err(ProviderError::NoFixture(_))
        ));
    }
}
