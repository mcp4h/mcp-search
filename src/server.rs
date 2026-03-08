use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use crate::config::{self, Config};
use crate::embedder::{build_embedder, Embedder};
use crate::langpacks::{load_language_packs, sync_language_packs};
use crate::logger;
use crate::parser::build_registry;
use crate::protocol::{Request, Response, tool_error};
use crate::search::{self, SearchContext};
use crate::store::{LanceDbStore, VectorStore};
use crate::indexer::{self, IndexConfig};
use crate::watcher;

pub async fn run() -> Result<()> {
	let mut state = AppState::new()?;
	let cwd = std::env::current_dir()?;
	logger::configure(state.config
		.log_path
		.as_ref(), &cwd)?;
	let stdin = tokio::io::stdin();
	let stdout = tokio::io::stdout();
	let mut reader = BufReader::new(stdin);
	let mut writer = BufWriter::new(stdout);
	let mut line = String::new();
	loop {
		line.clear();
		let bytes = reader.read_line(&mut line).await?;
		if bytes == 0 {
			break;
		}
		let req: Request = match serde_json::from_str(&line) {
			Ok(req) => req,
			Err(err) => {
				let resp = Response::err(Value::Null, -32700, err.to_string());
				write_response(&mut writer, resp).await?;
				continue;
			}
		};
		if req.method == "initialize" {
			if let Err(err) = config::apply_initialize_config(&mut state.config, &req) {
				let resp = Response::err(req.id.clone(), -32602, err.to_string());
				write_response(&mut writer, resp).await?;
				continue;
			}
			if let Err(err) = logger::configure(state.config
				.log_path
				.as_ref(), &cwd) {
				let resp = Response::err(req.id.clone(), -32603, err.to_string());
				write_response(&mut writer, resp).await?;
				continue;
			}
			state.invalidate_resources();
			if let Err(err) = state.start_indexing().await {
				logger::error(format!("failed to start indexing: {}", err));
			}
			if let Err(err) = state.start_watcher().await {
				logger::error(format!("failed to start watcher: {}", err));
			}
		}
		let resp = handle_request(&mut state, req).await;
		write_response(&mut writer, resp).await?;
	}
	Ok(())
}

async fn write_response(writer: &mut BufWriter<tokio::io::Stdout>, resp: Response) -> Result<()> {
	let payload = serde_json::to_string(&resp)?;
	writer.write_all(payload.as_bytes()).await?;
	writer.write_all(b"\n").await?;
	writer.flush().await?;
	Ok(())
}

async fn handle_request(state: &mut AppState, req: Request) -> Response {
	match route(state, &req).await {
		Ok(value) => Response::ok(req.id, value),
		Err(err) => Response::err(req.id, -32000, err.to_string()),
	}
}

async fn route(state: &mut AppState, req: &Request) -> Result<Value> {
	match req.method.as_str() {
		"initialize" => Ok(
			json!({
				"serverInfo": {
				"name": "mcp-search",
				"version": "0.1.0"
			},
				"configSchema": config::config_schema(),
				"capabilities": {
				"resources": { "list": true, "read": true },
				"tools": { "list": true, "call": true },
				"experimental": { "policy": true },
				"_meta": { "server": "mcp-search", "vendor": "celerex" }
			}
			})
		),
		"tools/list" => Ok(json!({
			"tools": tool_definitions()
		})),
		"tools/call" => {
			let name = req.params
				.get("name")
				.and_then(Value::as_str)
				.ok_or_else(|| anyhow!("name is required"))?;
			let arguments = req.params
				.get("arguments")
				.cloned()
				.unwrap_or_else(|| json!({}));
			let meta = req.params
				.get("_meta")
				.cloned()
				.unwrap_or_else(|| json!({}));
			execute_tool(
				state,
				name,
				&arguments,
				&meta
			).await
		}
		"resources/list" => Ok(resources_list()),
		"resources/read" => resources_read(req),
		_ => Err(anyhow!("method not found")),
	}
}

