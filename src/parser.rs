use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use anyhow::{anyhow, Result};
use libloading::Library;
use tree_sitter::{Language, Parser, Query};
use crate::chunker::structural_chunks;
use crate::langpacks::LanguagePack;
use crate::types::Chunk;

#[derive(Debug)]
pub struct LoadedLanguage {
	pub name: String,
	pub extensions: Vec<String>,
	pub language: Language,
	pub structural_query: Query,
	_lib: Arc<Library>,
}

#[derive(Clone, Debug)]
pub struct LanguageRegistry {
	by_extension: HashMap<String, Arc<LoadedLanguage>>,
}

pub fn load_language(pack: &LanguagePack) -> Result<LoadedLanguage> {
	let lib = unsafe { Library::new(&pack.parser_path) }.map_err(|err| anyhow!("failed to load {}: {}", pack.parser_path.display(), err))?;
	let symbol_name = format!("tree_sitter_{}", pack.config.name.replace('-', "_"));
	let language = unsafe {
		let func: libloading::Symbol<unsafe extern "C" fn() -> Language> = lib.get(symbol_name.as_bytes()).map_err(|err| anyhow!("missing symbol {}: {}", symbol_name, err))?;
		func()
	};
	let query_source = std::fs::read_to_string(&pack.structural_query_path)
		.map_err(|err| {
			anyhow!(
				"failed to read {}: {}",
				pack.structural_query_path.display(),
				err
			)
		})?;
	let structural_query = Query::new(&language, &query_source)
		.map_err(|err| anyhow!("invalid structural query for {}: {}", pack.config.name, err))?;
	Ok(
		LoadedLanguage {
			name: pack.config
				.name
				.clone(),
			extensions: pack.config
				.extensions
				.clone(),
			language,
			structural_query,
			_lib: Arc::new(lib)
		}
	)
}

pub fn build_registry(packs: &[LanguagePack]) -> Result<LanguageRegistry> {
	let mut by_extension = HashMap::new();
	for pack in packs {
		let loaded = Arc::new(load_language(pack)?);
		for ext in &loaded.extensions {
			by_extension.insert(ext.to_lowercase(), Arc::clone(&loaded));
		}
	}
	Ok(LanguageRegistry { by_extension })
}

pub fn detect_language(registry: &LanguageRegistry, path: &Path) -> Option<Arc<LoadedLanguage>> {
	let ext = path.extension().and_then(|ext| ext.to_str())?;
	registry.by_extension
		.get(&ext.to_lowercase())
		.cloned()
}

pub fn chunk_source_with_limits(
	language: &LoadedLanguage,
	file_path: &Path,
	source: &str,
	max_tokens: usize,
	overlap_tokens: usize) -> Result<Vec<Chunk>> {
	let mut parser = Parser::new();
	let tree = parse_source(&mut parser, language, source)?;
	Ok(
		structural_chunks(
			file_path,
			&language.name,
			source,
			&tree,
			&language.structural_query,
			max_tokens,
			overlap_tokens
		)
	)
}

pub fn parse_source(
	parser: &mut Parser,
	language: &LoadedLanguage,
	source: &str) -> Result<tree_sitter::Tree> {
	parser.set_language(&language.language).map_err(|_| anyhow!("failed to set parser language"))?;
	parser.parse(source, None).ok_or_else(|| anyhow!("failed to parse source"))
}
