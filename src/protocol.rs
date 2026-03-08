use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
pub struct Request {
	pub id: Value,
	pub method: String,
	#[serde(default)]
	pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct Response {
	pub jsonrpc: String,
	pub id: Value,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub result: Option<Value>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub error: Option<ResponseError>,
}

#[derive(Debug, Serialize)]
pub struct ResponseError {
	pub code: i64,
	pub message: String,
}

impl Response {
	pub fn ok(id: Value, value: Value) -> Self {
		Response {
			jsonrpc: "2.0".to_string(),
			id,
			result: Some(value),
			error: None
		}
	}
	pub fn err(id: Value, code: i64, message: String) -> Self {
		Response {
			jsonrpc: "2.0".to_string(),
			id,
			result: None,
			error: Some(ResponseError { code, message })
		}
	}
}

pub fn tool_error(name: &str, message: &str) -> Value {
	json!({
		"tool": name,
		"error": message
	})
}