async fn execute_tool(
	state: &mut AppState,
	name: &str,
	arguments: &Value,
	meta: &Value) -> Result<Value> {
	match name {
		"semantic_search" => {
			let config = state.config.clone();
			let resources = state.ensure_search_resources().await?;
			let structured = search::search(
				&config,
				SearchContext { embedder: resources.embedder.as_ref(), store: resources.store.as_ref() },
				arguments,
				meta
			).await?;
			Ok(tool_success(name, &structured))
		}
		_ => Ok(tool_error(name, "unknown tool")),
	}
}

fn tool_definitions() -> Vec<Value> {
	vec![json!({
		"name": "semantic_search",
		"description": "Finds content by meaning and context.",
		"intentTemplate": "Search for {query}",
		"annotations": {
			"scopes": ["read:search"],
			"group": "search"
		},
		"inputSchema": {
			"type": "object",
			"additionalProperties": false,
			"properties": {
				"query": { "type": "string", "description": "A natural language description of what you are looking for." },
				"limit": { "type": "integer", "minimum": 1, "default": 5 }
			},
			"required": ["query"]
		}
	})]
}

fn tool_success(name: &str, structured: &Value) -> Value {
	let summary = match structured.get("count").and_then(Value::as_u64) {
		Some(count) => format!("Found {} result(s)", count),
		None => format!("{} completed", name),
	};
	json!({
		"content": [
			{
				"type": "text",
				"text": summary
			}
		],
		"structuredContent": structured
	})
}

fn resources_list() -> Value {
	json!({
		"resources": [
			{
				"uri": "ui://search/index.html",
				"name": "Search",
				"mimeType": "text/html",
				"annotations": { "type": "application" }
			},
			{
				"uri": "ui://search/app.css",
				"name": "search styles",
				"mimeType": "text/css"
			},
			{
				"uri": "ui://search/app.js",
				"name": "search script",
				"mimeType": "text/javascript"
			}
		]
	})
}

fn resources_read(req: &Request) -> Result<Value> {
	let uri = req.params
		.get("uri")
		.and_then(Value::as_str)
		.ok_or_else(|| anyhow!("uri is required"))?;
	match uri {
		"ui://search/index.html" => Ok(json!({
			"contents": [{
				"uri": uri,
				"mimeType": "text/html",
				"text": search_index_html()
			}]
		})),
		"ui://search/app.css" => Ok(json!({
			"contents": [{
				"uri": uri,
				"mimeType": "text/css",
				"text": search_app_css()
			}]
		})),
		"ui://search/app.js" => Ok(json!({
			"contents": [{
				"uri": uri,
				"mimeType": "text/javascript",
				"text": search_app_js()
			}]
		})),
		_ => Err(anyhow!("resource not found")),
	}
}

fn search_index_html() -> &'static str {
	include_str!("../assets/ui/search/index.html")
}

fn search_app_css() -> &'static str {
	include_str!("../assets/ui/search/app.css")
}

fn search_app_js() -> &'static str {
	include_str!("../assets/ui/search/app.js")
}

struct AppState {
	config: Config,
	resources: Option<Resources>,
	index_task: Option<tokio::task::JoinHandle<()>>,
	watcher_task: Option<tokio::task::JoinHandle<()>>,
}

struct Resources {
	pub embedder: Arc<dyn Embedder>,
	pub store: Arc<dyn VectorStore>,
}

