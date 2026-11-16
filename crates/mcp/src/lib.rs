//! The player's MCP server: 18 tools in four groups of rights, four
//! resources, a template and three prompts over the local REST API (crates/api), so
//! the token, its rights, the rate limit and its actor are the API's.
//!
//! * stdio: `nikgob mcp` (or the console `nikgob-mcp`) talks to the
//!   running player over HTTP ([`stdio::main`]).
//! * Streamable HTTP: the API serves `POST /mcp` itself and calls its own
//!   routes in-process with the caller's token ([`McpServer`] over a
//!   [`Backend`] of its own).
//!
//! The protocol's JSON-RPC is implemented here directly (`initialize`,
//! `ping`, `tools/*`, `resources/*`, `prompts/*`); the transports only
//! carry text.

pub mod backend;
pub mod http;
pub mod prompts;
pub mod protocol;
pub mod resources;
pub mod stdio;
pub mod tools;

pub use backend::{ApiRequest, ApiResponse, Backend, Method, NOT_RUNNING, Unreachable};
pub use protocol::{McpServer, PROTOCOL_VERSIONS, RpcError};
