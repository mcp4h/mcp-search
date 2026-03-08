use crate::types::{Chunk, ChunkMetadata};
use std::collections::HashSet;
use tree_sitter::{Node, Query, QueryCursor, StreamingIterator, Tree};

pub fn fallback_chunk(
	file_path: &std::path::Path,
	language: &str,
	text: &str,
	max_tokens: usize,
	overlap_tokens: usize) -> Vec<Chunk> {
	let token_len = estimate_tokens_from_len(text.len());
	if max_tokens == 0 || token_len <= max_tokens {
		let span = ChunkSpan {
			start_byte: 0,
			end_byte: text.len(),
			start_line: 1,
			end_line: text.lines()
				.count()
				.max(1),
			node_kind: "file".to_string()
		};
		let header = build_chunk_header(
			file_path,
			language,
			&span,
			None
		);
		let text = format!("{}{}", header, text);
		return vec![Chunk {
			id: uuid::Uuid::new_v4().to_string(),
			text,
			metadata: ChunkMetadata {
                file_path: file_path.to_path_buf(),
                language: language.to_string(),
                node_kind: "file".to_string(),
                start_line: 1,
                end_line: span.end_line,
                parent_context: None,
            },
		}];
	}
	let spans = fallback_chunk_spans(
		text,
		0,
		text.len(),
		1,
		max_tokens,
		"file"
	);
	let spans = apply_overlap(
		spans,
		text,
		overlap_tokens,
		max_tokens
	);
	spans.into_iter()
		.map(
			|span| {
				let mut text = slice_source(text, span.start_byte, span.end_byte);
				let header = build_chunk_header(
					file_path,
					language,
					&span,
					None
				);
				text = format!("{}{}", header, text);
				Chunk {
					id: uuid::Uuid::new_v4().to_string(),
					text,
					metadata: ChunkMetadata {
						file_path: file_path.to_path_buf(),
						language: language.to_string(),
						node_kind: span.node_kind,
						start_line: span.start_line,
						end_line: span.end_line,
						parent_context: None
					}
				}
			})
		.collect()
}

#[derive(Clone, Debug)]
struct NodeSpan {
	start_byte: usize,
	end_byte: usize,
	start_line: usize,
	end_line: usize,
	kind: String,
	children: Vec<usize>,
	parent: Option<usize>,
}

#[derive(Clone, Debug)]
struct ChunkSpan {
	start_byte: usize,
	end_byte: usize,
	start_line: usize,
	end_line: usize,
	node_kind: String,
}

pub fn structural_chunks(
	file_path: &std::path::Path,
	language: &str,
	source: &str,
	tree: &Tree,
	query: &Query,
	max_tokens: usize,
	overlap_tokens: usize) -> Vec<Chunk> {
	let nodes = extract_nodes(source, tree, query);
	if nodes.is_empty() {
		return fallback_chunk(
			file_path,
			language,
			source,
			max_tokens,
			overlap_tokens
		);
	}
	let roots: Vec<usize> = nodes.iter()
		.enumerate()
		.filter(|(_, node)| node.parent.is_none())
		.map(|(index, _)| index)
		.collect();
	let mut spans = Vec::new();
	for root in roots {
		let mut chunk_spans = chunk_node(
			root,
			&nodes,
			source,
			max_tokens
		);
		spans.append(&mut chunk_spans);
	}
	spans.sort_by_key(|span| span.start_byte);
	let spans = prune_contained_spans(spans);
	let spans = apply_overlap(
		spans,
		source,
		overlap_tokens,
		max_tokens
	);
	spans.into_iter()
		.map(
			|span| {
				let mut text = slice_source(source, span.start_byte, span.end_byte);
				let context_path = compute_context_path(
					language,
					tree,
					source,
					&span
				);
				let header = build_chunk_header(
					file_path,
					language,
					&span,
					context_path.as_deref()
				);
				text = format!("{}{}", header, text);
				Chunk {
					id: uuid::Uuid::new_v4().to_string(),
					text,
					metadata: ChunkMetadata {
						file_path: file_path.to_path_buf(),
						language: language.to_string(),
						node_kind: span.node_kind,
						start_line: span.start_line,
						end_line: span.end_line,
						parent_context: context_path
					}
				}
			})
		.collect()
}

