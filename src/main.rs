mod chunker;
mod config;
mod embedder;
mod indexer;
mod langpacks;
mod logger;
mod parser;
mod policy;
mod protocol;
mod search;
mod server;
mod store;
mod types;
mod watcher;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	server::run().await
}
