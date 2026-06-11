//! Dockable views. Chat lives on `Workspace::render_chat`; cross-workspace
//! views (dashboard, and later git) live here.

pub mod agent_manager;
pub mod agent_wizard;
pub mod common;
pub mod dashboard;
pub mod mcp_manager;
pub mod mcp_wizard;
pub mod path_browser;
pub mod remote_connect;
pub mod skills_manager;
pub mod skills_wizard;