fn extract_nodes(source: &str, tree: &Tree, query: &Query) -> Vec<NodeSpan> {
	let mut cursor = QueryCursor::new();
	let mut nodes = Vec::new();
	let capture_names = query.capture_names();
	let mut seen = HashSet::new();
	let mut captures = cursor.captures(query, tree.root_node(), source.as_bytes());
	while let Some((match_, capture_index)) = captures.next() {
		let capture = &match_.captures[*capture_index];
		let name = capture_names[capture.index as usize];
		if name != "structural" {
			continue;
		}
		let node = capture.node;
		let start = node.start_byte();
		let end = node.end_byte();
		if !seen.insert((start, end)) {
			continue;
		}
		let (start_line, end_line) = node_lines(&node);
		nodes.push(
			NodeSpan {
				start_byte: start,
				end_byte: end,
				start_line,
				end_line,
				kind: node.kind().to_string(),
				children: Vec::new(),
				parent: None
			}
		);
	}
	if nodes.is_empty() {
		return nodes;
	}
	nodes.sort_by(
		|a, b| {
			let start = a.start_byte.cmp(&b.start_byte);
			if start == std::cmp::Ordering::Equal {
				a.end_byte.cmp(&b.end_byte)
			}
			else {
				start
			}
		}
	);
	let mut stack: Vec<usize> = Vec::new();
	for index in 0..nodes.len() {
		while let Some(&last) = stack.last() {
			if nodes[index].start_byte >= nodes[last].end_byte {
				stack.pop();
			}
			else {
				break;
			}
		}
		if let Some(&parent) = stack.last() {
			if nodes[index].end_byte <= nodes[parent].end_byte {
				nodes[index].parent = Some(parent);
				nodes[parent].children.push(index);
			}
		}
		stack.push(index);
	}
	nodes
}

fn node_lines(node: &Node) -> (usize, usize) {
	let start_line = node.start_position().row + 1;
	let end_line = node.end_position().row + 1;
	(start_line, end_line)
}

fn chunk_node(
	node_index: usize,
	nodes: &[NodeSpan],
	source: &str,
	max_tokens: usize) -> Vec<ChunkSpan> {
	let node = &nodes[node_index];
	let span_len = node.end_byte.saturating_sub(node.start_byte);
	let token_len = estimate_tokens_from_len(span_len);
	if fits_limits(token_len, max_tokens) {
		return vec![ChunkSpan {
			start_byte: node.start_byte,
			end_byte: node.end_byte,
			start_line: node.start_line,
			end_line: node.end_line,
			node_kind: node.kind.clone(),
		}];
	}
	if node.children.is_empty() {
		return fallback_chunk_spans(
			source,
			node.start_byte,
			node.end_byte,
			node.start_line,
			max_tokens,
			&node.kind
		);
	}
	let mut spans = Vec::new();
	for &child in &node.children {
		let mut child_spans = chunk_node(
			child,
			nodes,
			source,
			max_tokens
		);
		spans.append(&mut child_spans);
	}
	spans.sort_by_key(|span| span.start_byte);
	merge_spans(spans, max_tokens, &node.kind)
}

fn merge_spans(spans: Vec<ChunkSpan>, max_tokens: usize, parent_kind: &str) -> Vec<ChunkSpan> {
	let mut merged = Vec::new();
	let mut current: Option<ChunkSpan> = None;
	for span in spans {
		if let Some(mut active) = current.take() {
			let combined_len = span.end_byte.saturating_sub(active.start_byte);
			let combined_tokens = estimate_tokens_from_len(combined_len);
			if fits_limits(combined_tokens, max_tokens) {
				let combined_kind = if active.start_byte == span.start_byte && active.end_byte == span.end_byte {
					active.node_kind.clone()
				}
				else {
					parent_kind.to_string()
				};
				active.end_byte = span.end_byte;
				active.end_line = span.end_line;
				active.node_kind = combined_kind;
				current = Some(active);
			}
			else {
				merged.push(active);
				current = Some(span);
			}
		}
		else {
			current = Some(span);
		}
	}
	if let Some(span) = current {
		merged.push(span);
	}
	merged
}

fn fallback_chunk_spans(
	source: &str,
	start_byte: usize,
	end_byte: usize,
	start_line: usize,
	max_tokens: usize,
	default_kind: &str) -> Vec<ChunkSpan> {
	let slice = slice_source(source, start_byte, end_byte);
	let slice_len = slice.len();
	let slice_tokens = estimate_tokens_from_len(slice_len);
	if fits_limits(slice_tokens, max_tokens) {
		return vec![ChunkSpan {
			start_byte,
			end_byte,
			start_line,
			end_line: start_line + slice.lines().count().saturating_sub(1),
			node_kind: default_kind.to_string(),
		}];
	}
	let mut spans = Vec::new();
	let mut current = String::new();
	let mut current_tokens = 0usize;
	let mut line_index = 0usize;
	let mut chunk_start_line = start_line;
	let mut byte_cursor = start_byte;
	for line in slice.lines() {
		line_index += 1;
		let line_len = line.len() + 1;
		let line_tokens = estimate_tokens_from_len(line_len);
		let next_tokens = current_tokens + line_tokens;
		if !current.is_empty() && !fits_limits(next_tokens, max_tokens) {
			let chunk_end_line = chunk_start_line + line_index - 2;
			let chunk_end_byte = byte_cursor;
			spans.push(
				ChunkSpan {
					start_byte: byte_cursor - current.len(),
					end_byte: chunk_end_byte,
					start_line: chunk_start_line,
					end_line: chunk_end_line,
					node_kind: default_kind.to_string()
				}
			);
			current.clear();
			current_tokens = 0;
			chunk_start_line = start_line + line_index - 1;
		}
		current.push_str(line);
		current.push('\n');
		current_tokens += line_tokens;
		byte_cursor += line_len;
	}
	if !current.is_empty() {
		let chunk_end_line = start_line + line_index - 1;
		spans.push(
			ChunkSpan {
				start_byte: end_byte.saturating_sub(current.len()),
				end_byte,
				start_line: chunk_start_line,
				end_line: chunk_end_line,
				node_kind: default_kind.to_string()
			}
		);
	}
	spans
}

