//! Dockable views. Chat lives on `Workspace::render_chat`; cross-workspace
//! views (dashboard, and later git) live here.

pub mod common;
pub mod dashboard;
pub mod mcp_manager;
pub mod skills_manager;
pub mod skills_wizard;
