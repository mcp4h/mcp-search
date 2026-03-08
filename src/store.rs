use std::path::{Path, PathBuf};
use std::sync::Arc;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use arrow_array::{
	Array,
	ArrayRef,
	FixedSizeListArray,
	Float32Array,
	RecordBatch,
	RecordBatchIterator,
	StringArray,
	UInt32Array,
};
use arrow_schema::{ArrowError, DataType, Field, Schema};
use lancedb::{connect, Connection, Table};
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use tokio_stream::StreamExt;
use crate::types::{Chunk, EmbeddedChunk, ChunkMetadata};

#[derive(Clone, Debug)]
pub struct SearchResult {
	pub chunk: Chunk,
	pub score: f32,
}

#[derive(Clone, Debug, Default)]
pub struct SearchFilter {
	pub allowed_roots: Vec<PathBuf>,
	pub allow_globs: Vec<String>,
	pub deny_globs: Vec<String>,
	pub allow_languages: Vec<String>,
	pub deny_languages: Vec<String>,
	pub max_results: Option<usize>,
	pub max_bytes: Option<usize>,
}

#[async_trait]
pub trait VectorStore: Send + Sync {
	async fn upsert_chunks(&self, chunks: &[EmbeddedChunk]) -> Result<()>;
	async fn delete_file(&self, path: &PathBuf) -> Result<()>;
	async fn search(
		&self,
		embedding: &[f32],
		filter: &SearchFilter,
		top_k: usize) -> Result<Vec<SearchResult>>;
	async fn get_source_hash(&self, source_type: &str, source_id: &str) -> Result<Option<String>>;
	async fn upsert_source_hash(
		&self,
		source_type: &str,
		source_id: &str,
		content_hash: &str) -> Result<()>;
	async fn delete_source(&self, source_type: &str, source_id: &str) -> Result<()>;
	async fn list_sources(&self, source_type: &str) -> Result<Vec<String>>;
}

pub struct LanceDbStore {
	connection: Connection,
}

