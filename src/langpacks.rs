use std::collections::HashSet;
use std::path::{Path, PathBuf};
use anyhow::{anyhow, Result};
use ignore::WalkBuilder;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use crate::logger;

#[derive(Clone, Debug)]
pub struct LanguagePack {
	pub config: LanguageConfig,
	pub parser_path: PathBuf,
	pub structural_query_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LanguageConfig {
	pub name: String,
	pub extensions: Vec<String>,
	pub parser: String,
	pub custom: Option<bool>,
	pub queries: LanguageQueries,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LanguageQueries {
	pub structural: String,
}

#[derive(Clone, Debug)]
struct KnownLanguage {
	name: &'static str,
	repo: &'static str,
	extensions: &'static [&'static str],
	query: &'static str,
}

const KNOWN_LANGUAGES: &[KnownLanguage] = &[
	KnownLanguage {
		name: "css",
		repo: "neatify-tech/tree-sitter-css",
		extensions: &["css"],
		query: "(rule_set) @structural\n(at_rule) @structural\n"
	},
	KnownLanguage {
		name: "html",
		repo: "neatify-tech/tree-sitter-html",
		extensions: &["html", "htm"],
		query: "(element) @structural\n(script_element) @structural\n(style_element) @structural\n"
	},
	KnownLanguage {
		name: "java",
		repo: "neatify-tech/tree-sitter-java",
		extensions: &["java"],
		query: "(class_declaration) @structural\n(interface_declaration) @structural\n(enum_declaration) @structural\n(record_declaration) @structural\n(method_declaration) @structural\n(constructor_declaration) @structural\n"
	},
	KnownLanguage {
		name: "json",
		repo: "neatify-tech/tree-sitter-json",
		extensions: &["json"],
		query: "(object) @structural\n(array) @structural\n(pair) @structural\n"
	},
	KnownLanguage {
		name: "markdown",
		repo: "neatify-tech/tree-sitter-markdown",
		extensions: &["md", "markdown"],
		query: "(section) @structural\n(atx_heading) @structural\n(setext_heading) @structural\n(fenced_code_block) @structural\n(list) @structural\n(block_quote) @structural\n"
	},
	KnownLanguage {
		name: "rhai",
		repo: "neatify-tech/tree-sitter-rhai",
		extensions: &["rhai"],
		query: "(DefFn) @structural\n(DefConst) @structural\n(DefLet) @structural\n"
	},
	KnownLanguage {
		name: "rust",
		repo: "neatify-tech/tree-sitter-rust",
		extensions: &["rs"],
		query: "(function_item) @structural\n(struct_item) @structural\n(enum_item) @structural\n(impl_item) @structural\n(trait_item) @structural\n(mod_item) @structural\n(type_item) @structural\n(const_item) @structural\n(static_item) @structural\n"
	},
	KnownLanguage {
		name: "sql",
		repo: "neatify-tech/tree-sitter-sql",
		extensions: &["sql"],
		query: "(select_statement) @structural\n(with_clause) @structural\n(cte) @structural\n"
	},
	KnownLanguage {
		name: "typescript",
		repo: "neatify-tech/tree-sitter-typescript",
		extensions: &["ts", "tsx"],
		query: "(function_declaration) @structural\n(class_declaration) @structural\n(interface_declaration) @structural\n(type_alias_declaration) @structural\n(enum_declaration) @structural\n(method_definition) @structural\n"
	},
	KnownLanguage {
		name: "vue",
		repo: "neatify-tech/tree-sitter-vue",
		extensions: &["vue"],
		query: "(element) @structural\n(template_element) @structural\n(script_element) @structural\n(style_element) @structural\n"
	},
	KnownLanguage {
		name: "xml",
		repo: "neatify-tech/tree-sitter-xml",
		extensions: &["xml", "xsd", "xsl", "xslt"],
		query: "(element) @structural\n"
	}
];

pub fn load_language_packs(root: &Path, enabled: Option<&[String]>) -> Result<Vec<LanguagePack>> {
	let mut packs = Vec::new();
	let enabled_set = enabled.map(
		|items| {
			items.iter()
				.map(|item| item.to_lowercase())
				.collect::<HashSet<_>>()
		}
	);
	let mut walker = WalkBuilder::new(root);
	walker.hidden(false);
	for entry in walker.build() {
		let entry = entry?;
		if !entry.file_type()
			.map(|ft| ft.is_file())
			.unwrap_or(false) {
			continue;
		}
		if entry.file_name() != "language.toml" {
			continue;
		}
		let config_path = entry.into_path();
		let pack_root = config_path.parent().ok_or_else(|| anyhow!("language.toml missing parent"))?;
		let content = std::fs::read_to_string(&config_path)
			.map_err(|err| anyhow!("failed to read {}: {}", config_path.display(), err))?;
		let config: LanguageConfig = toml::from_str(&content).map_err(|err| anyhow!("failed to parse {}: {}", config_path.display(), err))?;
		if let Some(set) = enabled_set.as_ref() {
			if !set.contains(&config.name.to_lowercase()) {
				continue;
			}
		}
		let parser_path = resolve_parser_path(pack_root, &config.parser)?;
		let structural_query_path = pack_root.join(&config.queries.structural);
		if !structural_query_path.exists() {
			logger::warn(format!(
				"missing structural query: {}",
				structural_query_path.display()
			));
			continue;
		}
		if !parser_path.exists() {
			logger::warn(format!("missing parser binary: {}", parser_path.display()));
			continue;
		}
		packs.push(LanguagePack {
			config,
			parser_path,
			structural_query_path
		});
	}
	Ok(packs)
}

pub fn sync_language_packs(
	root: &Path,
	enabled: Option<&[String]>,
	allow_download: bool) -> Result<()> {
	let enabled_names = resolve_enabled(enabled);
	for language in KNOWN_LANGUAGES {
		if !enabled_names.contains(language.name) {
			continue;
		}
		if let Err(err) = sync_language(root, language, allow_download) {
			logger::error(format!("failed to sync {}: {}", language.name, err));
		}
	}
	Ok(())
}

fn resolve_enabled(enabled: Option<&[String]>) -> HashSet<&'static str> {
	match enabled {
		Some(list) if !list.is_empty() => list.iter()
			.filter_map(
				|name| {
					KNOWN_LANGUAGES.iter()
						.find(|lang| lang.name.eq_ignore_ascii_case(name))
						.map(|lang| lang.name)
				})
			.collect(),
		_ => KNOWN_LANGUAGES.iter()
			.map(|lang| lang.name)
			.collect(),
	}
}

