use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use anyhow::{anyhow, Result};
use chrono::Local;

static LOGGER: OnceLock<Logger> = OnceLock::new();

#[derive(Debug)]
struct Logger {
	state: Mutex<LoggerState>,
}

#[derive(Debug)]
struct LoggerState {
	dir: PathBuf,
	base_name: String,
	date: String,
	file: File,
}

pub fn configure(log_path: Option<&PathBuf>, cwd: &PathBuf) -> Result<()> {
	let (dir, base_name) = resolve_log_target(log_path, cwd)?;
	let date = today();
	let file = open_log_file(&dir, &base_name, &date)?;
	let logger = LOGGER.get_or_init(
		|| Logger {
			state: Mutex::new(LoggerState {
				dir: dir.clone(),
				base_name: base_name.clone(),
				date: date.clone(),
				file
			})
		}
	);
	let mut state = logger.state
		.lock()
		.map_err(|_| anyhow!("logger lock poisoned"))?;
	state.dir = dir;
	state.base_name = base_name;
	state.date = date;
	state.file = open_log_file(&state.dir, &state.base_name, &state.date)?;
	Ok(())
}

pub fn info(message: impl AsRef<str>) {
	write_log("INFO", message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
	write_log("WARN", message.as_ref());
}

pub fn error(message: impl AsRef<str>) {
	write_log("ERROR", message.as_ref());
}

fn write_log(level: &str, message: &str) {
	let Some(logger) = LOGGER.get() else {
		return;
	};
	let mut state = match logger.state.lock() {
		Ok(state) => state,
		Err(_) => return,
	};
	let now_date = today();
	if state.date != now_date {
		state.date = now_date;
		if let Ok(file) = open_log_file(&state.dir, &state.base_name, &state.date) {
			state.file = file;
		}
	}
	let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
	let line = format!("{} [{}] {}\n", timestamp, level, message);
	let _ = state.file.write_all(line.as_bytes());
}

fn resolve_log_target(log_path: Option<&PathBuf>, cwd: &PathBuf) -> Result<(PathBuf, String)> {
	let default_base = "mcp-search".to_string();
	let Some(path) = log_path else {
		return Ok((cwd.clone(), default_base));
	};
	let resolved = if path.is_absolute() {
		path.clone()
	}
	else {
		cwd.join(path)
	};
	if resolved.exists() && resolved.is_dir() {
		return Ok((resolved, default_base));
	}
	if resolved.extension()
		.map(|ext| ext == "log")
		.unwrap_or(false) {
		let dir = resolved.parent()
			.unwrap_or(cwd)
			.to_path_buf();
		let stem = resolved.file_stem()
			.and_then(|name| name.to_str())
			.unwrap_or("mcp-search")
			.to_string();
		return Ok((dir, stem));
	}
	Ok((resolved, default_base))
}

fn open_log_file(dir: &Path, base_name: &str, date: &str) -> Result<File> {
	fs::create_dir_all(dir).map_err(|err| anyhow!("failed to create log dir {}: {}", dir.display(), err))?;
	let filename = format!("{}-{}.log", base_name, date);
	let path = dir.join(filename);
	OpenOptions::new()
		.create(true)
		.append(true)
		.open(&path)
		.map_err(|err| anyhow!("failed to open log file {}: {}", path.display(), err))
}

fn today() -> String {
	Local::now().format("%Y-%m-%d").to_string()
}
