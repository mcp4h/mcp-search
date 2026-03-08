# MCP Search Server Design

This document captures the initial design for a Rust-based MCP server focused on search with Tree-sitter driven structural chunking and pluggable embeddings.

## Goals

- Build an MCP server for search with AST-aware parsing via Tree-sitter.
- Use Tree-sitter queries to identify structural elements.
- Recursively chunk structural elements until each chunk fits embedding limits.
- Attach context to each embedding (file path, line ranges, node kind, parent context).
- Support file watching and automatic re-embedding for changed files.
- Provide pluggable embedding providers (default fastembed; optional Ollama, OpenAI, etc.).
- Local embeddings are provided via fastembed; default model is `bge-small-en-v1.5`.
- Use LanceDB for vector storage and similarity search.

## Build prerequisites

- `protoc` (protobuf compiler) is required to build LanceDB dependencies.
  - Debian/Ubuntu: `sudo apt-get install protobuf-compiler`

## Configuration Model

### Live instance configuration (SEP-1596)

The server follows the same configuration flow as `../mcp-fs`:

- `initialize` returns `configSchema`.
- The live config instance is sent via `capabilities.experimental.configuration`.
- No file-based instance configuration is used.

Live config schema includes:

- `roots`: directories to index/watch.
- `chunk_max_chars` / `chunk_max_tokens`: size limits for chunking.
- `watch_debounce_ms`: debounce for file change events.
- `embedder`: provider selection and provider-specific settings.
- `embedder.concurrency`: max concurrent embedding jobs (default: `1`).
- `lancedb.path`: local directory for the embedded database.
- `language_pack_root`: path to language packs; defaults to `./languages` when not configured.
- `languages`: optional list of language names to enable; omit to enable all known languages.
- `context`: logical instance context for data isolation (default: `default`).

### Per-call policy restrictions

The server adopts the same dynamic policy model used in `mcp-fs`:

- Tool calls can include `_meta.policy` to restrict search scope.
- Policies can restrict *where to search* using roots and allow/deny globs.
- Policies apply to search results and query filtering only.
- Policies do not steer indexing or refresh operations.

Additional policy restrictions (non-glob):

- `languages` / `deny_languages`: allow/deny by language name.
- `max_results`: cap the number of returned matches.
- `max_bytes`: cap total returned text bytes.

Policy-wide allow/deny globs (in addition to per-root allow/deny) are supported via `allow` and `deny` fields in `_meta.policy`.

## Language Packs

Language packs are folders under a configurable `language_pack_root`:

- `languages/<lang>/language.toml`
- `languages/<lang>/queries.scm`
- `languages/<lang>/tree-sitter-<lang>-<os>-<arch>{ext}`

Example binary name from releases:

- `tree-sitter-typescript-linux-x86_64.so`

### `language.toml` schema (language-only)

```toml
name = "typescript"
extensions = ["ts", "tsx"]
parser = "tree-sitter-typescript-{os}-{arch}{ext}"
custom = false

[queries]
structural = "queries.scm"
```

Placeholder resolution:

- `{os}`: `linux`, `darwin`, or `windows`.
- `{arch}`: `x86_64`, `aarch64`, etc.
- `{ext}`: `.so`, `.dylib`, or `.dll`.

If `custom = true`, the server will not overwrite `queries.scm`. When `custom = false` (default), `queries.scm` is overwritten on startup with the bundled default for that language.

On startup, the server syncs enabled language packs by:

- Creating missing `language.toml` and `queries.scm` files.
- Overwriting `queries.scm` when `custom = false`.
- Downloading the latest Tree-sitter parser binary from the neatify-tech release if missing.

## Parsing and Structural Chunking

- Detect language by file extension.
- Load Tree-sitter parser binary and structural query from the language pack.
- Extract structural nodes (e.g., modules, classes, functions, impl blocks).
- Recursively descend into child structural nodes if a node’s text exceeds size limits.
- Fall back to line/paragraph chunking when no structural nodes exist.

Token sizing uses a lightweight heuristic (approx. 1 token per 4 characters).

## Change detection

- Always hash file contents (XXH3 128-bit) before embedding.
- Re-embed only when the content hash changes.
- Full scans remove embeddings for files no longer present.

Each chunk includes metadata:

- `file_path`
- `language`
- `node_kind`
- `start_line`, `end_line`
- `parent_context` (e.g., `impl Foo::bar`)

Chunk content is prefixed with metadata headers (FILE, LANG, KIND, LINES, PATH when available).

## Embedding Providers

Embedders are pluggable via a trait-based interface:

- Default: local BGE model.
- Optional: Ollama, OpenAI, other providers.

Provider selection and configuration are supplied via the live config schema.

Concurrency defaults to `1` to be conservative (e.g., local BGE can be internally multithreaded). It is configurable via `embedder.concurrency`.

## Storage (LanceDB)

- LanceDB is used as the embedded vector store.
- The store path is derived as `lancedb.path/<context>` to isolate instances.
- Table schema includes:
  - `chunk_id`, `file_path`, `start_line`, `end_line`, `node_kind`, `parent_context`, `text`, `embedding`
- On file change: delete rows for that file, then insert new chunks.
- Query: embed the query text, run vector search, then apply policy filters.

## File Watching

- Watch roots from live config.
- Debounce changes.
- On modify: re-index file only.
- On delete: remove file’s chunks from LanceDB.

## MCP Tools (initial)

We expect a single tool such as `search` (name TBD) that:

- Accepts a query string.
- Accepts optional `_meta.policy` restrictions to narrow search scope.
- Returns ranked results with chunk text and context metadata.

Indexing and refresh are automatic and not directly steered by policy.
