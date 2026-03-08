use std::path::PathBuf;
use std::collections::HashSet;
use std::sync::Arc;
use anyhow::{anyhow, Result};
use ignore::WalkBuilder;
use tokio::sync::Semaphore;
use crate::embedder::Embedder;
use crate::logger;
use crate::parser::{chunk_source_with_limits, detect_language, LanguageRegistry};
use crate::store::VectorStore;
use crate::types::EmbeddedChunk;
use xxhash_rust::xxh3::xxh3_128;

#[derive(Clone, Debug)]
pub struct IndexConfig {
	pub roots: Vec<PathBuf>,
	pub max_tokens: usize,
	pub overlap_tokens: usize,
	pub concurrency: usize,
}

pub async fn index_roots(
	config: IndexConfig,
	languages: Arc<LanguageRegistry>,
	embedder: Arc<dyn Embedder>,
	store: Arc<dyn VectorStore>) -> Result<()> {
	let mut paths = Vec::new();
	for root in &config.roots {
		let mut walker = WalkBuilder::new(root);
		walker.hidden(false);
		for entry in walker.build() {
			let entry = entry?;
			if !entry.file_type()
				.map(|ft| ft.is_file())
				.unwrap_or(false) {
				continue;
			}
			paths.push(entry.into_path());
		}
	}
	let concurrency = config.concurrency.max(1);
	let semaphore = Arc::new(Semaphore::new(concurrency));
	let mut handles = Vec::new();
	let seen: HashSet<String> = paths.iter()
		.map(|path| path.to_string_lossy().to_string())
		.collect();
	for path in paths {
		let permit = semaphore.clone().acquire_owned().await?;
		let languages = Arc::clone(&languages);
		let embedder = Arc::clone(&embedder);
		let store = Arc::clone(&store);
		let max_tokens = config.max_tokens;
		let overlap_tokens = config.overlap_tokens;
		handles.push(
			tokio::spawn(
				async move {
					let _permit = permit;
					if let Err(err) = index_file(
						path,
						languages,
						embedder,
						store,
						max_tokens,
						overlap_tokens
					).await {
						logger::error(format!("indexing failed: {}", err));
					}
				}
			)
		);
	}
	for handle in handles {
		let _ = handle.await;
	}
	cleanup_removed_files(&store, &seen).await?;
	Ok(())
}

pub async fn index_file(
	path: PathBuf,
	languages: Arc<LanguageRegistry>,
	embedder: Arc<dyn Embedder>,
	store: Arc<dyn VectorStore>,
	max_tokens: usize,
	overlap_tokens: usize) -> Result<()> {
	let Some(language) = detect_language(&languages, &path) else {
		return Ok(());
	};
	logger::info(format!("indexing file: {}", path.display()));
	let source = match std::fs::read_to_string(&path) {
		Ok(text) => text,
		Err(_) => return Ok(()),
	};
	let hash = hash_content(&source);
	let source_id = path.to_string_lossy().to_string();
	if let Some(existing) = store.get_source_hash("file", &source_id).await? {
		if existing == hash {
			return Ok(());
		}
	}
	let chunks = chunk_source_with_limits(
		&language,
		&path,
		&source,
		max_tokens,
		overlap_tokens
	)?;
	if chunks.is_empty() {
		return Ok(());
	}
	logger::info(format!("embedded {} chunks for {}", chunks.len(), path.display()));
	let texts = chunks.iter()
		.map(|chunk| chunk.text.clone())
		.collect::<Vec<_>>();
	let embedder = Arc::clone(&embedder);
	let embeddings = tokio::task::spawn_blocking(move || embedder.embed(&texts)).await.map_err(|err| anyhow!("embedding task failed: {}", err))??;
	if embeddings.len() != chunks.len() {
		return Err(anyhow!("embedding count mismatch"));
	}
	let mut embedded = Vec::with_capacity(chunks.len());
	for (chunk, embedding) in chunks.into_iter().zip(embeddings.into_iter()) {
		embedded.push(EmbeddedChunk { chunk, embedding });
	}
	store.delete_file(&path).await?;
	store.upsert_chunks(&embedded).await?;
	store.upsert_source_hash("file", &source_id, &hash).await?;
	Ok(())
}

pub async fn delete_file(path: PathBuf, store: Arc<dyn VectorStore>) -> Result<()> {
	let source_id = path.to_string_lossy().to_string();
	store.delete_file(&path).await?;
	store.delete_source("file", &source_id).await
}

async fn cleanup_removed_files(store: &Arc<dyn VectorStore>, seen: &HashSet<String>) -> Result<()> {
	let sources = store.list_sources("file").await?;
	for source_id in sources {
		if !seen.contains(&source_id) {
			let path = PathBuf::from(&source_id);
			logger::info(format!("removing stale file embeddings: {}", source_id));
			store.delete_file(&path).await?;
			store.delete_source("file", &source_id).await?;
		}
	}
	Ok(())
}

fn hash_content(content: &str) -> String {
	let hash = xxh3_128(content.as_bytes());
	format!("{:032x}", hash)
}
