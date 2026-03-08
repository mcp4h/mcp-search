use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::{anyhow, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use tokio::sync::mpsc;
use crate::indexer::{delete_file, index_file};
use crate::parser::LanguageRegistry;
use crate::store::VectorStore;
use crate::embedder::Embedder;
use crate::logger;

pub struct WatcherHandle {
	pub _task: tokio::task::JoinHandle<()>,
	pub _watcher: RecommendedWatcher,
}

pub async fn start_watcher(
	roots: Vec<PathBuf>,
	debounce_ms: u64,
	max_tokens: usize,
	overlap_tokens: usize,
	languages: Arc<LanguageRegistry>,
	embedder: Arc<dyn Embedder>,
	store: Arc<dyn VectorStore>) -> Result<WatcherHandle> {
	let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
	let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
		if let Ok(event) = res {
			let _ = tx.send(event);
		}
	})
		.map_err(|err| anyhow!("failed to create watcher: {}", err))?;
	for root in &roots {
		watcher.watch(root, RecursiveMode::Recursive)
			.map_err(|err| anyhow!("failed to watch {}: {}", root.display(), err))?;
		logger::info(format!("watching root: {}", root.display()));
	}
	let task = tokio::spawn(
		async move {
			let mut last_seen: HashMap<PathBuf, Instant> = HashMap::new();
			let debounce = Duration::from_millis(debounce_ms);
			while let Some(event) = rx.recv().await {
				for path in event.paths {
					if path.is_dir() {
						continue;
					}
					if let Some(last) = last_seen.get(&path) {
						if last.elapsed() < debounce {
							continue;
						}
					}
					last_seen.insert(path.clone(), Instant::now());
					match event.kind {
						EventKind::Remove(_) => {
							logger::info(format!("removed file: {}", path.display()));
							let store = Arc::clone(&store);
							if let Err(err) = delete_file(path.clone(), store).await {
								logger::error(format!("failed to delete {}: {}", path.display(), err));
							}
						}
						EventKind::Modify(_) | EventKind::Create(_) => {
							logger::info(format!("changed file: {}", path.display()));
							let languages = Arc::clone(&languages);
							let embedder = Arc::clone(&embedder);
							let store = Arc::clone(&store);
							if let Err(err) = index_file(
								path.clone(),
								languages,
								embedder,
								store,
								max_tokens,
								overlap_tokens
							).await {
								logger::error(format!("failed to reindex {}: {}", path.display(), err));
							}
						}
						_ => {}
					}
				}
			}
		}
	);
	Ok(WatcherHandle { _task: task, _watcher: watcher })
}
