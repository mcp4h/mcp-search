use std::env;
use std::sync::Mutex;
use anyhow::{anyhow, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use crate::config::{EmbedderConfig, FastembedConfig, OllamaConfig, OpenAiConfig};

pub trait Embedder: Send + Sync {
	fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

pub fn build_embedder(config: &EmbedderConfig) -> Result<Box<dyn Embedder>> {
	match config.provider.as_str() {
		"fastembed" => Ok(
			Box::new(
				BgeEmbedder::new(
					config.fastembed
						.clone()
						.ok_or_else(|| anyhow!("embedder.fastembed is required"))?,
					config.concurrency
				)?
			)
		),
		"ollama" => Ok(
			Box::new(
				OllamaEmbedder::new(config.ollama
					.clone()
					.ok_or_else(|| anyhow!("embedder.ollama is required"))?, config.concurrency)?
			)
		),
		"openai" => Ok(
			Box::new(
				OpenAiEmbedder::new(config.openai
					.clone()
					.ok_or_else(|| anyhow!("embedder.openai is required"))?, config.concurrency)?
			)
		),
		_ => Err(anyhow!("unknown embedder provider: {}", config.provider)),
	}
}

pub struct BgeEmbedder {
	model: Mutex<TextEmbedding>,
	_concurrency: usize,
}

impl BgeEmbedder {
	pub fn new(config: FastembedConfig, concurrency: usize) -> Result<Self> {
		if config.model_path
			.trim()
			.is_empty() {
			return Err(anyhow!("embedder.fastembed.model_path is required"));
		}
		let model_name = resolve_model(&config)?;
		let options = InitOptions::new(model_name).with_cache_dir(std::path::PathBuf::from(&config.model_path));
		let model = TextEmbedding::try_new(options).map_err(|err| anyhow!("failed to initialize BGE embedder: {}", err))?;
		Ok(Self {
			model: Mutex::new(model),
			_concurrency: concurrency
		})
	}
}

impl Embedder for BgeEmbedder {
	fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
		let model = self.model
			.lock()
			.map_err(|_| anyhow!("BGE embedder lock poisoned"))?;
		model.embed(texts.to_vec(), None).map_err(|err| anyhow!("BGE embedding failed: {}", err))
	}
}

pub struct OllamaEmbedder {
	_config: OllamaConfig,
	client: Client,
	_concurrency: usize,
}

impl OllamaEmbedder {
	pub fn new(config: OllamaConfig, concurrency: usize) -> Result<Self> {
		if config.base_url
			.trim()
			.is_empty()
			|| config.model
				.trim()
				.is_empty() {
			return Err(anyhow!(
			"embedder.ollama.base_url and embedder.ollama.model are required",
			));
		}
		Ok(Self {
			_config: config,
			client: Client::new(),
			_concurrency: concurrency
		})
	}
}

impl Embedder for OllamaEmbedder {
	fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
		if self._config.batch {
			let request = OllamaBatchRequest {
				model: self._config
					.model
					.clone(),
				input: texts.to_vec()
			};
			let url = format!("{}/api/embed", self._config.base_url.trim_end_matches('/'),);
			let response: OllamaBatchResponse = self.client
				.post(url)
				.json(&request)
				.send()
				.map_err(|err| anyhow!("ollama batch request failed: {}", err))?.error_for_status()
				.map_err(|err| anyhow!("ollama batch error: {}", err))?.json()
				.map_err(|err| anyhow!("ollama batch response parse failed: {}", err))?;
			return Ok(response.embeddings);
		}
		let mut embeddings = Vec::with_capacity(texts.len());
		for text in texts {
			let request = OllamaRequest {
				model: self._config
					.model
					.clone(),
				prompt: text.clone()
			};
			let url = format!(
				"{}/api/embeddings",
				self._config.base_url.trim_end_matches('/'),
			);
			let response: OllamaResponse = self.client
				.post(url)
				.json(&request)
				.send()
				.map_err(|err| anyhow!("ollama request failed: {}", err))?.error_for_status()
				.map_err(|err| anyhow!("ollama error: {}", err))?.json()
				.map_err(|err| anyhow!("ollama response parse failed: {}", err))?;
			embeddings.push(response.embedding);
		}
		Ok(embeddings)
	}
}

pub struct OpenAiEmbedder {
	_config: OpenAiConfig,
	client: Client,
	_concurrency: usize,
}

impl OpenAiEmbedder {
	pub fn new(config: OpenAiConfig, concurrency: usize) -> Result<Self> {
		if config.api_key_env
			.trim()
			.is_empty()
			|| config.model
				.trim()
				.is_empty() {
			return Err(anyhow!(
			"embedder.openai.api_key_env and embedder.openai.model are required",
			));
		}
		Ok(Self {
			_config: config,
			client: Client::new(),
			_concurrency: concurrency
		})
	}
}

impl Embedder for OpenAiEmbedder {
	fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
		let api_key = env::var(&self._config.api_key_env)
			.map_err(|_| anyhow!("missing OPENAI api key env: {}", self._config.api_key_env))?;
		let request = OpenAiRequest {
			model: self._config
				.model
				.clone(),
			input: texts.to_vec()
		};
		let response: OpenAiResponse = self.client
			.post("https://api.openai.com/v1/embeddings")
			.bearer_auth(api_key)
			.json(&request)
			.send()
			.map_err(|err| anyhow!("openai request failed: {}", err))?.error_for_status()
			.map_err(|err| anyhow!("openai error: {}", err))?.json()
			.map_err(|err| anyhow!("openai response parse failed: {}", err))?;
		let mut data = response.data;
		data.sort_by_key(|item| item.index);
		Ok(
			data.into_iter()
				.map(|item| item.embedding)
				.collect()
		)
	}
}

fn resolve_model(config: &FastembedConfig) -> Result<EmbeddingModel> {
	let Some(name) = config.model.as_ref() else {
		return Ok(EmbeddingModel::BGESmallENV15);
	};
	match name.as_str() {
		"bge-small-en-v1.5" => Ok(EmbeddingModel::BGESmallENV15),
		"bge-base-en-v1.5" => Ok(EmbeddingModel::BGEBaseENV15),
		"bge-large-en-v1.5" => Ok(EmbeddingModel::BGELargeENV15),
		"e5-small-v2" => Ok(EmbeddingModel::MultilingualE5Small),
		"e5-base-v2" => Ok(EmbeddingModel::MultilingualE5Base),
		"e5-large-v2" => Ok(EmbeddingModel::MultilingualE5Large),
		other => Err(anyhow!("unsupported fastembed model: {}", other)),
	}
}

#[derive(Debug, Serialize)]
struct OllamaRequest {
	model: String,
	prompt: String,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
	embedding: Vec<f32>,
}

#[derive(Debug, Serialize)]
struct OllamaBatchRequest {
	model: String,
	input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaBatchResponse {
	embeddings: Vec<Vec<f32>>,
}

#[derive(Debug, Serialize)]
struct OpenAiRequest {
	model: String,
	input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
	data: Vec<OpenAiEmbedding>,
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbedding {
	index: usize,
	embedding: Vec<f32>,
}
