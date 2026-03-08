use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Chunk {
	pub id: String,
	pub text: String,
	pub metadata: ChunkMetadata,
}

#[derive(Clone, Debug)]
pub struct EmbeddedChunk {
	pub chunk: Chunk,
	pub embedding: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct ChunkMetadata {
	pub file_path: PathBuf,
	pub language: String,
	pub node_kind: String,
	pub start_line: usize,
	pub end_line: usize,
	pub parent_context: Option<String>,
}
