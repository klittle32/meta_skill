//! CASS JSON export decoder.
//!
//! Converts each record from `cass export --format json --include-tools` into
//! [`SessionMessage`] values. Records are dispatched independently so a single
//! export may mix normalized flat messages, raw Claude Code conversation
//! records, raw Codex `response_item` records, and provider noise.

use serde_json::{Map, Value};

use super::client::{SessionMessage, ToolCall, ToolResult};

/// Errors produced while decoding a CASS JSON export array.
#[derive(Debug, thiserror::Error)]
pub enum CassExportDecodeError {
    #[error("invalid cass export JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("cass export contained {record_count} records but no supported conversation messages")]
    NoSupportedMessages { record_count: usize },
}

/// Decode a CASS JSON export body into session messages.
pub fn decode_cass_export(
    bytes: &[u8],
) -> std::result::Result<Vec<SessionMessage>, CassExportDecodeError> {
    let records: Vec<Value> = serde_json::from_slice(bytes)?;
    let record_count = records.len();
    let mut messages = Vec::new();
    for (raw_index, record) in records.iter().enumerate() {
        if let Some(message) = decode_record(record, raw_index)? {
            messages.push(message);
        }
    }
    if !messages.iter().any(is_material_message) {
        return Err(CassExportDecodeError::NoSupportedMessages { record_count });
    }
    for (index, message) in messages.iter_mut().enumerate() {
        message.index = index;
        if message.tool_calls.is_empty() {
            message.tool_calls = parse_inline_tool_markers(&message.content, index);
        }
    }
    Ok(messages)
}

fn decode_record(
    record: &Value,
    raw_index: usize,
) -> std::result::Result<Option<SessionMessage>, CassExportDecodeError> {
    if is_normalized_record(record) {
        return Ok(Some(decode_normalized_record(record)?));
    }
    if is_claude_record(record) {
        return Ok(decode_claude_record(record, raw_index));
    }
    if is_codex_record(record) {
        return Ok(decode_codex_record(record, raw_index));
    }
    Ok(None)
}

fn is_normalized_record(record: &Value) -> bool {
    record
        .as_object()
        .is_some_and(|object| object.contains_key("role") && object.contains_key("content"))
}

fn is_claude_record(record: &Value) -> bool {
    let Some(object) = record.as_object() else {
        return false;
    };
    if !matches!(
        object.get("type").and_then(Value::as_str),
        Some("user" | "assistant")
    ) {
        return false;
    }
    let Some(message) = object.get("message").and_then(Value::as_object) else {
        return false;
    };
    matches!(
        message.get("role").and_then(Value::as_str),
        Some("user" | "assistant")
    ) && message.contains_key("content")
}

fn is_codex_record(record: &Value) -> bool {
    record.get("type").and_then(Value::as_str) == Some("response_item")
        && record.get("payload").is_some_and(Value::is_object)
}

fn decode_normalized_record(
    record: &Value,
) -> std::result::Result<SessionMessage, serde_json::Error> {
    serde_json::from_value(record.clone())
}

fn decode_claude_record(record: &Value, raw_index: usize) -> Option<SessionMessage> {
    let message = record.get("message")?.as_object()?;
    let role = message.get("role")?.as_str()?;
    if !matches!(role, "user" | "assistant") {
        return None;
    }
    let content_value = message.get("content")?;
    let (content, tool_calls, tool_results) = decode_claude_content(content_value, raw_index);
    let decoded = SessionMessage {
        index: 0,
        role: role.to_string(),
        content,
        tool_calls,
        tool_results,
    };
    is_material_message(&decoded).then_some(decoded)
}

fn decode_claude_content(
    content: &Value,
    raw_index: usize,
) -> (String, Vec<ToolCall>, Vec<ToolResult>) {
    match content {
        Value::String(text) => (text.clone(), Vec::new(), Vec::new()),
        Value::Array(blocks) => decode_claude_blocks(blocks, raw_index),
        other => (flatten_text_content(other), Vec::new(), Vec::new()),
    }
}

