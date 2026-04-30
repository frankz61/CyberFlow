//! Local MCP (Model Context Protocol) server.
//!
//! Exposes the Sangfor and mstsc full-flow automations as MCP tools over a
//! local HTTP + JSON-RPC endpoint (MCP's "Streamable HTTP" transport). A
//! fixed Bearer token gates access. Binds to 127.0.0.1 only — never the
//! network.
//!
//! Endpoint: `POST http://127.0.0.1:<port>/mcp`
//! Header:   `Authorization: Bearer <token>`

pub mod server;
pub mod state;
pub mod tools;

pub use server::ServerInfo;
pub use state::McpServerState;