impl LanceDbStore {
	pub async fn new(path: PathBuf) -> Result<Self> {
		let connection = connect(path.to_string_lossy().as_ref()).execute().await.map_err(|err| anyhow!("failed to open lancedb: {}", err))?;
		Ok(Self { connection })
	}
	async fn open_or_create_table(&self, embedding_dim: usize) -> Result<Table> {
		let table_name = "chunks";
		if let Ok(table) = self.connection
			.open_table(table_name)
			.execute().await {
			return Ok(table);
		}
		let schema = Arc::new(
			Schema::new(
				vec![
				Field::new("chunk_id", DataType::Utf8, false),
				Field::new("file_path", DataType::Utf8, false),
				Field::new("language", DataType::Utf8, false),
				Field::new("node_kind", DataType::Utf8, false),
				Field::new("start_line", DataType::UInt32, false),
				Field::new("end_line", DataType::UInt32, false),
				Field::new("parent_context", DataType::Utf8, true),
				Field::new("text", DataType::Utf8, false),
				Field::new(
					"embedding",
					DataType::FixedSizeList(
						Arc::new(Field::new("item", DataType::Float32, false)),
						embedding_dim as i32,
					),
					false,
				),
				]
			)
		);
		self.connection
			.create_empty_table(table_name, schema)
			.execute().await.map_err(|err| anyhow!("failed to create lancedb table: {}", err))
	}
	async fn open_or_create_sources_table(&self) -> Result<Table> {
		let table_name = "sources";
		if let Ok(table) = self.connection
			.open_table(table_name)
			.execute().await {
			return Ok(table);
		}
		let schema = Arc::new(
			Schema::new(
				vec![
				Field::new("source_id", DataType::Utf8, false),
				Field::new("source_type", DataType::Utf8, false),
				Field::new("content_hash", DataType::Utf8, false),
				]
			)
		);
		self.connection
			.create_empty_table(table_name, schema)
			.execute().await.map_err(|err| anyhow!("failed to create sources table: {}", err))
	}
	fn build_sources_batch(
		&self,
		source_type: &str,
		source_id: &str,
		content_hash: &str) -> Result<RecordBatch> {
		let schema = Arc::new(
			Schema::new(
				vec![
				Field::new("source_id", DataType::Utf8, false),
				Field::new("source_type", DataType::Utf8, false),
				Field::new("content_hash", DataType::Utf8, false),
				]
			)
		);
		let source_ids = StringArray::from(vec![source_id]);
		let source_types = StringArray::from(vec![source_type]);
		let hashes = StringArray::from(vec![content_hash]);
		RecordBatch::try_new(schema, vec![Arc::new(source_ids), Arc::new(source_types), Arc::new(hashes)])
			.map_err(|err| anyhow!("failed to build sources batch: {}", err))
	}
	fn escape_sql(value: &str) -> String {
		value.replace('"', "\"").replace('\\', "\\\\")
	}
	fn is_missing_table_error(err: &lancedb::Error) -> bool {
		let message = err.to_string();
		message.contains("Table 'chunks' was not found")
			|| message.contains("Table 'sources' was not found")
			|| message.contains("table was not found")
			|| message.contains("table not found")
	}
	fn build_record_batch(&self, chunks: &[EmbeddedChunk]) -> Result<RecordBatch> {
		if chunks.is_empty() {
			return Err(anyhow!("no chunks to upsert"));
		}
		let embedding_dim = chunks[0].embedding.len();
		if embedding_dim == 0 {
			return Err(anyhow!("embedding dimension is zero"));
		}
		for chunk in chunks {
			if chunk.embedding.len() != embedding_dim {
				return Err(anyhow!("embedding dimension mismatch"));
			}
		}
		let chunk_ids = StringArray::from(
			chunks.iter()
				.map(|chunk| chunk.chunk
					.id
					.as_str())
				.collect::<Vec<_>>()
		);
		let file_paths = StringArray::from(
			chunks.iter()
				.map(
					|chunk| chunk.chunk
						.metadata
						.file_path
						.to_string_lossy()
						.to_string())
				.collect::<Vec<_>>()
		);
		let languages = StringArray::from(
			chunks.iter()
				.map(
					|chunk| chunk.chunk
						.metadata
						.language
						.as_str())
				.collect::<Vec<_>>()
		);
		let node_kinds = StringArray::from(
			chunks.iter()
				.map(
					|chunk| chunk.chunk
						.metadata
						.node_kind
						.as_str())
				.collect::<Vec<_>>()
		);
		let start_lines = UInt32Array::from(
			chunks.iter()
				.map(|chunk| chunk.chunk
					.metadata
					.start_line as u32)
				.collect::<Vec<_>>()
		);
		let end_lines = UInt32Array::from(
			chunks.iter()
				.map(|chunk| chunk.chunk
					.metadata
					.end_line as u32)
				.collect::<Vec<_>>()
		);
		let parent_context = StringArray::from(
			chunks.iter()
				.map(
					|chunk| chunk.chunk
						.metadata
						.parent_context
						.as_deref())
				.collect::<Vec<_>>()
		);
		let texts = StringArray::from(
			chunks.iter()
				.map(|chunk| chunk.chunk
					.text
					.as_str())
				.collect::<Vec<_>>()
		);
		let mut values = Vec::with_capacity(chunks.len() * embedding_dim);
		for chunk in chunks {
			values.extend_from_slice(&chunk.embedding);
		}
		let values = Float32Array::from(values);
		let list_values: ArrayRef = Arc::new(values);
		let embedding = FixedSizeListArray::try_new(
			Arc::new(Field::new("item", DataType::Float32, false)),
			embedding_dim as i32,
			list_values,
			None
		)
			.map_err(|err| anyhow!("failed to build embedding array: {}", err))?;
		let schema = Arc::new(
			Schema::new(
				vec![
				Field::new("chunk_id", DataType::Utf8, false),
				Field::new("file_path", DataType::Utf8, false),
				Field::new("language", DataType::Utf8, false),
				Field::new("node_kind", DataType::Utf8, false),
				Field::new("start_line", DataType::UInt32, false),
				Field::new("end_line", DataType::UInt32, false),
				Field::new("parent_context", DataType::Utf8, true),
				Field::new("text", DataType::Utf8, false),
				Field::new(
					"embedding",
					DataType::FixedSizeList(
						Arc::new(Field::new("item", DataType::Float32, false)),
						embedding_dim as i32,
					),
					false,
				),
				]
			)
		);
		RecordBatch::try_new(
			schema,
			vec![
			Arc::new(chunk_ids),
			Arc::new(file_paths),
			Arc::new(languages),
			Arc::new(node_kinds),
			Arc::new(start_lines),
			Arc::new(end_lines),
			Arc::new(parent_context),
			Arc::new(texts),
			Arc::new(embedding),
			]
		)
			.map_err(|err| anyhow!("failed to build record batch: {}", err))
	}
	fn build_glob_set(patterns: &[String]) -> Result<Option<GlobSet>> {
		if patterns.is_empty() {
			return Ok(None);
		}
		let mut builder = GlobSetBuilder::new();
		for pattern in patterns {
			let glob = Glob::new(pattern).map_err(|err| anyhow!("invalid glob {}: {}", pattern, err))?;
			builder.add(glob);
		}
		Ok(Some(builder.build().map_err(|err| anyhow!("invalid glob set: {}", err))?))
	}
	fn path_allowed(
		path: &Path,
		root: &Path,
		allow: &Option<GlobSet>,
		deny: &Option<GlobSet>) -> bool {
		let rel = path.strip_prefix(root).ok();
		let rel = rel.and_then(|rel| rel.to_str()).unwrap_or("");
		if rel.is_empty() {
			return true;
		}
		if let Some(set) = deny {
			if set.is_match(rel) {
				return false;
			}
		}
		if let Some(set) = allow {
			return set.is_match(rel);
		}
		true
	}
}