fn slice_source(source: &str, start: usize, end: usize) -> String {
	let start = start.min(source.len());
	let end = end.min(source.len());
	if start >= end {
		return String::new();
	}
	source[start..end].to_string()
}

fn fits_limits(tokens: usize, max_tokens: usize) -> bool {
	max_tokens == 0 || tokens <= max_tokens
}

fn prune_contained_spans(spans: Vec<ChunkSpan>) -> Vec<ChunkSpan> {
	if spans.len() < 2 {
		return spans;
	}
	let mut sorted = spans;
	sorted.sort_by(
		|a, b| {
			let a_len = a.end_byte.saturating_sub(a.start_byte);
			let b_len = b.end_byte.saturating_sub(b.start_byte);
			b_len.cmp(&a_len).then_with(|| a.start_byte.cmp(&b.start_byte))
		}
	);
	let mut kept: Vec<ChunkSpan> = Vec::new();
	for span in sorted {
		if kept.iter().any(|other| span_contained_by(&span, other)) {
			continue;
		}
		kept.push(span);
	}
	kept.sort_by_key(|span| span.start_byte);
	kept
}

fn span_contained_by(inner: &ChunkSpan, outer: &ChunkSpan) -> bool {
	outer.start_byte <= inner.start_byte && outer.end_byte >= inner.end_byte
}

fn apply_overlap(
	spans: Vec<ChunkSpan>,
	source: &str,
	overlap_tokens: usize,
	max_tokens: usize) -> Vec<ChunkSpan> {
	if spans.len() < 2 || overlap_tokens == 0 {
		return spans;
	}
	let effective_overlap = if max_tokens == 0 {
		overlap_tokens
	}
	else {
		overlap_tokens.min(max_tokens.saturating_sub(1))
	};
	if effective_overlap == 0 {
		return spans;
	}
	let overlap_bytes = effective_overlap.saturating_mul(4);
	let mut out = Vec::with_capacity(spans.len());
	for (index, span) in spans.into_iter().enumerate() {
		if index == 0 {
			out.push(span);
			continue;
		}
		let new_start = span.start_byte.saturating_sub(overlap_bytes);
		if new_start == span.start_byte {
			out.push(span);
			continue;
		}
		let adjusted_start = line_start_for_byte(source, new_start);
		let start_line = line_for_byte(source, adjusted_start);
		out.push(ChunkSpan {
			start_byte: adjusted_start,
			start_line,
			..span
		});
	}
	out
}

fn line_for_byte(source: &str, byte: usize) -> usize {
	let end = byte.min(source.len());
	let mut count = 1usize;
	for b in source.as_bytes()[..end].iter() {
		if *b == b'\n' {
			count += 1;
		}
	}
	count
}

fn line_start_for_byte(source: &str, byte: usize) -> usize {
	let end = byte.min(source.len());
	let prefix = &source[..end];
	match prefix.rfind('\n') {
		Some(index) => index + 1,
		None => 0,
	}
}

fn estimate_tokens_from_len(len: usize) -> usize {
	(len + 3) / 4
}

fn build_chunk_header(
	file_path: &std::path::Path,
	language: &str,
	span: &ChunkSpan,
	context: Option<&str>) -> String {
	let mut header = String::new();
	header.push_str("FILE: ");
	header.push_str(&file_path.display().to_string());
	header.push('\n');
	header.push_str("LANG: ");
	header.push_str(language);
	header.push('\n');
	header.push_str("KIND: ");
	header.push_str(&span.node_kind);
	header.push('\n');
	header.push_str("LINES: ");
	header.push_str(&format!("{}-{}", span.start_line, span.end_line));
	header.push('\n');
	if let Some(path) = context {
		header.push_str("PATH: ");
		header.push_str(path);
		header.push('\n');
	}
	header.push('\n');
	header
}

