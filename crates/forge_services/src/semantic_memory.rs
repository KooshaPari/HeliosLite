//! Runtime `SemanticMemoryPort` service.
//!
//! Wraps a boxed [`SemanticMemoryPort`] adapter selected from the process
//! environment so the runtime can expose one stable service type regardless
//! of whether the portable JSONL adapter or a remote provider (Supermemory /
//! Letta / Cognee) backs the call.

use forge_domain::{
    SemanticMemoryBudget, SemanticMemoryError, SemanticMemoryIdentity, SemanticMemoryPort,
    SemanticMemoryQuery, SemanticMemoryRecord, SemanticMemoryWrite,
};

/// A concrete, clonable `SemanticMemoryPort` that delegates to a boxed
/// adapter. Constructed once at service startup from
/// [`forge_semantic::config::Config::build`].
#[derive(Clone)]
pub struct ForgeSemanticMemory {
    inner: std::sync::Arc<Box<dyn SemanticMemoryPort>>,
}

impl ForgeSemanticMemory {
    /// Wraps the given port adapter.
    pub fn new(port: Box<dyn SemanticMemoryPort>) -> Self {
        Self { inner: std::sync::Arc::new(port) }
    }

    /// Builds the adapter selected from the environment, wrapping it.
    ///
    /// # Panics
    /// Panics if the configured adapter cannot be validated/constructed.
    /// Callers are responsible for validating `FORGE_SEMANTIC_ADAPTER`
    /// config before constructing the runtime; a misconfigured remote
    /// adapter here is a hard startup error.
    pub fn from_env() -> Self {
        Self::new(
            forge_semantic::config::Config::from_env()
                .and_then(|cfg| cfg.build())
                .unwrap_or_else(|err| {
                    panic!("failed to build semantic-memory adapter from env: {err}")
                }),
        )
    }
}

#[async_trait::async_trait]
impl SemanticMemoryPort for ForgeSemanticMemory {
    async fn store(
        &self,
        write: SemanticMemoryWrite,
    ) -> Result<SemanticMemoryIdentity, SemanticMemoryError> {
        self.inner.store(write).await
    }

    async fn recall(
        &self,
        query: SemanticMemoryQuery,
        budget: SemanticMemoryBudget,
    ) -> Result<Vec<SemanticMemoryRecord>, SemanticMemoryError> {
        self.inner.recall(query, budget).await
    }

    async fn forget(&self, identity: SemanticMemoryIdentity) -> Result<(), SemanticMemoryError> {
        self.inner.forget(identity).await
    }

    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }
}
