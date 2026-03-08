use std::path::PathBuf;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::protocol::Request;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
	pub roots: Vec<RootConfig>,
	pub default_root: PathBuf,
	pub default_root_canon: PathBuf,
	pub chunk_max_tokens: usize,
	pub chunk_overlap_tokens: usize,
	pub watch_debounce_ms: u64,
	pub language_pack_root: PathBuf,
	pub languages: Option<Vec<String>>,
	pub language_download: bool,
	pub context: String,
	pub embedder: EmbedderConfig,
	pub lancedb_path: PathBuf,
	pub log_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootConfig {
	pub path: PathBuf,
	pub path_canon: PathBuf,
	pub display: String,
	pub default: bool,
	pub deny: Vec<String>,
	pub allow: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmbedderConfig {
	pub provider: String,
	pub concurrency: usize,
	pub fastembed: Option<FastembedConfig>,
	pub ollama: Option<OllamaConfig>,
	pub openai: Option<OpenAiConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FastembedConfig {
	#[serde(default)]
	pub model: Option<String>,
	pub model_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OllamaConfig {
	pub base_url: String,
	pub model: String,
	#[serde(default)]
	pub batch: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenAiConfig {
	pub api_key_env: String,
	pub model: String,
}

#[derive(Clone, Debug, Deserialize)]
struct RootInput {
	pub path: String,
	#[serde(default)]
	pub default: Option<bool>,
	#[serde(default)]
	pub deny: Vec<String>,
	#[serde(default)]
	pub allow: Vec<String>,
	#[serde(default)]
	pub blocked: Option<bool>,
}

pub fn default_config() -> Result<Config> {
	let cwd = std::env::current_dir()?;
	let default_root = root_from_cwd(&cwd);
	let default_root_path = default_root.path.clone();
	let default_root_canon = default_root.path_canon.clone();
	Ok(
		Config {
			roots: vec![default_root],
			default_root: default_root_path,
			default_root_canon: default_root_canon,
			chunk_max_tokens: 300,
			chunk_overlap_tokens: 50,
			watch_debounce_ms: 200,
			language_pack_root: PathBuf::from("./languages"),
			languages: None,
			language_download: true,
			context: "default".to_string(),
			embedder: EmbedderConfig {
				provider: "fastembed".to_string(),
				concurrency: 1,
				fastembed: Some(FastembedConfig {
					model: Some("bge-small-en-v1.5".to_string()),
					model_path: "./models/bge".to_string()
				}),
				ollama: None,
				openai: None
			},
			lancedb_path: PathBuf::from("./lancedb"),
			log_path: None
		}
	)
}

pub fn config_schema() -> Value {
	json!({
		"$schema": "http://json-schema.org/draft-07/schema#",
		"title": "mcp-search configuration",
		"type": "object",
		"additionalProperties": false,
		"properties": {
            "roots": {
                "type": "array",
                "minItems": 0,
                "description": "Allowed roots. The default root is used for relative paths.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "path": { "type": "string", "description": "Absolute or root-relative path." },
                        "default": { "type": "boolean", "description": "Exactly one root should be default." },
                        "deny": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Glob patterns to exclude from all operations."
                        },
                        "allow": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Glob patterns to include; anything not matching is denied."
                        },
                        "blocked": {
                            "type": "boolean",
                            "description": "Block this root when used in _meta.policy.",
                            "scope": "policy"
                        }
                    },
                    "required": ["path"]
                }
            },
            "chunk_max_tokens": {
                "type": "integer",
                "minimum": 1,
                "default": 300,
                "description": "Max tokens per chunk."
            },
            "chunk_overlap_tokens": {
                "type": "integer",
                "minimum": 0,
                "default": 50,
                "description": "Overlapping tokens between adjacent chunks."
            },
            "watch_debounce_ms": {
                "type": "integer",
                "minimum": 0,
                "default": 200,
                "description": "Debounce milliseconds for file watching."
            },
            "language_pack_root": {
                "type": "string",
                "default": "./languages",
                "description": "Root directory for language packs."
            },
            "context": {
                "type": "string",
                "default": "default",
                "description": "Logical instance context for data isolation."
            },
            "languages": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Language names to enable; omit to enable all known languages."
            },
            "language_download": {
                "type": "boolean",
                "default": true,
                "description": "Allow downloading missing Tree-sitter parser binaries."
            },
            "log_path": {
                "type": "string",
                "description": "Optional log directory or base .log path; defaults to CWD."
            },
            "lancedb": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": {
                        "type": "string",
                        "default": "./lancedb",
                        "description": "Directory for LanceDB data."
                    }
                }
            },
        "embedder": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "provider": {
                    "type": "string",
                    "enum": ["fastembed", "ollama", "openai"],
                    "description": "Embedding provider."
                },
                    "concurrency": {
                        "type": "integer",
                        "minimum": 1,
                        "default": 1,
                        "description": "Max concurrent embedding jobs."
                    },
                "fastembed": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "model": { "type": "string", "default": "bge-small-en-v1.5" },
                        "model_path": { "type": "string" }
                    },
                    "description": "Local fastembed model configuration."
                },
                    "ollama": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "base_url": { "type": "string" },
                            "model": { "type": "string" },
                            "batch": { "type": "boolean", "default": false }
                        },
                        "description": "Ollama embedding configuration."
                    },
                    "openai": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "api_key_env": { "type": "string" },
                            "model": { "type": "string" }
                        },
                        "description": "OpenAI embedding configuration."
                    }
                },
                "required": ["provider"]
            }
        },
		"required": []
	})
}

