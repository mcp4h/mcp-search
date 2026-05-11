# mcp-search

MCP server for semantic code search. It indexes files into a vector store and exposes a `semantic_search` tool plus a small web UI for inspecting matches and embedding payloads.

## Features
- MCP tool: `semantic_search` (query embedding + vector search)
- Background indexing + file watching
- Tree-sitter structural chunking with overlap
- LanceDB storage
- Web UI bundled into `assets/ui/search`

## Configuration
The server reads configuration from MCP initialize `capabilities.experimental.configuration`.

Common settings:
- `roots`: array of root objects `{ path, allow, deny, default }`
- `chunk_max_tokens`: max tokens per chunk (default 300)
- `chunk_overlap_tokens`: token overlap between chunks (default 50)
- `data_root`: base directory for language packs + LanceDB (default `.mcp-search`)
- `language_pack_root`: directory for language packs (default `.mcp-search/languages`)
- `language_download`: allow downloading parser binaries (default `true`)
- `lancedb.path`: directory for LanceDB data (default `.mcp-search/lancedb`)
- `log_path`: optional log directory or base `.log` path; defaults to CWD

`data_root` sets both `language_pack_root` and `lancedb.path` unless you override them explicitly.

Content ingestion:
- `content/update`: `{ id, language, content, metadata }` to upsert embeddings for external content
- `content/delete`: `{ id }` to remove all embeddings for an external id
- `content/list`: returns `{ ids }` for the stored external ids
- When `roots` is empty, file indexing is disabled but content ingestion remains available.

## Development
Build the server:
```
cargo build --release
```

Build the UI:
```
cd frontend
npm install
npm run build
```

## Notes
- The server runs over stdio; logs are written to a file.
- Query embeddings are performed on tool calls; indexing runs in background tasks.
