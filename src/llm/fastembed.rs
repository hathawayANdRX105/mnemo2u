//! Local ONNX embeddings — fastembed, the production [`Embedder`].
//!
//! The trait's `dim` comes from the model info rather than a hardcoded
//! constant, so a model swap (bge-small → bge-m3) stays a config change.
//! fastembed's `embed` is synchronous and takes `&mut self`, so a `Mutex`
//! serializes access: the callers already bound their own concurrency
//! (`best_model_max_async`), and the vector rows are batched per upsert.

use std::sync::Mutex;

use async_trait::async_trait;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

use crate::core::traits::{Embedder, Result, StoreError};

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

/// The default model (fastembed's own default: bge-small-en-v1.5, 384 dims).
pub const DEFAULT_MODEL: &str = "BAAI/bge-small-en-v1.5";

/// Lowercase alphanumerics only: `BAAI/bge-small-en-v1.5` and
/// `BGESmallENV15` both normalize to `bgesmallenv15`.
fn normalize(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// Resolve the config's model spelling to a fastembed model. An unknown name
/// fails at startup with the supported list, so a typo never silently falls
/// back to a different embedding space.
fn resolve_model(model: &str) -> Result<EmbeddingModel> {
    if let Ok(parsed) = model.parse::<EmbeddingModel>() {
        return Ok(parsed);
    }
    let wanted = normalize(model);
    // `BAAI/bge-small-en-v1.5` → also try its bare `bge-small-en-v1.5`
    // segment, so the org prefix does not defeat the match.
    let bare = model
        .rsplit('/')
        .next()
        .map(normalize)
        .unwrap_or_else(|| wanted.clone());
    let matched = TextEmbedding::list_supported_models()
        .into_iter()
        .find(|info| {
            let debug = normalize(&format!("{:?}", info.model));
            debug == wanted || debug == bare
        });
    match matched {
        Some(info) => Ok(info.model),
        None => {
            let supported: Vec<String> = TextEmbedding::list_supported_models()
                .into_iter()
                .map(|info| format!("{:?}", info.model))
                .collect();
            Err(backend(format!(
                "unknown embedding model: {model} (supported: {})",
                supported.join(", ")
            )))
        }
    }
}

/// A local embedding model loaded once and shared across the process.
pub struct FastembedEmbedder {
    model: Mutex<TextEmbedding>,
    model_id: String,
    dim: usize,
    max_token_size: usize,
}

impl FastembedEmbedder {
    /// Load the model named by `model`, downloading it on first use into
    /// fastembed's cache directory.
    ///
    /// Two spellings are accepted: fastembed's own variant name
    /// (`BGESmallENV15`) and the HuggingFace repo id it wraps
    /// (`BAAI/bge-small-en-v1.5`). The second form is what operators write,
    /// so it is matched by normalizing both sides to alphanumerics.
    pub fn new(model: &str) -> Result<Self> {
        let embedding_model = resolve_model(model)?;
        let dim = TextEmbedding::get_model_info(&embedding_model)
            .map_err(backend)?
            .dim;
        let text =
            TextEmbedding::try_new(TextInitOptions::new(embedding_model)).map_err(backend)?;
        Ok(Self {
            model: Mutex::new(text),
            model_id: model.to_string(),
            dim,
            // fastembed's tokenizers truncate over-long inputs; the model info
            // struct carries no token cap, so the cap comes from the model's
            // own max_length (bge family: 512, bge-m3: 8192).
            max_token_size: max_length_for(model),
        })
    }
}

/// fastembed's per-model input cap (`HasMaxLength::MAX_LENGTH` values).
fn max_length_for(model: &str) -> usize {
    match model {
        "BAAI/bge-m3" => 8192,
        "BAAI/bge-large-zh-v1.5" | "BAAI/bge-large-en-v1.5" => 8192,
        _ => 512,
    }
}

#[async_trait]
impl Embedder for FastembedEmbedder {
    fn model_id(&self) -> String {
        self.model_id.clone()
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn max_token_size(&self) -> usize {
        self.max_token_size
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        // An empty slice is a no-op: callers probing with no input must not
        // pay an inference call.
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut model = self
            .model
            .lock()
            .map_err(|_| StoreError::Backend("embedder lock poisoned".into()))?;
        model
            .embed(texts, None)
            .map_err(|error| StoreError::Backend(error.to_string()))
    }
}