pub fn apply_initialize_config(config: &mut Config, req: &Request) -> Result<()> {
	let Some(value) = req.params
		.get("capabilities")
		.and_then(|caps| caps.get("experimental"))
		.and_then(|exp| exp.get("configuration")) else {
		return Ok(());
	};
	let cwd = std::env::current_dir()?;
	let updated = apply_config_override(config.clone(), value, &cwd)?;
	*config = updated;
	Ok(())
}

pub fn apply_config_override(base: Config, value: &Value, cwd: &PathBuf) -> Result<Config> {
	let obj = value.as_object().ok_or_else(|| anyhow!("config must be an object"))?;
	let mut next = base.clone();
	for (key, value) in obj {
		match key.as_str() {
			"roots" => {
				let inputs = parse_root_inputs(value)?;
				let roots = if inputs.is_empty() {
					vec![root_from_cwd(cwd)]
				}
				else {
					build_root_configs(&inputs, cwd)?
				};
				let (roots, default_root, default_root_canon) = finalize_roots(roots)?;
				next.roots = roots;
				next.default_root = default_root;
				next.default_root_canon = default_root_canon;
			}
			"chunk_max_tokens" => {
				next.chunk_max_tokens = parse_usize_value(value, "chunk_max_tokens")?;
			}
			"chunk_overlap_tokens" => {
				next.chunk_overlap_tokens = parse_usize_value(value, "chunk_overlap_tokens")?;
			}
			"watch_debounce_ms" => {
				next.watch_debounce_ms = parse_u64_value(value, "watch_debounce_ms")?;
			}
			"language_pack_root" => {
				let text = value.as_str().ok_or_else(|| anyhow!("language_pack_root must be a string"))?;
				next.language_pack_root = PathBuf::from(text);
			}
			"languages" => {
				let list = parse_string_list(value, "languages")?;
				next.languages = Some(list);
			}
			"language_download" => {
				if !value.is_null() {
					next.language_download = value.as_bool().ok_or_else(|| anyhow!("language_download must be a boolean"))?;
				}
			}
			"log_path" => {
				if !value.is_null() {
					let text = value.as_str().ok_or_else(|| anyhow!("log_path must be a string"))?;
					if text.trim().is_empty() {
						next.log_path = None;
					}
					else {
						next.log_path = Some(PathBuf::from(text));
					}
				}
			}
			"context" => {
				let text = value.as_str().ok_or_else(|| anyhow!("context must be a string"))?;
				if text.trim().is_empty() {
					return Err(anyhow!("context must not be empty"));
				}
				next.context = text.to_string();
			}
			"lancedb" => {
				let obj = value.as_object().ok_or_else(|| anyhow!("lancedb must be an object"))?;
				if let Some(path_value) = obj.get("path") {
					let text = path_value.as_str().ok_or_else(|| anyhow!("lancedb.path must be a string"))?;
					next.lancedb_path = PathBuf::from(text);
				}
			}
			"embedder" => {
				next.embedder = parse_embedder_config(value)?;
			}
			_ => return Err(anyhow!("unknown config key: {}", key)),
		}
	}
	Ok(next)
}