fn decode_claude_blocks(
    blocks: &[Value],
    raw_index: usize,
) -> (String, Vec<ToolCall>, Vec<ToolResult>) {
    let mut texts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();
    for (block_index, block) in blocks.iter().enumerate() {
        if let Some(text) = block.as_str() {
            if !text.is_empty() {
                texts.push(text.to_string());
            }
            continue;
        }
        let Some(object) = block.as_object() else {
            continue;
        };
        match object.get("type").and_then(Value::as_str).unwrap_or("") {
            "thinking" | "reasoning" => {}
            "tool_use" => tool_calls.push(claude_tool_use(object, raw_index, block_index)),
            "tool_result" => {
                tool_results.push(claude_tool_result(object, raw_index, block_index));
            }
            "text" | "input_text" | "output_text" | "" => {
                if let Some(text) = object.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        texts.push(text.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    (texts.join("\n"), tool_calls, tool_results)
}

fn claude_tool_use(block: &Map<String, Value>, raw_index: usize, block_index: usize) -> ToolCall {
    let id = nonempty_string(block.get("id")).map_or_else(
        || format!("claude_call_{raw_index}_{block_index}"),
        str::to_string,
    );
    let name = nonempty_string(block.get("name"))
        .unwrap_or("unknown")
        .to_string();
    let arguments = match block.get("input") {
        Some(Value::Object(map)) => Value::Object(map.clone()),
        None | Some(Value::Null) => serde_json::json!({}),
        Some(other) => serde_json::json!({ "input": other }),
    };
    ToolCall {
        id,
        name,
        arguments,
    }
}

fn claude_tool_result(
    block: &Map<String, Value>,
    raw_index: usize,
    block_index: usize,
) -> ToolResult {
    let tool_call_id = nonempty_string(block.get("tool_use_id"))
        .or_else(|| nonempty_string(block.get("tool_call_id")))
        .map_or_else(
            || format!("claude_result_{raw_index}_{block_index}"),
            str::to_string,
        );
    let content = block
        .get("content")
        .map_or_else(String::new, flatten_tool_output);
    let is_error = block
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    ToolResult {
        tool_call_id,
        content,
        is_error,
    }
}

fn decode_codex_record(record: &Value, raw_index: usize) -> Option<SessionMessage> {
    let payload = record.get("payload")?.as_object()?;
    let kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "reasoning" => None,
        "message" => decode_codex_message(payload),
        "function_call" => Some(codex_function_call(payload, raw_index)),
        "custom_tool_call" => Some(codex_custom_tool_call(payload, raw_index)),
        other
            if other.ends_with("_output")
                && (payload.contains_key("output") || payload.contains_key("result")) =>
        {
            Some(codex_output(payload, raw_index))
        }
        _ => None,
    }
}

fn decode_codex_message(payload: &Map<String, Value>) -> Option<SessionMessage> {
    let role = match payload.get("role").and_then(Value::as_str)? {
        "user" => "user",
        "assistant" | "agent" => "assistant",
        _ => return None,
    };
    let content = payload
        .get("content")
        .map_or_else(String::new, flatten_text_content);
    let decoded = SessionMessage {
        index: 0,
        role: role.to_string(),
        content,
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
    };
    is_material_message(&decoded).then_some(decoded)
}

fn codex_function_call(payload: &Map<String, Value>, raw_index: usize) -> SessionMessage {
    SessionMessage {
        index: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: first_call_id(payload, format!("codex_call_{raw_index}")),
            name: tool_name(payload),
            arguments: function_call_arguments(payload.get("arguments")),
        }],
        tool_results: Vec::new(),
    }
}

fn codex_custom_tool_call(payload: &Map<String, Value>, raw_index: usize) -> SessionMessage {
    SessionMessage {
        index: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: first_call_id(payload, format!("codex_custom_call_{raw_index}")),
            name: tool_name(payload),
            arguments: custom_tool_arguments(payload.get("input")),
        }],
        tool_results: Vec::new(),
    }
}