fn sync_language(root: &Path, language: &KnownLanguage, allow_download: bool) -> Result<()> {
	let pack_root = root.join(language.name);
	std::fs::create_dir_all(&pack_root)
		.map_err(|err| anyhow!("failed to create {}: {}", pack_root.display(), err))?;
	let config_path = pack_root.join("language.toml");
	if !config_path.exists() {
		let config = format!(
			"name = \"{}\"\nextensions = [{}]\nparser = \"tree-sitter-{}-{{os}}-{{arch}}{{ext}}\"\ncustom = false\n\n[queries]\nstructural = \"queries.scm\"\n",
			language.name,
			language
			.extensions
			.iter()
			.map(|ext| format!("\"{}\"", ext))
			.collect::<Vec<_>>()
			.join(", "),
			language.name,
		);
		std::fs::write(&config_path, config)
			.map_err(|err| anyhow!("failed to write {}: {}", config_path.display(), err))?;
		logger::info(format!("created {}", config_path.display()));
	}
	let content = std::fs::read_to_string(&config_path)
		.map_err(|err| anyhow!("failed to read {}: {}", config_path.display(), err))?;
	let config: LanguageConfig = toml::from_str(&content).map_err(|err| anyhow!("failed to parse {}: {}", config_path.display(), err))?;
	let structural_path = pack_root.join("queries.scm");
	let custom = config.custom.unwrap_or(false);
	if !structural_path.exists() {
		std::fs::write(&structural_path, language.query)
			.map_err(|err| anyhow!("failed to write {}: {}", structural_path.display(), err))?;
		logger::info(format!("created {}", structural_path.display()));
	}
	else if !custom {
		std::fs::write(&structural_path, language.query)
			.map_err(|err| anyhow!("failed to overwrite {}: {}", structural_path.display(), err))?;
		logger::info(format!("updated {}", structural_path.display()));
	}
	else {
		logger::info(format!("custom queries preserved for {}", language.name));
	}
	let parser_path = resolve_parser_path(&pack_root, &config.parser)?;
	if !parser_path.exists() {
		if allow_download {
			download_parser(&parser_path, language)?;
		}
		else {
			log_manual_download(language, &expected_asset_name(language.name), &parser_path);
			return Err(anyhow!("parser download disabled"));
		}
	}
	Ok(())
}