fn parse_root_inputs(value: &Value) -> Result<Vec<RootInput>> {
	let inputs: Vec<RootInput> = serde_json::from_value(value.clone()).map_err(|err| anyhow!("invalid roots: {}", err))?;
	Ok(inputs)
}

fn parse_string_list(value: &Value, name: &str) -> Result<Vec<String>> {
	match value {
		Value::Array(items) => {
			let mut result = Vec::new();
			for item in items {
				let text = item.as_str().ok_or_else(|| anyhow!("{} must be strings", name))?;
				if !text.trim().is_empty() {
					result.push(text.to_string());
				}
			}
			Ok(result)
		}
		Value::String(text) => {
			if text.trim().is_empty() {
				Ok(Vec::new())
			}
			else {
				Ok(vec![text.to_string()])
			}
		}
		_ => Err(anyhow!("{} must be a string or array of strings", name)),
	}
}

fn build_root_configs(inputs: &[RootInput], cwd: &PathBuf) -> Result<Vec<RootConfig>> {
	let mut roots = Vec::new();
	for input in inputs {
		if input.blocked.is_some() {
			return Err(anyhow!("root.blocked is only allowed in policy"));
		}
		let normalized = normalize_root_path(&input.path, cwd);
		let display = if std::path::Path::new(&input.path).is_absolute() {
			input.path.clone()
		}
		else {
			normalize_relative(&input.path)
		};
		roots.push(
			RootConfig {
				path: normalized.clone(),
				path_canon: canonicalize_or_self(&normalized),
				display,
				default: input.default.unwrap_or(false),
				deny: input.deny.clone(),
				allow: input.allow.clone()
			}
		);
	}
	Ok(roots)
}

fn root_from_cwd(cwd: &PathBuf) -> RootConfig {
	let normalized = normalize_path(cwd);
	RootConfig {
		path: normalized.clone(),
		path_canon: canonicalize_or_self(&normalized),
		display: normalized.display().to_string(),
		default: true,
		deny: Vec::new(),
		allow: Vec::new()
	}
}

fn finalize_roots(mut roots: Vec<RootConfig>) -> Result<(Vec<RootConfig>, PathBuf, PathBuf)> {
	if roots.is_empty() {
		return Err(anyhow!("roots are required"));
	}
	let default_index = roots.iter().position(|root| root.default);
	let default_index = match default_index {
		Some(index) => index,
		None => 0,
	};
	for (index, root) in roots.iter_mut().enumerate() {
		root.default = index == default_index;
	}
	let default_root = roots[default_index].path.clone();
	let default_root_canon = roots[default_index].path_canon.clone();
	Ok((roots, default_root, default_root_canon))
}