impl AppState {
	fn new() -> Result<Self> {
		Ok(Self {
			config: config::default_config()?,
			resources: None,
			index_task: None,
			watcher_task: None
		})
	}
	fn invalidate_resources(&mut self) {
		self.resources = None;
	}
	async fn start_indexing(&mut self) -> Result<()> {
		if let Some(handle) = self.index_task.take() {
			handle.abort();
		}
		let roots = self.config
			.roots
			.iter()
			.map(|root| root.path.clone())
			.collect::<Vec<_>>();
		let max_tokens = self.config.chunk_max_tokens;
		let overlap_tokens = self.config.chunk_overlap_tokens;
		let concurrency = self.config
			.embedder
			.concurrency;
		let language_pack_root = self.config
			.language_pack_root
			.clone();
		let languages = self.config
			.languages
			.clone();
		let language_download = self.config.language_download;
		let embedder_config = self.config
			.embedder
			.clone();
		let lancedb_path = self.config
			.lancedb_path
			.join(&self.config.context);
		let config = IndexConfig {
			roots,
			max_tokens,
			overlap_tokens,
			concurrency
		};
		self.index_task = Some(
			tokio::spawn(
				async move {
					if let Err(err) = run_indexer(
						config,
						language_pack_root,
						languages.as_deref(),
						language_download,
						embedder_config,
						lancedb_path
					).await {
						logger::error(format!("indexing failed: {}", err));
					}
				}
			)
		);
		Ok(())
	}
	async fn start_watcher(&mut self) -> Result<()> {
		if let Some(handle) = self.watcher_task.take() {
			handle.abort();
		}
		let roots = self.config
			.roots
			.iter()
			.map(|root| root.path.clone())
			.collect::<Vec<_>>();
		let debounce = self.config.watch_debounce_ms;
		let max_tokens = self.config.chunk_max_tokens;
		let overlap_tokens = self.config.chunk_overlap_tokens;
		let language_pack_root = self.config
			.language_pack_root
			.clone();
		let languages = self.config
			.languages
			.clone();
		let language_download = self.config.language_download;
		let embedder_config = self.config
			.embedder
			.clone();
		let lancedb_path = self.config
			.lancedb_path
			.join(&self.config.context);
		self.watcher_task = Some(
			tokio::spawn(
				async move {
					if let Err(err) = run_watcher(
						roots,
						debounce,
						max_tokens,
						overlap_tokens,
						language_pack_root,
						languages.as_deref(),
						language_download,
						embedder_config,
						lancedb_path
					).await {
						logger::error(format!("watcher failed: {}", err));
					}
				}
			)
		);
		Ok(())
	}
	async fn ensure_search_resources(&mut self) -> Result<&Resources> {
		if self.resources.is_none() {
			let embedder = build_embedder(&self.config.embedder)?;
			let lancedb_path = self.config
				.lancedb_path
				.join(&self.config.context);
			logger::info(format!("using lancedb path: {}", lancedb_path.display()));
			let store = LanceDbStore::new(lancedb_path).await?;
			self.resources = Some(Resources {
				embedder: Arc::from(embedder),
				store: Arc::from(store)
			});
		}
		self.resources
			.as_ref()
			.ok_or_else(|| anyhow!("failed to initialize resources"))
	}
}

async fn run_indexer(
	config: IndexConfig,
	language_pack_root: PathBuf,
	enabled_languages: Option<&[String]>,
	allow_download: bool,
	embedder_config: crate::config::EmbedderConfig,
	lancedb_path: PathBuf) -> Result<()> {
	sync_language_packs(&language_pack_root, enabled_languages, allow_download)?;
	let language_packs = load_language_packs(&language_pack_root, enabled_languages)?;
	logger::info(format!("loaded {} language packs", language_packs.len()));
	let languages = Arc::new(build_registry(&language_packs)?);
	let embedder = Arc::from(build_embedder(&embedder_config)?);
	logger::info(format!("using lancedb path: {}", lancedb_path.display()));
	let store = Arc::from(LanceDbStore::new(lancedb_path).await?);
	indexer::index_roots(
		config,
		languages,
		embedder,
		store
	).await
}

async fn run_watcher(
	roots: Vec<PathBuf>,
	debounce_ms: u64,
	max_tokens: usize,
	overlap_tokens: usize,
	language_pack_root: PathBuf,
	enabled_languages: Option<&[String]>,
	allow_download: bool,
	embedder_config: crate::config::EmbedderConfig,
	lancedb_path: PathBuf) -> Result<()> {
	sync_language_packs(&language_pack_root, enabled_languages, allow_download)?;
	let language_packs = load_language_packs(&language_pack_root, enabled_languages)?;
	logger::info(format!("loaded {} language packs", language_packs.len()));
	let languages = Arc::new(build_registry(&language_packs)?);
	let embedder = Arc::from(build_embedder(&embedder_config)?);
	logger::info(format!("using lancedb path: {}", lancedb_path.display()));
	let store = Arc::from(LanceDbStore::new(lancedb_path).await?);
	let handle = watcher::start_watcher(
		roots,
		debounce_ms,
		max_tokens,
		overlap_tokens,
		languages,
		embedder,
		store
	).await?;
	let _ = handle._task.await;
	Ok(())
}
