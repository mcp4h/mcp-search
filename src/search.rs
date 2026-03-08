use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use crate::embedder::Embedder;
use crate::policy;
use crate::store::{SearchFilter, VectorStore};

pub struct SearchContext <'a> {
	pub embedder: &'a dyn Embedder,
	pub store: &'a dyn VectorStore,
}

pub async fn search(
	config: &crate::config::Config,
	context: SearchContext<'_>,
	arguments: &Value,
	meta: &Value) -> Result<Value> {
	let query = arguments.get("query")
		.and_then(Value::as_str)
		.ok_or_else(|| anyhow!("query is required"))?;
	let top_k = arguments.get("limit")
		.and_then(Value::as_u64)
		.unwrap_or(5);
	let call_config = policy::resolve_call_config(config, meta)?;
	if call_config.roots.is_empty() {
		return Err(anyhow!("no roots configured"));
	}
	let mut top_k = top_k as usize;
	if let Some(limit) = call_config.max_results {
		top_k = top_k.min(limit);
	}
	let embedding = context.embedder.embed(&[query.to_string()])?;
	let embedding = embedding.get(0).ok_or_else(|| anyhow!("embedder returned no vectors"))?;
	let filter = build_filter(&call_config);
	let results = context.store.search(embedding, &filter, top_k).await?;
	let payload = filter_results(results, &call_config)
		.into_iter()
		.map(
			|result| {
				let text = strip_chunk_header(&result.chunk.text);
				json!({
					"score": result.score,
					"text": text,
					"metadata": {
				"file_path": result.chunk.metadata.file_path,
				"language": result.chunk.metadata.language,
				"node_kind": result.chunk.metadata.node_kind,
				"start_line": result.chunk.metadata.start_line,
				"end_line": result.chunk.metadata.end_line,
				"parent_context": result.chunk.metadata.parent_context,
			}
				})
			})
		.collect::<Vec<_>>();
	Ok(json!({
		"matches": payload,
		"count": payload.len()
	}))
}

fn strip_chunk_header(text: &str) -> String {
	if let Some((_, body)) = text.split_once("\n\n") {
		return body.to_string();
	}
	if let Some(index) = text.find("\r\n\r\n") {
		return text[index + 4..].to_string();
	}
	text.to_string()
}

fn build_filter(call_config: &policy::CallConfig) -> SearchFilter {
	let mut filter = SearchFilter::default();
	for root in &call_config.roots {
		filter.allowed_roots.push(root.path.clone());
		filter.allow_globs.extend(root.allow.clone());
		filter.deny_globs.extend(root.deny.clone());
	}
	filter.allow_globs.extend(call_config.allow_globs.clone());
	filter.deny_globs.extend(call_config.deny_globs.clone());
	filter.allow_languages = call_config.allow_languages.clone();
	filter.deny_languages = call_config.deny_languages.clone();
	filter.max_results = call_config.max_results;
	filter.max_bytes = call_config.max_bytes;
	filter
}

fn filter_results(
	results: Vec<crate::store::SearchResult>,
	config: &policy::CallConfig) -> Vec<crate::store::SearchResult> {
	let mut filtered = Vec::new();
	let mut total_bytes = 0usize;
	for result in results {
		if !matches_language(&result, config) {
			continue;
		}
		if let Some(limit) = config.max_bytes {
			let next = total_bytes + result.chunk
				.text
				.len();
			if next > limit {
				break;
			}
			total_bytes = next;
		}
		filtered.push(result);
		if let Some(limit) = config.max_results {
			if filtered.len() >= limit {
				break;
			}
		}
	}
	filtered
}

fn matches_language(result: &crate::store::SearchResult, config: &policy::CallConfig) -> bool {
	let language = result.chunk
		.metadata
		.language
		.as_str();
	if !config.allow_languages.is_empty()
		&& !config.allow_languages
			.iter()
			.any(|item| item == language) {
		return false;
	}
	if config.deny_languages
		.iter()
		.any(|item| item == language) {
		return false;
	}
	true
}