fn parse_embedder_config(value: &Value) -> Result<EmbedderConfig> {
	let obj = value.as_object().ok_or_else(|| anyhow!("embedder must be an object"))?;
	let provider = obj.get("provider")
		.and_then(Value::as_str)
		.ok_or_else(|| anyhow!("embedder.provider is required"))?.to_string();
	let concurrency = obj.get("concurrency")
		.map(|value| parse_usize_value(value, "embedder.concurrency"))
		.transpose()?.unwrap_or(1);
	let fastembed = obj.get("fastembed")
		.map(
			|value| {
				serde_json::from_value::<FastembedConfig>(value.clone())
					.map_err(|err| anyhow!("invalid embedder.fastembed: {}", err))
			})
		.transpose()?;
	let ollama = obj.get("ollama")
		.map(
			|value| {
				serde_json::from_value::<OllamaConfig>(value.clone())
					.map_err(|err| anyhow!("invalid embedder.ollama: {}", err))
			})
		.transpose()?;
	let openai = obj.get("openai")
		.map(
			|value| {
				serde_json::from_value::<OpenAiConfig>(value.clone())
					.map_err(|err| anyhow!("invalid embedder.openai: {}", err))
			})
		.transpose()?;
	match provider.as_str() {
		"fastembed" => {
			let Some(cfg) = fastembed.as_ref() else {
				return Err(anyhow!(
				"embedder.fastembed is required when provider is fastembed"
				));
			};
			if cfg.model_path
				.trim()
				.is_empty() {
				return Err(anyhow!("embedder.fastembed.model_path is required"));
			}
		}
		"ollama" => {
			let Some(cfg) = ollama.as_ref() else {
				return Err(anyhow!(
				"embedder.ollama is required when provider is ollama"
				));
			};
			if cfg.base_url
				.trim()
				.is_empty()
				|| cfg.model
					.trim()
					.is_empty() {
				return Err(anyhow!(
				"embedder.ollama.base_url and embedder.ollama.model are required"
				));
			}
		}
		"openai" => {
			let Some(cfg) = openai.as_ref() else {
				return Err(anyhow!(
				"embedder.openai is required when provider is openai"
				));
			};
			if cfg.api_key_env
				.trim()
				.is_empty()
				|| cfg.model
					.trim()
					.is_empty() {
				return Err(anyhow!(
				"embedder.openai.api_key_env and embedder.openai.model are required"
				));
			}
		}
		_ => return Err(anyhow!("unknown embedder provider: {}", provider)),
	}
	Ok(EmbedderConfig {
		provider,
		concurrency,
		fastembed,
		ollama,
		openai
	})
}

fn parse_usize_value(value: &Value, name: &str) -> Result<usize> {
	let num = value.as_u64().ok_or_else(|| anyhow!("{} must be an integer", name))?;
	usize::try_from(num).map_err(|_| anyhow!("{} out of range", name))
}

fn parse_u64_value(value: &Value, name: &str) -> Result<u64> {
	value.as_u64().ok_or_else(|| anyhow!("{} must be an integer", name))
}

fn normalize_root_path(path: &str, cwd: &PathBuf) -> PathBuf {
	let mut root_path = PathBuf::from(path);
	if !root_path.is_absolute() {
		root_path = cwd.join(root_path);
	}
	normalize_path(&root_path)
}

fn normalize_path(path: &PathBuf) -> PathBuf {
	let mut normalized = PathBuf::new();
	for component in path.components() {
		use std::path::Component;
		match component {
			Component::CurDir => {}
			Component::ParentDir => {
				normalized.pop();
			}
			_ => normalized.push(component.as_os_str()),
		}
	}
	normalized
}

fn normalize_relative(path: &str) -> String {
	let mut normalized = String::new();
	for part in path.split('/') {
		if part.is_empty() || part == "." {
			continue;
		}
		if part == ".." {
			if let Some(pos) = normalized.rfind('/') {
				normalized.truncate(pos);
			}
			else {
				normalized.clear();
			}
			continue;
		}
		if !normalized.is_empty() {
			normalized.push('/');
		}
		normalized.push_str(part);
	}
	if normalized.is_empty() {
		".".to_string()
	}
	else {
		normalized
	}
}

fn canonicalize_or_self(path: &PathBuf) -> PathBuf {
	if path.exists() {
		path.canonicalize().unwrap_or_else(|_| path.clone())
	}
	else {
		path.clone()
	}
}
