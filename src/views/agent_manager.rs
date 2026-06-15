//! Agent Manager: Code Puppy's agents (JSON configs + built-ins) as a
//! dockable tab.
//!
//! Same load-bearing invariant as the MCP/Skills managers: agents are global/
//! project Code Puppy config, but all data flows through a workspace's sidecar
//! channel. We pick the first ready workspace, read the catalog it received,
//! and send ops through its backend handle. The visual builder lives in
//! [`super::agent_wizard`]; built-in (Python) agents are read-only and can be
//! cloned into editable JSON copies.

use crate::backend::AgentConfigInfo;

/// Case-insensitive name/description filter. `needle` must be lowercased.
pub(crate) fn matches_filter(agent: &AgentConfigInfo, needle: &str) -> bool {
    needle.is_empty()
        || agent.name.to_lowercase().contains(needle)
        || agent.display_name.to_lowercase().contains(needle)
        || agent.description.to_lowercase().contains(needle)
}

/// A short, human badge for where an agent lives.
pub(crate) fn source_badge(source: &str) -> &str {
    match source {
        "user" => "user",
        "project" => "project",
        "builtin" => "built-in",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, display: &str, description: &str) -> AgentConfigInfo {
        AgentConfigInfo {
            name: name.into(),
            display_name: display.into(),
            description: description.into(),
            model: String::new(),
            tool_count: 0,
            source: "user".into(),
            editable: true,
            path: String::new(),
            current: false,
        }
    }

    #[test]
    fn filter_matches_name_display_and_description() {
        let a = info("qa-kitten", "QA Kitten", "Writes tests");
        assert!(matches_filter(&a, ""));
        assert!(matches_filter(&a, "kitten"));
        assert!(matches_filter(&a, "qa")); // matches display name
        assert!(matches_filter(&a, "tests"));
        assert!(!matches_filter(&a, "docker"));
    }

    #[test]
    fn source_badges_are_human() {
        assert_eq!(source_badge("user"), "user");
        assert_eq!(source_badge("project"), "project");
        assert_eq!(source_badge("builtin"), "built-in");
        assert_eq!(source_badge("weird"), "weird");
    }
}
