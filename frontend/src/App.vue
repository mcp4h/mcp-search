<template>
	<div class="app-shell">
		<header class="hero">
			<div class="hero-title">
				<p class="kicker">MCP Search</p>
				<h1>Search</h1>
				<p class="subtitle">Find the most relevant chunks and inspect what the model sees.</p>
			</div>
			<div class="hero-actions">
				<label class="search-field">
					<span>Query</span>
					<input
						v-model="query"
						type="search"
						placeholder="Search across indexed files"
						@input="queueSearch"/>
				</label>
				<div class="field">
					<span>Limit</span>
					<input
						v-model.number="limit"
						type="number"
						min="1"
						max="50"/>
				</div>
				<button
					class="action"
					:type="'button'"
					:disabled="loading || !query.trim()"
					@click="runSearch">
					Search
				</button>
			</div>
		</header>
		<div class="status-bar">
			<span v-if="loading">Searching...</span>
			<span v-else-if="error">{{ error }}</span>
			<span v-else>{{ summary }}</span>
		</div>
		<main class="workspace">
			<section class="pane pane-results">
				<div class="pane-header">
					<h2>Matches</h2>
					<span>{{ matches.length }} results</span>
				</div>
				<div class="pane-body">
					<div v-if="!matches.length && !loading" class="empty">
						Type a query to explore indexed content.
					</div>
					<button
						v-for="match in matches"
						:key="match.id"
						class="result-card"
						:type="'button'"
						:class="{ selected: selectedMatch?.id === match.id }"
						@click="selectMatch(match)">
						<div class="result-head">
							<span class="score">{{ formatScore(match.score) }}</span>
							<span class="kind">{{ match.metadata.node_kind }}</span>
						</div>
						<div class="result-path">{{ match.metadata.file_path }}</div>
						<div class="result-meta">
							<span>Lines {{ match.metadata.start_line }}-{{ match.metadata.end_line }}</span>
							<span v-if="match.metadata.parent_context">{{ match.metadata.parent_context }}</span>
						</div>
						<p class="result-preview">{{ previewSnippet(match.text) }}</p>
					</button>
				</div>
			</section>
			<section class="pane pane-detail">
				<div class="pane-header">
					<h2>Embedding Payload</h2>
					<button
						class="action subtle"
						:type="'button'"
						:disabled="!selectedMatch"
						@click="copyEmbedding">{{ copyLabel }}</button>
				</div>
				<div class="pane-body">
					<div v-if="!selectedMatch" class="empty">
						Select a match to inspect the exact embedding input.
					</div>
					<pre v-else class="payload">{{ selectedMatch.text }}</pre>
				</div>
			</section>
		</main>
	</div>
</template>
<script setup lang="ts">
	import { computed, ref } from "vue";
	type SearchMatch = {
		id: string;
		score: number;
		text: string;
		metadata: {
						file_path: string;
						language: string;
						node_kind: string;
						start_line: number;
						end_line: number;
						parent_context?: string | null;
					};
	};
	const query = ref("");
	const limit = ref(10);
	const matches = ref<SearchMatch[]>([]);
	const loading = ref(false);
	const error = ref("");
	const selectedMatch = ref<SearchMatch | null>(null);
	const copyLabel = ref("Copy");
	let searchTimer: number | undefined;
	const summary = computed(
		() => {
			if (!query.value.trim()) return "Ready to search.";
			return `${matches.value.length} matches for "${query.value.trim()}"`;
		}
	);
	function ensureMcp() {
		const mcp = (window as unknown as { mcp?: { callTool?: any } }).mcp;
		if (!mcp?.callTool) {
			throw new Error("mcp.callTool unavailable");
		}
		return mcp.callTool.bind(mcp);
	}
	async function callTool(name: string, args: Record<string, unknown> = {}, meta?: Record<string, unknown>) {
		const call = ensureMcp();
		const payload: { name: string; arguments?: Record<string, unknown>; _meta?: Record<string, unknown> } = { name, arguments: args };
		if (meta) {
			payload._meta = meta;
		}
		console.debug("[mcp-ui] tool call", payload);
		const result = await call(payload);
		console.debug("[mcp-ui] tool result", name, result);
		return result;
	}
	async function runSearch() {
		const term = query.value.trim();
		if (!term) {
			matches.value = [];
			selectedMatch.value = null;
			return;
		}
		loading.value = true;
		error.value = "";
		try {
			const result = await callTool("semantic_search", { query: term, limit: limit.value });
			const structured = result?.structuredContent ?? result;
			const rawMatches = (structured?.matches || []) as Array<any>;
			matches.value = rawMatches.map(
				(match, index) => ({
					id: `${match?.metadata?.file_path || "match"}-${index}`,
					score: Number(match?.score ?? 0),
					text: String(match?.text ?? ""),
					metadata: {
						file_path: String(match?.metadata?.file_path ?? ""),
						language: String(match?.metadata?.language ?? ""),
						node_kind: String(match?.metadata?.node_kind ?? ""),
						start_line: Number(match?.metadata?.start_line ?? 0),
						end_line: Number(match?.metadata?.end_line ?? 0),
						parent_context: match?.metadata?.parent_context ?? null
					}
				})
			);
			selectedMatch.value = matches.value[0] ?? null;
		}
		catch (err) {
			error.value = err instanceof Error ? err.message : "Search failed";
			matches.value = [];
			selectedMatch.value = null;
		}
		finally {
			loading.value = false;
		}
	}
	function queueSearch() {
		if (searchTimer) window.clearTimeout(searchTimer);
		searchTimer = window.setTimeout(() => runSearch(), 300);
	}
	function selectMatch(match: SearchMatch) {
		selectedMatch.value = match;
		copyLabel.value = "Copy";
	}
	function formatScore(score: number) {
		return score.toFixed(4);
	}
	function previewSnippet(text: string) {
		const clean = text.replace(/\s+/g, " ").trim();
		return clean.length > 140 ? `${clean.slice(0, 140)}...` : clean;
	}
	async function copyEmbedding() {
		if (!selectedMatch.value) return;
		copyLabel.value = "Copying...";
		try {
			await navigator.clipboard.writeText(selectedMatch.value.text);
			copyLabel.value = "Copied";
			window.setTimeout(() => (copyLabel.value = "Copy"), 1200);
		}
		catch (err) {
			copyLabel.value = "Copy";
		}
	}
</script>
