//! # superx-inference — local GGUF inference engine
//!
//! Implements the **local-model pillar** (`ARCHITECTURE.md` §0c-1) using
//! Candle + a quantized-llama model loader. A model is loaded from a GGUF file
//! on disk along with its tokenizer; the engine then runs forward-pass
//! generation token-by-token with logits sampling.
//!
//! ## Entry points
//!
//! - [`InferenceEngine::new`] — construct from a model GGUF path and a
//!   tokenizer JSON path.
//! - [`InferenceEngine::predict`] — synchronous text generation. Logs the
//!   prompt *length* + a short preview at INFO; full prompt at DEBUG only
//!   (privacy + log-volume — operator opts in via `RUST_LOG=debug`).
//!
//! ## Design notes
//!
//! - **Today**: Single in-process Candle engine. The roadmap (#16 Rig.rs
//!   adoption) will wrap this behind a multi-provider `CompletionModel`
//!   trait so local Candle, remote `Anthropic`, remote `OpenAI` etc. are
//!   selectable per-task via `execution_params`.
//! - **Sampling params are hardcoded** in `predict` (seed = c, temp = 0.7,
//!   `top_p` = 0.9). Roadmap #1b (`execution_params` SCD-2 table) makes them
//!   per-run parameters.
//! - **EOS detection** uses a hardcoded token id (`2`). Different model
//!   families have different EOS — Roadmap follow-up will read it from the
//!   tokenizer config at engine construction time.
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use candle_core::{Device, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::quantized_llama::ModelWeights;
use tokenizers::Tokenizer;
use std::path::Path;
use thiserror::Error;

/// All error types surfaced by `InferenceEngine`. Distinguishes load-time
/// failures (model file or tokenizer missing / malformed) from run-time
/// failures (tokenisation, forward-pass, sampling). `Candle` and `Io` variants
/// preserve the underlying error so callers can introspect for retry policy.
#[derive(Error, Debug)]
pub enum InferenceError {
    /// Failure during the inference/generation phase.
    #[error("Inference failure: {0}")]
    Failure(String),
    /// Failure while loading model weights or tokenizer.
    #[error("Model load failure: {0}")]
    Load(String),
    /// Error from the underlying Candle tensor framework.
    #[error("Candle error: {0}")]
    Candle(#[from] candle_core::Error),
    /// Standard I/O failure.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// `InferenceEngine`: A safety-hardened, zero-dependency GGUF runner.
pub struct InferenceEngine {
    device: Device,
    tokenizer: Tokenizer,
    model: ModelWeights,
}

const MAX_PREDICT_TOKENS: usize = 4096;

impl InferenceEngine {
    /// Creates a new `InferenceEngine` from local GGUF and tokenizer files.
    ///
    /// # Errors
    /// Returns `InferenceError::Load` if either path does not exist, the
    /// tokenizer cannot be parsed, or the GGUF model cannot be read.
    pub fn new(model_path: &Path, tokenizer_path: &Path) -> Result<Self, InferenceError> {
        // Operator-facing API: a wrong --model-path / --tokenizer-path flag must
        // surface as a clean Err the CLI can print and exit on, not a panic.
        if !model_path.exists() {
            return Err(InferenceError::Load(format!(
                "model path does not exist: {}",
                model_path.display()
            )));
        }
        if !tokenizer_path.exists() {
            return Err(InferenceError::Load(format!(
                "tokenizer path does not exist: {}",
                tokenizer_path.display()
            )));
        }

        let device = Device::Cpu;
        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| InferenceError::Load(e.to_string()))?;
        
        let mut file = std::fs::File::open(model_path)?;
        let content = candle_core::quantized::gguf_file::Content::read(&mut file)?;
        let model = ModelWeights::from_gguf(content, &mut file, &device)?;

        Ok(Self { device, tokenizer, model })
    }

    /// `predict`: Generates text based on the provided prompt.
    ///
    /// # Panics
    /// Panics if the prompt is empty.
    ///
    /// # Errors
    /// Returns `InferenceError::Failure` if tokenization or generation fails.
    pub fn predict(&mut self, prompt: &str, max_tokens: usize) -> Result<String, InferenceError> {
        assert!(!prompt.is_empty(), "Prompt must not be empty");
        let safe_max = if max_tokens > MAX_PREDICT_TOKENS { MAX_PREDICT_TOKENS } else { max_tokens };

        // Log length + a short prefix at INFO. Full prompts can contain user-confidential
        // payloads and reach megabytes when the compiler blade fans-in a whole project DAG;
        // emit the full text only when the operator opts in via `RUST_LOG=debug`.
        let prompt_preview: String = prompt.chars().take(80).collect();
        tracing::info!("GGUF inference: {} chars (preview: {prompt_preview:?})", prompt.len());
        tracing::debug!("full inference prompt: {prompt}");
        
        let tokens = self.tokenizer.encode(prompt, true).map_err(|e| InferenceError::Failure(e.to_string()))?;
        let mut tokens_ids = tokens.get_ids().to_vec();
        let mut generated = String::new();
        let mut logits_processor = LogitsProcessor::new(299_792_458, Some(0.7), Some(0.9));

        for i in 0..safe_max {
            assert!(i < MAX_PREDICT_TOKENS, "Safety violation: MAX_PREDICT_TOKENS exceeded");
            // Standard KV-cache pattern: iteration 0 (prefill) feeds the entire
            // prompt; subsequent iterations feed only the single newly-sampled
            // token. The model's `start_pos` argument tells it where in the
            // cache to append, so we never re-process the prompt.
            let context_size = if i > 0 { 1 } else { tokens_ids.len() };
            let start_pos = tokens_ids.len().saturating_sub(context_size);
            let input = Tensor::new(&tokens_ids[start_pos..], &self.device)?.unsqueeze(0)?;
            let logits = self.model.forward(&input, start_pos)?;
            let logits = logits.squeeze(0)?;
            let next_token = logits_processor.sample(&logits)?;
            
            tokens_ids.push(next_token);
            let token_text = self.tokenizer.decode(&[next_token], true).map_err(|e| InferenceError::Failure(e.to_string()))?;
            generated.push_str(&token_text);

            if next_token == 2 { // Typical EOS
                break;
            }
        }

        Ok(generated)
    }
}