fn codex_output(payload: &Map<String, Value>, raw_index: usize) -> SessionMessage {
    let output = payload.get("output").or_else(|| payload.get("result"));
    let content = output.map_or_else(String::new, flatten_tool_output);
    SessionMessage {
        index: 0,
        role: "tool".to_string(),
        content: String::new(),
        tool_calls: Vec::new(),
        tool_results: vec![ToolResult {
            tool_call_id: first_call_id(payload, format!("codex_result_{raw_index}")),
            content,
            is_error: payload_is_error(payload),
        }],
    }
}

fn tool_name(payload: &Map<String, Value>) -> String {
    nonempty_string(payload.get("name"))
        .unwrap_or("unknown")
        .to_string()
}

fn first_call_id(payload: &Map<String, Value>, fallback: String) -> String {
    nonempty_string(payload.get("call_id"))
        .or_else(|| nonempty_string(payload.get("id")))
        .map_or(fallback, str::to_string)
}

fn function_call_arguments(value: Option<&Value>) -> Value {
    match value {
        None | Some(Value::Null) => serde_json::json!({}),
        Some(Value::Object(map)) => Value::Object(map.clone()),
        Some(Value::String(raw)) => parse_json_arguments(raw, "value"),
        Some(other) => serde_json::json!({ "value": other }),
    }
}

fn custom_tool_arguments(value: Option<&Value>) -> Value {
    match value {
        None | Some(Value::Null) => serde_json::json!({}),
        Some(Value::Object(map)) => Value::Object(map.clone()),
        Some(Value::String(raw)) => match serde_json::from_str::<Value>(raw) {
            Ok(Value::Object(map)) => Value::Object(map),
            _ => serde_json::json!({ "input": raw }),
        },
        Some(other) => serde_json::json!({ "input": other }),
    }
}

fn parse_json_arguments(raw: &str, fallback_key: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(parsed) => serde_json::json!({ fallback_key: parsed }),
        Err(_) => serde_json::json!({ "raw": raw }),
    }
}

fn payload_is_error(payload: &Map<String, Value>) -> bool {
    payload
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || payload
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| {
                matches!(status.to_ascii_lowercase().as_str(), "error" | "failed")
            })
}

fn flatten_text_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(text_from_block)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

fn flatten_tool_output(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(text_from_block)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => map.get("content").map_or_else(
            || serde_json::to_string(value).unwrap_or_default(),
            flatten_tool_output,
        ),
        other => other.to_string(),
    }
}

fn text_from_block(block: &Value) -> Option<String> {
    if let Some(text) = block.as_str() {
        return nonempty_owned(text);
    }
    let object = block.as_object()?;
    let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
    if matches!(kind, "thinking" | "reasoning") {
        return None;
    }
    if matches!(kind, "text" | "input_text" | "output_text" | "") {
        return object
            .get("text")
            .and_then(Value::as_str)
            .and_then(nonempty_owned);
    }
    None
}

