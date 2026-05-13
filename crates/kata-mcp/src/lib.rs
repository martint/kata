//! MCP server adapter over [`kata_service::ReviewService`].
//!
//! Mounted as an axum service on a chosen path (typically `/mcp`). Agents
//! issue tool calls via MCP and end up driving the same business-logic
//! layer that backs the HTTP API.

pub mod dispatcher;
pub mod tools;

pub use dispatcher::{AUTHOR_QUERY_PARAM, McpDispatcher};
pub use tools::{ReviewMcp, mcp_service};
