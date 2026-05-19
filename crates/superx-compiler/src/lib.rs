//! # superx-compiler — context distillation blade
//!
//! Implements the **RAG / daggify pillar** (`ARCHITECTURE.md` §0a) by combining
//! structural traversal (`Kernel::compile_context`) with optional LLM-driven
//! distillation. Returns a context XML window that downstream agents read.
//!
//! ## Entry point
//!
//! [`CompilerBlade::compile`] — durable, tier-aware context distillation with
//! optional model post-processing. Records compilation checkpoints to the
//! `execution_cursor` for resumability.
//!
//! ## Design notes
//!
//! - **Model is optional.** Without an `InferenceEngine`, the blade returns
//!   the raw substrate-traversal XML. With one, it adds a distilled summary
//!   layer on top. Graceful degradation per the §0c-1 "always intelligent"
//!   principle: `SuperX` still works without a model loaded.
//! - **Prompts are hardcoded today** (roadmap #6 will move them to
//!   `node_artifact` entities so they're operator-customisable).
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use superx_kernel::{Kernel, KernelError};
use superx_inference::InferenceEngine;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

/// `CompilerBlade`: The durable context distillation engine.
pub struct CompilerBlade<'a> {
    pub kernel: &'a Kernel,
    pub inference: Option<Arc<Mutex<InferenceEngine>>>,
}

/// Metadata for tracking compilation progress.
#[derive(Serialize, Deserialize, Debug)]
pub struct CompilationMetadata {
    pub total_nodes: usize,
    pub current_node_index: usize,
}

impl<'a> CompilerBlade<'a> {
    /// Creates a new `CompilerBlade`.
    #[must_use]
    pub fn new(kernel: &'a Kernel, inference: Option<Arc<Mutex<InferenceEngine>>>) -> Self {
        Self { kernel, inference }
    }

    /// `compile`: Durable context distillation with optional LLM processing and memory tiering.
    ///
    /// # Panics
    /// Panics if `root_id` or `run_id` are empty.
    ///
    /// # Errors
    /// Returns `KernelError` if substrate interaction or inference fails.
    pub async fn compile(
        &self, 
        root_id: &str, 
        run_id: &str,
        tiers: Option<Vec<String>>
    ) -> Result<String, KernelError> {
        assert!(!root_id.is_empty(), "Root ID mandatory");
        assert!(!run_id.is_empty(), "Run ID mandatory");

        tracing::info!("Starting durable compilation for {root_id} (Run ID: {run_id})");

        // 1. Initial Checkpoint
        self.kernel.checkpoint_execution(
            run_id,
            "compilation",
            None,
            Some(serde_json::json!(CompilationMetadata { total_nodes: 0, current_node_index: 0 }))
        ).await?;

        // 2. Structural Traversal (Tier-aware)
        let raw_xml = self.kernel.compile_context(root_id, run_id, tiers).await?;

        // 3. Optional Distillation (LLM Processing)
        let final_output = if let Some(ref engine_mux) = self.inference {
            tracing::info!("Distilling context via local inference...");
            let mut engine = engine_mux.lock().await;
            let prompt = format!("Distill the following context into a concise summary:\n\n{raw_xml}");
            match engine.predict(&prompt, 512) {
                Ok(distilled) => format!("<distilled>\n{distilled}\n</distilled>"),
                Err(e) => {
                    tracing::error!("Distillation failed: {e}");
                    raw_xml
                }
            }
        } else {
            raw_xml
        };

        // 4. Final Checkpoint
        self.kernel.checkpoint_execution(
            run_id,
            "compilation",
            Some("DONE".to_string()),
            None
        ).await?;

        Ok(final_output)
    }
}
