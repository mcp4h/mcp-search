use std::path::PathBuf;
use anyhow::{anyhow, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use serde_json::Value;
use crate::config::{Config, RootConfig};

#[derive(Clone, Debug)]
pub struct CallConfig {
	pub roots: Vec<CallRoot>,
	pub allow_globs: Vec<String>,
	pub deny_globs: Vec<String>,
	pub allow_languages: Vec<String>,
	pub deny_languages: Vec<String>,
	pub max_results: Option<usize>,
	pub max_bytes: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct CallRoot {
	pub path: PathBuf,
	pub path_canon: PathBuf,
	pub default: bool,
	pub blocked: bool,
	pub deny: Vec<String>,
	pub allow: Vec<String>,
	pub deny_set: Option<GlobSet>,
	pub allow_set: Option<GlobSet>,
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

pub fn resolve_call_config(config: &Config, meta: &Value) -> Result<CallConfig> {
	let policy = meta.get("policy");
	let mut roots = Vec::new();
	let mut allow_globs = Vec::new();
	let mut deny_globs = Vec::new();
	let mut allow_languages = Vec::new();
	let mut deny_languages = Vec::new();
	let mut max_results = None;
	let mut max_bytes = None;
	for root in &config.roots {
		roots.push(build_call_root(root)?);
	}
	if let Some(policy_value) = policy {
		let settings = apply_policy_to_roots(&mut roots, policy_value, config)?;
		allow_globs = settings.allow_globs;
		deny_globs = settings.deny_globs;
		allow_languages = settings.allow_languages;
		deny_languages = settings.deny_languages;
		max_results = settings.max_results;
		max_bytes = settings.max_bytes;
		return Ok(
			CallConfig {
				roots: roots.into_iter()
					.filter(|root| !root.blocked)
					.collect(),
				allow_globs,
				deny_globs,
				allow_languages,
				deny_languages,
				max_results,
				max_bytes
			}
		);
	}
	Ok(CallConfig {
		roots,
		allow_globs,
		deny_globs,
		allow_languages,
		deny_languages,
		max_results,
		max_bytes
	})
}

fn build_call_root(root: &RootConfig) -> Result<CallRoot> {
	Ok(
		CallRoot {
			path: root.path.clone(),
			path_canon: root.path_canon.clone(),
			default: root.default,
			blocked: false,
			deny: root.deny.clone(),
			allow: root.allow.clone(),
			deny_set: build_glob_set(&root.deny)?,
			allow_set: build_glob_set(&root.allow)?
		}
	)
}

#[derive(Clone, Debug, Default)]
struct PolicySettings {
	allow_globs: Vec<String>,
	deny_globs: Vec<String>,
	allow_languages: Vec<String>,
	deny_languages: Vec<String>,
	max_results: Option<usize>,
	max_bytes: Option<usize>,
}

fn apply_policy_to_roots(
	roots: &mut [CallRoot],
	policy: &Value,
	config: &Config) -> Result<PolicySettings> {
	let obj = policy.as_object().ok_or_else(|| anyhow!("policy must be an object"))?;
	let mut policy_roots: Vec<RootInput> = Vec::new();
	let mut settings = PolicySettings::default();
	for (key, value) in obj {
		match key.as_str() {
			"roots" => {
				policy_roots = serde_json::from_value(value.clone()).map_err(|err| anyhow!("invalid policy roots: {}", err))?;
			}
			"allow" => {
				settings.allow_globs = parse_string_list(value, "allow")?;
			}
			"deny" => {
				settings.deny_globs = parse_string_list(value, "deny")?;
			}
			"languages" => {
				settings.allow_languages = parse_string_list(value, "languages")?;
			}
			"deny_languages" => {
				settings.deny_languages = parse_string_list(value, "deny_languages")?;
			}
			"max_results" => {
				settings.max_results = Some(parse_usize(value, "max_results")?);
			}
			"max_bytes" => {
				settings.max_bytes = Some(parse_usize(value, "max_bytes")?);
			}
			_ => return Err(anyhow!("unknown policy key: {}", key)),
		}
	}
	let cwd = std::env::current_dir().unwrap_or_else(|_| config.default_root.clone());
	for policy_root in policy_roots {
		if policy_root.default.is_some() {
			return Err(anyhow!("policy roots must not include default"));
		}
		let normalized = normalize_root_path(&policy_root.path, &cwd);
		let (index, _) = roots.iter()
			.enumerate()
			.find(|(_, root)| root.path_canon == normalized || root.path == normalized)
			.ok_or_else(|| anyhow!("policy root not found: {}", policy_root.path))?;
		if let Some(blocked) = policy_root.blocked {
			if blocked {
				if roots[index].default {
					return Err(anyhow!("policy cannot block the default root"));
				}
				roots[index].blocked = true;
			}
		}
		if !policy_root.deny.is_empty() {
			roots[index].deny.extend(policy_root.deny);
			roots[index].deny_set = build_glob_set(&roots[index].deny)?;
		}
		if !policy_root.allow.is_empty() {
			roots[index].allow.extend(policy_root.allow);
			roots[index].allow_set = build_glob_set(&roots[index].allow)?;
		}
	}
	Ok(settings)
}

pub fn build_glob_set(patterns: &[String]) -> Result<Option<GlobSet>> {
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

fn parse_usize(value: &Value, name: &str) -> Result<usize> {
	let num = value.as_u64().ok_or_else(|| anyhow!("{} must be an integer", name))?;
	usize::try_from(num).map_err(|_| anyhow!("{} out of range", name))
}