fn download_parser(path: &Path, language: &KnownLanguage) -> Result<()> {
	let asset_name = expected_asset_name(language.name);
	if let Err(err) = download_parser_inner(path, language, &asset_name) {
		log_manual_download(language, &asset_name, path);
		return Err(err);
	}
	Ok(())
}

fn download_parser_inner(path: &Path, language: &KnownLanguage, asset_name: &str) -> Result<()> {
	let latest_url = format!(
		"https://api.github.com/repos/{}/releases/latest",
		language.repo
	);
	let client = Client::builder()
		.user_agent("mcp-search")
		.build()
		.map_err(|err| anyhow!("failed to build http client: {}", err))?;
	let response = client.get(&latest_url)
		.send()
		.map_err(|err| anyhow!("failed to query {}: {}", latest_url, err))?.error_for_status()
		.map_err(|err| anyhow!("release query failed: {}", err))?;
	let payload: Value = response.json().map_err(|err| anyhow!("invalid release json: {}", err))?;
	let assets = payload.get("assets")
		.and_then(|value| value.as_array())
		.ok_or_else(|| anyhow!("release assets missing for {}", language.repo))?;
	let asset = assets.iter()
		.find(
			|asset| {
				asset.get("name")
					.and_then(|value| value.as_str())
					.map(|name| name == asset_name)
					.unwrap_or(false)
			}
		);
	let Some(asset) = asset else {
		return Err(anyhow!("release asset not found: {}", asset_name));
	};
	let download_url = asset.get("browser_download_url")
		.and_then(|value| value.as_str())
		.ok_or_else(|| anyhow!("asset download url missing for {}", language.repo))?;
	let bytes = client.get(download_url)
		.send()
		.map_err(|err| anyhow!("failed to download {}: {}", download_url, err))?.error_for_status()
		.map_err(|err| anyhow!("download failed: {}", err))?.bytes()
		.map_err(|err| anyhow!("download read failed: {}", err))?;
	let temp_path = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
	std::fs::write(&temp_path, &bytes)
		.map_err(|err| anyhow!("failed to write {}: {}", temp_path.display(), err))?;
	std::fs::rename(&temp_path, path)
		.map_err(|err| anyhow!("failed to move {}: {}", temp_path.display(), err))?;
	logger::info(format!(
		"downloaded parser for {} -> {}",
		language.name,
		path.display()
	));
	Ok(())
}

fn expected_asset_name(language: &str) -> String {
	let os = std::env::consts::OS;
	let arch = std::env::consts::ARCH;
	let ext = match os {
		"windows" => ".dll",
		"macos" => ".dylib",
		_ => ".so",
	};
	format!("tree-sitter-{}-{}-{}{}", language, os, arch, ext)
}

fn log_manual_download(language: &KnownLanguage, asset_name: &str, path: &Path) {
	logger::warn(
		format!(
			"missing parser for {}. Download '{}' from https://github.com/{}/releases/latest and place it at {}",
			language.name,
			asset_name,
			language.repo,
			path.display()
		)
	);
	logger::warn(
		format!(
			"ensure {}/language.toml and {}/queries.scm exist (custom=true prevents overwrite)",
			path.parent()
			.map(|p| p.display().to_string())
			.unwrap_or_else(|| "<language pack>".to_string()),
			path.parent()
			.map(|p| p.display().to_string())
			.unwrap_or_else(|| "<language pack>".to_string()),
		)
	);
}

pub fn resolve_parser_path(root: &Path, template: &str) -> Result<PathBuf> {
	let os = std::env::consts::OS;
	let arch = std::env::consts::ARCH;
	let ext = match os {
		"windows" => ".dll",
		"macos" => ".dylib",
		_ => ".so",
	};
	let replaced = template.replace("{os}", os)
		.replace("{arch}", arch)
		.replace("{ext}", ext);
	Ok(root.join(replaced))
}