fn compute_context_path(
	language: &str,
	tree: &Tree,
	source: &str,
	span: &ChunkSpan) -> Option<String> {
	match language {
		"xml" => compute_xml_path(tree, source, span),
		"json" => compute_json_path(tree, source, span),
		_ => None,
	}
}

fn compute_xml_path(tree: &Tree, source: &str, span: &ChunkSpan) -> Option<String> {
	let mut node = tree.root_node().descendant_for_byte_range(span.start_byte, span.end_byte)?;
	while node.start_byte() > span.start_byte || node.end_byte() < span.end_byte {
		if let Some(parent) = node.parent() {
			node = parent;
		}
		else {
			break;
		}
	}
	let mut parts = Vec::new();
	let mut current = Some(node);
	while let Some(node) = current {
		if is_xml_element(&node) {
			if let Some(name) = xml_element_name(&node, source) {
				parts.push(name);
			}
		}
		current = node.parent();
	}
	if parts.is_empty() {
		return None;
	}
	parts.reverse();
	Some(parts.join("/"))
}

fn is_xml_element(node: &Node) -> bool {
	matches!(node.kind(), "element" | "empty_element" | "start_tag")
}

fn xml_element_name(node: &Node, source: &str) -> Option<String> {
	if let Some(name) = node.child_by_field_name("name") {
		return Some(node_text(&name, source));
	}
	for i in 0..node.child_count() {
		if let Some(child) = node.child(i as u32) {
			let kind = child.kind();
			if kind == "name" || kind == "tag_name" || kind == "qualified_name" {
				return Some(node_text(&child, source));
			}
		}
	}
	None
}

fn compute_json_path(tree: &Tree, source: &str, span: &ChunkSpan) -> Option<String> {
	let mut node = tree.root_node().descendant_for_byte_range(span.start_byte, span.end_byte)?;
	while node.start_byte() > span.start_byte || node.end_byte() < span.end_byte {
		if let Some(parent) = node.parent() {
			node = parent;
		}
		else {
			break;
		}
	}
	let mut parts = Vec::new();
	let mut current = Some(node);
	let mut prev = None;
	while let Some(node) = current {
		match node.kind() {
			"pair" => {
				if let Some(key) = json_pair_key(&node, source) {
					parts.push(key);
				}
			}
			"array" => {
				if let Some(child) = prev {
					if let Some(index) = json_array_index_for_child(&node, &child) {
						parts.push(format!("[{}]", index));
					}
				}
			}
			_ => {}
		}
		prev = Some(node);
		current = node.parent();
	}
	if parts.is_empty() {
		return None;
	}
	parts.reverse();
	Some(parts.join("."))
}

fn json_pair_key(node: &Node, source: &str) -> Option<String> {
	if let Some(key) = node.child_by_field_name("key") {
		return Some(trim_json_string(&node_text(&key, source)));
	}
	for i in 0..node.child_count() {
		if let Some(child) = node.child(i as u32) {
			if child.kind() == "string" {
				return Some(trim_json_string(&node_text(&child, source)));
			}
		}
	}
	None
}

fn json_array_index_for_child(array_node: &Node, child_node: &Node) -> Option<usize> {
	if array_node.kind() != "array" {
		return None;
	}
	let mut index = 0usize;
	for i in 0..array_node.child_count() {
		let Some(child) = array_node.child(i as u32) else {
			continue;
		};
		if !is_json_value(&child) {
			continue;
		}
		if child.start_byte() == child_node.start_byte()
			&& child.end_byte() == child_node.end_byte() {
			return Some(index);
		}
		index += 1;
	}
	None
}

fn is_json_value(node: &Node) -> bool {
	matches!(
		node.kind(),
		"object" | "array" | "string" | "number" | "true" | "false" | "null"
	)
}

fn trim_json_string(value: &str) -> String {
	let trimmed = value.trim();
	if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
		trimmed[1..trimmed.len() - 1].to_string()
	}
	else {
		trimmed.to_string()
	}
}

fn node_text(node: &Node, source: &str) -> String {
	let start = node.start_byte();
	let end = node.end_byte();
	slice_source(source, start, end)
}

#[cfg(test)]
mod tests {
	use *;
	#[test]
	fn token_estimate_rounds_up() {
		assert_eq!(estimate_tokens_from_len(0), 0);
		assert_eq!(estimate_tokens_from_len(1), 1);
		assert_eq!(estimate_tokens_from_len(4), 1);
		assert_eq!(estimate_tokens_from_len(5), 2);
		assert_eq!(estimate_tokens_from_len(8), 2);
	}
	#[test]
	fn fits_limits_checks_tokens() {
		assert!(fits_limits(3, 0));
		assert!(fits_limits(3, 4));
		assert!(!fits_limits(3, 2));
	}
}