fn nonempty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn nonempty_owned(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

fn is_material_message(message: &SessionMessage) -> bool {
    !message.content.trim().is_empty()
        || !message.tool_calls.is_empty()
        || !message.tool_results.is_empty()
}

/// Reconstruct structured tool calls from cass inline `[Tool: …]` markers.
///
/// `[Tool: <Name> - <detail>]`, where `<detail>` is a file path for
/// file-oriented tools (`Read`/`Edit`/`Write`/`NotebookEdit`) and a
/// human-readable description for shell tools (`Bash`/`Shell`/…). A single
/// message may contain several markers.
///
/// We map the recovered detail into whichever argument key the miner reads:
/// `command` for shell tools (mining keys command/error patterns and bash
/// phase classification on `arguments.command`) and `file_path` for
/// file-mutating tools (code-change detection / error-resolution steps key on
/// `arguments.file_path`). `msg_index` is woven into a synthetic, unique id so
/// taint/evidence tracking sees distinct calls. Markers are best-effort: the
/// raw shell command is unrecoverable (cass emits only the description), so the
/// reconstruction is necessarily lossy but restores enough structure for the
/// existing extractors to produce patterns again (issue #114).
fn parse_inline_tool_markers(content: &str, msg_index: usize) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let mut rest = content;
    let mut seq = 0usize;

    while let Some(start) = rest.find("[Tool:") {
        let after_prefix = &rest[start + "[Tool:".len()..];
        let Some(end_rel) = after_prefix.find(']') else {
            break;
        };
        let inner = after_prefix[..end_rel].trim();
        rest = &after_prefix[end_rel + 1..];

        if inner.is_empty() {
            continue;
        }

        let (name, detail) = match inner.split_once(" - ") {
            Some((n, d)) => (n.trim(), Some(d.trim())),
            None => (inner, None),
        };
        if name.is_empty() {
            continue;
        }

        let name_lower = name.to_lowercase();
        let arguments = match (name_lower.as_str(), detail) {
            ("bash" | "shell" | "command" | "terminal" | "exec", Some(d)) => {
                serde_json::json!({ "command": d })
            }
            ("read" | "edit" | "write" | "notebookedit" | "multiedit", Some(d)) => {
                serde_json::json!({ "file_path": d })
            }
            (_, Some(d)) => serde_json::json!({ "detail": d }),
            (_, None) => serde_json::json!({}),
        };

        calls.push(ToolCall {
            id: format!("inline_{msg_index}_{seq}"),
            name: name.to_string(),
            arguments,
        });
        seq += 1;
    }

    calls
}

#[cfg(test)]
mod tests {
    use super::{CassExportDecodeError, decode_cass_export, parse_inline_tool_markers};
    use serde_json::{Value, json};

