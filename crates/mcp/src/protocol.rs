//! The Model Context Protocol's JSON-RPC: `initialize`, `ping`, the tools,
//! the resources and the prompts. Messages come one at a time (a line of
//! stdio, or the body of a POST to `/mcp`); the transports only move the
//! text. Nothing here keeps a session beyond the token's preset: every
//! call is a request to the API with the same token.

use std::sync::Mutex;

use nikgob_core::settings::ApiPreset;
use serde_json::{Value, json};

use crate::backend::{ApiRequest, Backend};
use crate::resources::ReadError;
use crate::{prompts, resources, tools};

/// The protocol versions understood, newest first. A client asking for
/// another one gets the newest.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// What the server tells the client about itself.
const INSTRUCTIONS: &str = "Controls the music.nikgob player on the user's computer and reads its library. \
Start with status_get or the music://now-playing resource. Use library_search for the user's own music and discover_search for anything else. \
Destructive tools (playlist_remove, library_retag, library_trash) answer a plan and a confirm_token first: show the plan and repeat the call with the token only after the user agrees. \
Messages from the player are in Russian.";

/// A JSON-RPC error.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub const PARSE: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    /// The MCP's code of a resource that is not there.
    pub const RESOURCE_NOT_FOUND: i64 = -32002;

    fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

fn reply(id: Value, r: Result<Value, RpcError>) -> Value {
    match r {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": e.code, "message": e.message}}),
    }
}