#[async_trait]
impl VectorStore for LanceDbStore {
	async fn upsert_chunks(&self, chunks: &[EmbeddedChunk]) -> Result<()> {
		if chunks.is_empty() {
			return Ok(());
		}
		let embedding_dim = chunks[0].embedding.len();
		let table = self.open_or_create_table(embedding_dim).await?;
		let batch = self.build_record_batch(chunks)?;
		let schema = batch.schema();
		let batches = RecordBatchIterator::new(vec![Ok::<RecordBatch, ArrowError>(batch)], schema);
		table.add(batches).execute().await.map_err(|err| anyhow!("failed to insert: {}", err))?;
		Ok(())
	}
	async fn delete_file(&self, path: &PathBuf) -> Result<()> {
		let table = match self.connection
			.open_table("chunks")
			.execute().await {
			Ok(table) => table,
			Err(err) => {
				if Self::is_missing_table_error(&err) {
					return Ok(());
				}
				return Err(anyhow!("failed to open table: {}", err));
			}
		};
		let filter = format!("file_path = '{}'", path.to_string_lossy().replace('"', "\\\""));
		table.delete(&filter).await.map_err(|err| anyhow!("failed to delete rows: {}", err))?;
		Ok(())
	}
	async fn search(
		&self,
		embedding: &[f32],
		filter: &SearchFilter,
		top_k: usize) -> Result<Vec<SearchResult>> {
		let table = match self.connection
			.open_table("chunks")
			.execute().await {
			Ok(table) => table,
			Err(err) => {
				if Self::is_missing_table_error(&err) {
					return Ok(Vec::new());
				}
				return Err(anyhow!("failed to open table: {}", err));
			}
		};
		let mut stream = table.query().nearest_to(embedding)?.limit(top_k).execute().await.map_err(|err| anyhow!("search failed: {}", err))?;
		let allow_set = Self::build_glob_set(&filter.allow_globs)?;
		let deny_set = Self::build_glob_set(&filter.deny_globs)?;
		let mut results = Vec::new();
		while let Some(batch) = stream.next().await {
			let batch = batch.map_err(|err| anyhow!("search batch error: {}", err))?;
			let chunk_id = batch.column_by_name("chunk_id").ok_or_else(|| anyhow!("missing chunk_id column"))?.as_any()
				.downcast_ref::<StringArray>()
				.ok_or_else(|| anyhow!("chunk_id column type"))?;
			let file_path = batch.column_by_name("file_path").ok_or_else(|| anyhow!("missing file_path column"))?.as_any()
				.downcast_ref::<StringArray>()
				.ok_or_else(|| anyhow!("file_path column type"))?;
			let language = batch.column_by_name("language").ok_or_else(|| anyhow!("missing language column"))?.as_any()
				.downcast_ref::<StringArray>()
				.ok_or_else(|| anyhow!("language column type"))?;
			let node_kind = batch.column_by_name("node_kind").ok_or_else(|| anyhow!("missing node_kind column"))?.as_any()
				.downcast_ref::<StringArray>()
				.ok_or_else(|| anyhow!("node_kind column type"))?;
			let start_line = batch.column_by_name("start_line").ok_or_else(|| anyhow!("missing start_line column"))?.as_any()
				.downcast_ref::<UInt32Array>()
				.ok_or_else(|| anyhow!("start_line column type"))?;
			let end_line = batch.column_by_name("end_line").ok_or_else(|| anyhow!("missing end_line column"))?.as_any()
				.downcast_ref::<UInt32Array>()
				.ok_or_else(|| anyhow!("end_line column type"))?;
			let parent_context = batch.column_by_name("parent_context").ok_or_else(|| anyhow!("missing parent_context column"))?.as_any()
				.downcast_ref::<StringArray>()
				.ok_or_else(|| anyhow!("parent_context column type"))?;
			let text = batch.column_by_name("text").ok_or_else(|| anyhow!("missing text column"))?.as_any().downcast_ref::<StringArray>()
				.ok_or_else(|| anyhow!("text column type"))?;
			let score: Option<&Float32Array> = batch.column_by_name("_distance").and_then(|col| col.as_any().downcast_ref::<Float32Array>());
			for row in 0..batch.num_rows() {
				let file_path_str = file_path.value(row);
				let path = PathBuf::from(file_path_str);
				if !filter.allowed_roots.is_empty()
					&& !filter.allowed_roots
						.iter()
						.any(|root| path.starts_with(root)) {
					continue;
				}
				let mut allowed = true;
				for root in &filter.allowed_roots {
					if path.starts_with(root) {
						if !Self::path_allowed(
							&path,
							root,
							&allow_set,
							&deny_set
						) {
							allowed = false;
						}
						break;
					}
				}
				if !allowed {
					continue;
				}
				let lang = language.value(row);
				if !filter.allow_languages.is_empty()
					&& !filter.allow_languages
						.iter()
						.any(|item| item == lang) {
					continue;
				}
				if filter.deny_languages
					.iter()
					.any(|item| item == lang) {
					continue;
				}
				let parent = if parent_context.is_null(row) {
					None
				}
				else {
					Some(parent_context.value(row).to_string())
				};
				let score = score.map(|col| col.value(row)).unwrap_or(0.0);
				results.push(
					SearchResult {
						chunk: Chunk {
							id: chunk_id.value(row).to_string(),
							text: text.value(row).to_string(),
							metadata: ChunkMetadata {
								file_path: path,
								language: lang.to_string(),
								node_kind: node_kind.value(row).to_string(),
								start_line: start_line.value(row) as usize,
								end_line: end_line.value(row) as usize,
								parent_context: parent
							}
						},
						score
					}
				);
			}
		}
		Ok(results)
	}
	async fn get_source_hash(&self, source_type: &str, source_id: &str) -> Result<Option<String>> {
		let table = match self.connection
			.open_table("sources")
			.execute().await {
			Ok(table) => table,
			Err(_) => return Ok(None),
		};
		let filter = format!(
			"source_type = '{}' AND source_id = '{}'",
			Self::escape_sql(source_type),
			Self::escape_sql(source_id)
		);
		let mut stream = table.query()
			.only_if(&filter)
			.select(Select::columns(&["content_hash"]))
			.limit(1)
			.execute().await.map_err(|err| anyhow!("failed to query sources: {}", err))?;
		while let Some(batch) = stream.next().await {
			let batch = batch.map_err(|err| anyhow!("sources batch error: {}", err))?;
			let hashes = batch.column_by_name("content_hash").ok_or_else(|| anyhow!("missing content_hash column"))?.as_any()
				.downcast_ref::<StringArray>()
				.ok_or_else(|| anyhow!("content_hash column type"))?;
			if hashes.len() > 0 {
				return Ok(Some(hashes.value(0).to_string()));
			}
		}
		Ok(None)
	}
	async fn upsert_source_hash(
		&self,
		source_type: &str,
		source_id: &str,
		content_hash: &str) -> Result<()> {
		let table = self.open_or_create_sources_table().await?;
		let filter = format!(
			"source_type = '{}' AND source_id = '{}'",
			Self::escape_sql(source_type),
			Self::escape_sql(source_id)
		);
		let _ = table.delete(&filter).await;
		let batch = self.build_sources_batch(source_type, source_id, content_hash)?;
		let schema = batch.schema();
		let batches = RecordBatchIterator::new(vec![Ok::<RecordBatch, ArrowError>(batch)], schema);
		table.add(batches).execute().await.map_err(|err| anyhow!("failed to insert source: {}", err))?;
		Ok(())
	}
	async fn delete_source(&self, source_type: &str, source_id: &str) -> Result<()> {
		let table = match self.connection
			.open_table("sources")
			.execute().await {
			Ok(table) => table,
			Err(_) => return Ok(()),
		};
		let filter = format!(
			"source_type = '{}' AND source_id = '{}'",
			Self::escape_sql(source_type),
			Self::escape_sql(source_id)
		);
		table.delete(&filter).await.map_err(|err| anyhow!("failed to delete source: {}", err))?;
		Ok(())
	}
	async fn list_sources(&self, source_type: &str) -> Result<Vec<String>> {
		let table = match self.connection
			.open_table("sources")
			.execute().await {
			Ok(table) => table,
			Err(_) => return Ok(Vec::new()),
		};
		let filter = format!(
			"source_type = '{}'",
			Self::escape_sql(source_type)
		);
		let mut stream = table.query()
			.only_if(&filter)
			.select(Select::columns(&["source_id"]))
			.execute().await.map_err(|err| anyhow!("failed to scan sources: {}", err))?;
		let mut ids = Vec::new();
		while let Some(batch) = stream.next().await {
			let batch = batch.map_err(|err| anyhow!("sources scan error: {}", err))?;
			let sources = batch.column_by_name("source_id").ok_or_else(|| anyhow!("missing source_id column"))?.as_any()
				.downcast_ref::<StringArray>()
				.ok_or_else(|| anyhow!("source_id column type"))?;
			for row in 0..sources.len() {
				ids.push(sources.value(row).to_string());
			}
		}
		Ok(ids)
	}
}
