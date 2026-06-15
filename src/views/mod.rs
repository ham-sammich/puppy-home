//! Shared, frontend-agnostic logic for the manager/wizard screens.
//!
//! The egui renderers were removed in Phase G5; what remains are the state
//! machines and pure-logic helpers the GPUI shell (`gpui_ui`) consumes:
//! wizard step state, filter/badge helpers, and the blocking remote-connect
//! calls.

pub mod agent_manager;
pub mod agent_wizard;
pub mod common;
pub mod mcp_wizard;
pub mod remote_connect;
pub mod skills_manager;
pub mod skills_wizard;