/// A tool's answer as the protocol carries it.
pub fn tool_result(value: &Value, is_error: bool) -> Value {
    let text = serde_json::to_string(value).unwrap_or_default();
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

/// Where the preset of the session's token comes from.
enum Preset {
    /// The API server knows it (`/mcp`).
    Known(ApiPreset),
    /// Asked of the API once it answers (stdio).
    Asked(Mutex<Option<ApiPreset>>),
}

/// An MCP server over the API that `backend` reaches.
pub struct McpServer<B> {
    backend: B,
    preset: Preset,
}

impl<B: Backend> McpServer<B> {
    /// A server that asks the API which preset its token has.
    pub fn new(backend: B) -> Self {
        Self { backend, preset: Preset::Asked(Mutex::new(None)) }
    }

    /// A server whose token's preset is known.
    pub fn with_preset(backend: B, preset: ApiPreset) -> Self {
        Self { backend, preset: Preset::Known(preset) }
    }

    /// The token's preset; `None` while the API cannot say (not running).
    async fn preset(&self) -> Option<ApiPreset> {
        let cell = match &self.preset {
            Preset::Known(p) => return Some(*p),
            Preset::Asked(cell) => cell,
        };
        if let Some(p) = *cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            return Some(p);
        }
        let r = self.backend.call(ApiRequest::get("/v1/token")).await.ok().filter(|r| r.ok())?;
        let p: ApiPreset = serde_json::from_value(r.body.get("preset")?.clone()).ok()?;
        *cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(p);
        Some(p)
    }

    /// One message as text (or a batch of them); the answer to send back,
    /// if any (notifications get none).
    pub async fn handle_text(&self, text: &str) -> Option<String> {
        let msg: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => return Some(reply(Value::Null, Err(RpcError::new(RpcError::PARSE, format!("Parse error: {e}")))).to_string()),
        };
        match msg {
            Value::Array(batch) => {
                let mut out = Vec::new();
                for m in batch {
                    if let Some(r) = self.handle(m).await {
                        out.push(r);
                    }
                }
                (!out.is_empty()).then(|| Value::Array(out).to_string())
            }
            m => self.handle(m).await.map(|r| r.to_string()),
        }
    }

    /// One message; the answer, if it is a request.
    pub async fn handle(&self, msg: Value) -> Option<Value> {
        let id = msg.get("id").filter(|i| !i.is_null()).cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A client's answer to a request of ours (none are sent), or junk.
            let is_answer = msg.get("result").is_some() || msg.get("error").is_some();
            return match id {
                Some(id) if !is_answer => Some(reply(id, Err(RpcError::new(RpcError::INVALID_REQUEST, "No method")))),
                _ => None,
            };
        };
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = id else {
            // Notifications: initialized, cancelled, progress… nothing to do.
            log::debug!("mcp: notification {method}");
            return None;
        };
        Some(reply(id, self.request(method, &params).await))
    }

    async fn request(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {
                        "tools": {"listChanged": false},
                        "resources": {"subscribe": false, "listChanged": false},
                        "prompts": {"listChanged": false}
                    },
                    "serverInfo": {"name": "music.nikgob", "title": "music.nikgob", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS
                }))
            }
            "ping" | "logging/setLevel" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools::list(self.preset().await)})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or_else(|| RpcError::new(RpcError::INVALID_PARAMS, "No tool name"))?;
                let Some(tool) = tools::find(name) else {
                    return Err(RpcError::new(RpcError::INVALID_PARAMS, format!("Unknown tool: {name}")));
                };
                if !tools::visible(tool, self.preset().await) {
                    let e = json!({"error": "forbidden", "message": "У токена нет нужного права"});
                    return Ok(tool_result(&e, true));
                }
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                Ok(match tools::call(&self.backend, name, &args).await {
                    Ok(v) => tool_result(&v, false),
                    Err(e) => tool_result(&e.to_value(), true),
                })
            }
            "resources/list" => Ok(json!({"resources": resources::list()})),
            "resources/templates/list" => Ok(json!({"resourceTemplates": resources::templates()})),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).ok_or_else(|| RpcError::new(RpcError::INVALID_PARAMS, "No uri"))?;
                resources::read(&self.backend, uri).await.map_err(|e| match e {
                    ReadError::Unknown(u) => RpcError::new(RpcError::RESOURCE_NOT_FOUND, format!("Resource not found: {u}")),
                    ReadError::Tool(t) => RpcError::new(RpcError::INTERNAL, t.to_value().to_string()),
                })
            }
            "prompts/list" => Ok(json!({"prompts": prompts::list()})),
            "prompts/get" => {
                let name = params.get("name").and_then(Value::as_str).ok_or_else(|| RpcError::new(RpcError::INVALID_PARAMS, "No prompt name"))?;
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                prompts::get(name, &args).map_err(|m| RpcError::new(RpcError::INVALID_PARAMS, m))
            }
            other => Err(RpcError::new(RpcError::METHOD_NOT_FOUND, format!("Method not found: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ApiResponse, Unreachable};

    /// An API whose token has `preset`; every other route answers `{}`.
    struct Api(&'static str);

    impl Backend for Api {
        async fn call(&self, r: ApiRequest) -> Result<ApiResponse, Unreachable> {
            if r.path == "/v1/token" {
                return Ok(ApiResponse { status: 200, body: json!({"id": "x", "preset": self.0}) });
            }
            Ok(ApiResponse { status: 200, body: json!({"path": r.path}) })
        }
    }

    struct Down;

    impl Backend for Down {
        async fn call(&self, _: ApiRequest) -> Result<ApiResponse, Unreachable> {
            Err(Unreachable::NotRunning)
        }
    }

    async fn ask<B: Backend>(s: &McpServer<B>, method: &str, params: Value) -> Value {
        s.handle(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})).await.expect("an answer")
    }

    #[tokio::test]
    async fn initialize_answers_a_known_version_or_the_newest() {
        let s = McpServer::new(Api("full"));
        let r = ask(&s, "initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}})).await;
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert_eq!(r["result"]["serverInfo"]["name"], "music.nikgob");
        let r = ask(&s, "initialize", json!({"protocolVersion": "1999-01-01"})).await;
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert!(s.handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await.is_none());
        assert_eq!(ask(&s, "ping", json!({})).await["result"], json!({}));
        assert_eq!(ask(&s, "nope", json!({})).await["error"]["code"], RpcError::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn the_tools_listed_follow_the_token() {
        let n = |v: Value| v["result"]["tools"].as_array().unwrap().len();
        assert_eq!(n(ask(&McpServer::new(Api("read")), "tools/list", json!({})).await), 7);
        assert_eq!(n(ask(&McpServer::new(Api("control")), "tools/list", json!({})).await), 11);
        assert_eq!(n(ask(&McpServer::with_preset(Down, ApiPreset::Full), "tools/list", json!({})).await), 17);
        // The player is not running: everything is listed, each call says why it fails.
        let down = McpServer::new(Down);
        assert_eq!(n(ask(&down, "tools/list", json!({})).await), 17);
        let r = ask(&down, "tools/call", json!({"name": "status_get", "arguments": {}})).await;
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("Плеер не запущен"));
    }

    #[tokio::test]
    async fn a_tool_the_token_cannot_use_is_refused() {
        let s = McpServer::new(Api("read"));
        let r = ask(&s, "tools/call", json!({"name": "library_trash", "arguments": {"track_ids": ["a"]}})).await;
        assert_eq!(r["result"]["isError"], true);
        let r = ask(&s, "tools/call", json!({"name": "outputs_list"})).await;
        assert_eq!(r["result"]["isError"], false);
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap()["path"], "/v1/outputs");
        assert_eq!(ask(&s, "tools/call", json!({"name": "zzz"})).await["error"]["code"], RpcError::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn resources_and_prompts() {
        let s = McpServer::new(Api("read"));
        assert_eq!(ask(&s, "resources/list", json!({})).await["result"]["resources"].as_array().unwrap().len(), 4);
        assert_eq!(ask(&s, "resources/templates/list", json!({})).await["result"]["resourceTemplates"].as_array().unwrap().len(), 1);
        let r = ask(&s, "resources/read", json!({"uri": "music://playlist/liked"})).await;
        let text = r["result"]["contents"][0]["text"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap()["path"], "/v1/playlists/liked");
        assert_eq!(ask(&s, "resources/read", json!({"uri": "music://zzz"})).await["error"]["code"], RpcError::RESOURCE_NOT_FOUND);
        assert_eq!(ask(&s, "prompts/list", json!({})).await["result"]["prompts"].as_array().unwrap().len(), 3);
        assert!(ask(&s, "prompts/get", json!({"name": "library-audit"})).await["result"]["messages"].is_array());
    }

    #[tokio::test]
    async fn text_messages_batches_and_junk() {
        let s = McpServer::new(Api("full"));
        let r = s.handle_text("{nope").await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&r).unwrap()["error"]["code"], RpcError::PARSE);
        let r = s.handle_text(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"}]"#).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&r).unwrap().as_array().unwrap().len(), 1);
        assert!(s.handle_text(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}"#).await.is_none());
        assert!(s.handle_text(r#"{"jsonrpc":"2.0","id":5,"result":{}}"#).await.is_none(), "a client's answer");
    }
}