    struct ShapeCase {
        name: &'static str,
        json: serde_json::Value,
        expected_count: usize,
        expected_roles: &'static [&'static str],
        expected_tool_calls: usize,
        expected_tool_results: usize,
        must_contain: &'static [&'static str],
        must_not_contain: &'static [&'static str],
    }

    fn decode_case(case: &ShapeCase) -> Vec<super::SessionMessage> {
        let bytes = serde_json::to_vec(&case.json).expect("serialize fixture");
        decode_cass_export(&bytes).unwrap_or_else(|error| {
            panic!("{} should decode, got {error}", case.name);
        })
    }

    #[test]
    fn decode_supported_export_shapes_table() {
        let cases = [
            ShapeCase {
                name: "normalized_flat",
                json: json!([
                    {
                        "role": "user",
                        "content": "Fix the review-surface validation."
                    },
                    {
                        "role": "assistant",
                        "content": "I will inspect and test it.\n[Tool: Read - src/review_surface.rs]\n[Tool: Bash - cargo test review_surface]"
                    },
                    {
                        "role": null,
                        "content": null
                    }
                ]),
                expected_count: 3,
                expected_roles: &["user", "assistant", ""],
                expected_tool_calls: 2,
                expected_tool_results: 0,
                must_contain: &["I will inspect and test it."],
                must_not_contain: &[],
            },
            ShapeCase {
                name: "raw_claude_code",
                json: json!([
                    {
                        "type": "attachment",
                        "attachment": {"name": "not-a-user-message"}
                    },
                    {
                        "type": "user",
                        "message": {
                            "role": "user",
                            "content": "Fix the review surface."
                        }
                    },
                    {
                        "type": "assistant",
                        "message": {
                            "role": "assistant",
                            "content": [
                                {"type": "thinking", "thinking": "private reasoning"},
                                {"type": "text", "text": "I will inspect the file."},
                                {
                                    "type": "tool_use",
                                    "id": "toolu_read_1",
                                    "name": "Read",
                                    "input": {"file_path": "src/review_surface.rs"}
                                }
                            ]
                        }
                    },
                    {
                        "type": "user",
                        "message": {
                            "role": "user",
                            "content": [
                                {
                                    "type": "tool_result",
                                    "tool_use_id": "toolu_read_1",
                                    "content": "pub fn render_review_surface() {}",
                                    "is_error": false
                                }
                            ]
                        }
                    },
                    {
                        "type": "file-history-snapshot",
                        "message": "noise"
                    }
                ]),
                expected_count: 3,
                expected_roles: &["user", "assistant", "user"],
                expected_tool_calls: 1,
                expected_tool_results: 1,
                must_contain: &["Fix the review surface.", "I will inspect the file."],
                must_not_contain: &["private reasoning"],
            },
            ShapeCase {
                name: "raw_codex_rollout",
                json: json!([
                    {
                        "timestamp": "2026-08-13T12:00:00Z",
                        "type": "session_meta",
                        "payload": {"id": "noise"}
                    },
                    {
                        "timestamp": "2026-08-13T12:00:01Z",
                        "type": "response_item",
                        "payload": {
                            "type": "message",
                            "role": "user",
                            "content": [
                                {"type": "input_text", "text": "Fix the review surface."}
                            ]
                        }
                    },
                    {
                        "timestamp": "2026-08-13T12:00:02Z",
                        "type": "response_item",
                        "payload": {
                            "type": "message",
                            "role": "assistant",
                            "content": [
                                {"type": "output_text", "text": "I will run the focused tests."}
                            ]
                        }
                    },
                    {
                        "timestamp": "2026-08-13T12:00:03Z",
                        "type": "response_item",
                        "payload": {
                            "type": "function_call",
                            "name": "shell",
                            "arguments": "{\"command\":\"cargo test review_surface\"}",
                            "call_id": "call_shell_1"
                        }
                    },
                    {
                        "timestamp": "2026-08-13T12:00:04Z",
                        "type": "response_item",
                        "payload": {
                            "type": "function_call_output",
                            "call_id": "call_shell_1",
                            "output": "running 1 test\ntest result: ok. 1 passed"
                        }
                    },
                    {
                        "timestamp": "2026-08-13T12:00:05Z",
                        "type": "response_item",
                        "payload": {
                            "type": "custom_tool_call",
                            "name": "apply_patch",
                            "input": "*** Begin Patch\n*** End Patch",
                            "call_id": "call_patch_1"
                        }
                    },
                    {
                        "timestamp": "2026-08-13T12:00:06Z",
                        "type": "response_item",
                        "payload": {
                            "type": "custom_tool_call_output",
                            "call_id": "call_patch_1",
                            "output": "Done!"
                        }
                    },
                    {
                        "timestamp": "2026-08-13T12:00:07Z",
                        "type": "response_item",
                        "payload": {
                            "type": "reasoning",
                            "summary": [{"type": "summary_text", "text": "private reasoning"}]
                        }
                    }
                ]),
                expected_count: 6,
                expected_roles: &[
                    "user",
                    "assistant",
                    "assistant",
                    "tool",
                    "assistant",
                    "tool",
                ],
                expected_tool_calls: 2,
                expected_tool_results: 2,
                must_contain: &["Fix the review surface.", "I will run the focused tests."],
                must_not_contain: &["private reasoning"],
            },
        ];

        for case in &cases {
            let messages = decode_case(case);
            assert_eq!(
                messages.len(),
                case.expected_count,
                "{} message count",
                case.name
            );
            let roles: Vec<&str> = messages
                .iter()
                .map(|message| message.role.as_str())
                .collect();
            assert_eq!(roles, case.expected_roles, "{} roles", case.name);
            for (index, message) in messages.iter().enumerate() {
                assert_eq!(message.index, index, "{} index {index}", case.name);
            }
            let tool_calls: usize = messages
                .iter()
                .map(|message| message.tool_calls.len())
                .sum();
            let tool_results: usize = messages
                .iter()
                .map(|message| message.tool_results.len())
                .sum();
            assert_eq!(
                tool_calls, case.expected_tool_calls,
                "{} tool calls",
                case.name
            );
            assert_eq!(
                tool_results, case.expected_tool_results,
                "{} tool results",
                case.name
            );
            let joined = messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            for fragment in case.must_contain {
                assert!(
                    joined.contains(fragment),
                    "{} missing fragment {fragment}",
                    case.name
                );
            }
            for fragment in case.must_not_contain {
                assert!(
                    !joined.contains(fragment),
                    "{} unexpectedly contains {fragment}",
                    case.name
                );
            }

            match case.name {
                "normalized_flat" => {
                    assert_eq!(
                        messages[1].content,
                        "I will inspect and test it.\n[Tool: Read - src/review_surface.rs]\n[Tool: Bash - cargo test review_surface]"
                    );
                    assert_eq!(messages[2].role, "");
                    assert_eq!(messages[2].content, "");
                    let read = messages[1]
                        .tool_calls
                        .iter()
                        .find(|call| call.name == "Read")
                        .expect("Read call");
                    assert_eq!(
                        read.arguments.get("file_path").and_then(Value::as_str),
                        Some("src/review_surface.rs")
                    );
                    let bash = messages[1]
                        .tool_calls
                        .iter()
                        .find(|call| call.name == "Bash")
                        .expect("Bash call");
                    assert_eq!(
                        bash.arguments.get("command").and_then(Value::as_str),
                        Some("cargo test review_surface")
                    );
                    let ids: std::collections::HashSet<_> = messages[1]
                        .tool_calls
                        .iter()
                        .map(|call| call.id.as_str())
                        .collect();
                    assert_eq!(ids.len(), 2, "inline call IDs are distinct");
                }
                "raw_claude_code" => {
                    let read = messages[1]
                        .tool_calls
                        .iter()
                        .find(|call| call.name == "Read")
                        .expect("Read call");
                    assert_eq!(read.id, "toolu_read_1");
                    assert_eq!(
                        read.arguments.get("file_path").and_then(Value::as_str),
                        Some("src/review_surface.rs")
                    );
                    assert_eq!(messages[2].tool_results.len(), 1);
                    assert_eq!(messages[2].tool_results[0].tool_call_id, "toolu_read_1");
                    assert!(!messages[2].tool_results[0].content.is_empty());
                    assert!(!messages[2].tool_results[0].is_error);
                    assert!(!messages[0].content.trim().is_empty());
                }
                "raw_codex_rollout" => {
                    let shell = messages
                        .iter()
                        .flat_map(|message| &message.tool_calls)
                        .find(|call| call.name == "shell")
                        .expect("shell call");
                    assert_eq!(
                        shell.arguments.get("command").and_then(Value::as_str),
                        Some("cargo test review_surface")
                    );
                    let patch = messages
                        .iter()
                        .flat_map(|message| &message.tool_calls)
                        .find(|call| call.name == "apply_patch")
                        .expect("apply_patch call");
                    assert_eq!(
                        patch.arguments.get("input").and_then(Value::as_str),
                        Some("*** Begin Patch\n*** End Patch")
                    );
                    let result_ids: Vec<&str> = messages
                        .iter()
                        .flat_map(|message| &message.tool_results)
                        .map(|result| result.tool_call_id.as_str())
                        .collect();
                    assert!(result_ids.contains(&"call_shell_1"));
                    assert!(result_ids.contains(&"call_patch_1"));
                }
                other => panic!("unexpected case {other}"),
            }
        }
    }

    #[test]
    fn decode_rejects_nonempty_all_noise_export() {
        let bytes = serde_json::to_vec(&json!([
            {"type": "attachment"},
            {"type": "mode"},
            {"type": "response_item", "payload": {"type": "reasoning"}}
        ]))
        .expect("serialize noise");
        let error = decode_cass_export(&bytes).expect_err("noise export must fail");
        assert!(matches!(
            error,
            CassExportDecodeError::NoSupportedMessages { record_count: 3 }
        ));
    }

    #[test]
    fn decode_rejects_malformed_json() {
        let error = decode_cass_export(b"{not-json").expect_err("malformed JSON must fail");
        assert!(matches!(error, CassExportDecodeError::Json(_)));
    }

    #[test]
    fn decode_does_not_duplicate_inline_tools_when_structured_calls_exist() {
        let bytes = serde_json::to_vec(&json!([{
            "role": "assistant",
            "content": "Reading the file\n[Tool: Read - src/lib.rs]",
            "tool_calls": [{
                "id": "structured_1",
                "name": "Read",
                "arguments": {"file_path": "src/lib.rs"}
            }]
        }]))
        .expect("serialize structured+marker");
        let messages = decode_cass_export(&bytes).expect("decode structured+marker");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tool_calls.len(), 1);
        assert_eq!(messages[0].tool_calls[0].id, "structured_1");
    }

    #[test]
    fn decode_codex_invalid_function_arguments_preserves_raw_text() {
        let bytes = serde_json::to_vec(&json!([{
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "shell",
                "arguments": "not-json",
                "call_id": "call_bad_1"
            }
        }]))
        .expect("serialize invalid args");
        let messages = decode_cass_export(&bytes).expect("invalid args must decode");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tool_calls.len(), 1);
        assert_eq!(
            messages[0].tool_calls[0].arguments,
            json!({"raw": "not-json"})
        );
    }

    #[test]
    fn decode_claude_type_user_without_message_is_noise() {
        let bytes = serde_json::to_vec(&json!([
            {"type": "user"},
            {
                "type": "user",
                "message": {
                    "role": "user",
                    "content": "Keep this turn."
                }
            }
        ]))
        .expect("serialize claude noise");
        let messages = decode_cass_export(&bytes).expect("mixed claude export");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Keep this turn.");
    }

    #[test]
    fn parse_inline_tool_markers_handles_edge_cases() {
        assert!(parse_inline_tool_markers("just some prose", 0).is_empty());
        assert!(parse_inline_tool_markers("[Tool: Bash - never closed", 0).is_empty());
        assert!(parse_inline_tool_markers("[Tool: ]", 0).is_empty());
        let calls = parse_inline_tool_markers("[Tool: Edit - /a/b.rs][Tool: Write - /c/d.rs]", 7);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "Edit");
        assert_eq!(
            calls[0].arguments.get("file_path").and_then(Value::as_str),
            Some("/a/b.rs")
        );
        assert!(calls[0].id.starts_with("inline_7_"));
        assert_ne!(calls[0].id, calls[1].id);
    }

    #[test]
    #[ignore = "set MS_CASS_EXPORT_FIXTURE to a local cass JSON export"]
    fn decode_local_export_from_env() {
        let path = std::env::var("MS_CASS_EXPORT_FIXTURE")
            .expect("MS_CASS_EXPORT_FIXTURE must point at a local cass JSON export");
        let bytes = std::fs::read(&path).unwrap_or_else(|error| {
            panic!("failed to read {path}: {error}");
        });
        let messages = decode_cass_export(&bytes).expect("local export should decode");
        let nonempty_user = messages
            .iter()
            .filter(|message| message.role == "user" && !message.content.trim().is_empty())
            .count();
        let nonempty_assistant = messages
            .iter()
            .filter(|message| message.role == "assistant" && !message.content.trim().is_empty())
            .count();
        let tool_calls: usize = messages
            .iter()
            .map(|message| message.tool_calls.len())
            .sum();
        let tool_results: usize = messages
            .iter()
            .map(|message| message.tool_results.len())
            .sum();
        println!(
            "decoded_messages={} nonempty_user={} nonempty_assistant={} tool_calls={} tool_results={}",
            messages.len(),
            nonempty_user,
            nonempty_assistant,
            tool_calls,
            tool_results
        );
        assert!(
            !messages.is_empty(),
            "decoded total must be greater than zero"
        );
        assert!(
            nonempty_user > 0,
            "need at least one non-empty user message"
        );
        assert!(
            nonempty_assistant > 0,
            "need at least one non-empty assistant message"
        );
        assert!(tool_calls > 0, "need at least one tool call");
        assert!(tool_results > 0, "need at least one tool result");
    }
}
