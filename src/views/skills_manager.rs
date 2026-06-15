//! Skills Manager: Code Puppy's skills (SKILL.md folders) as a dockable tab.
//!
//! Same load-bearing invariant as the MCP manager: skills are global/project
//! Code Puppy config, but all data flows through a workspace's sidecar
//! channel. We pick the first ready workspace, read the catalog it received,
//! and send ops through its backend handle. The create/edit modal lives in
//! [`super::skills_wizard`].

use crate::backend::SkillInfo;

/// Return the markdown body of a SKILL.md: everything after the closing
/// `---` frontmatter fence. Content without a well-formed fence is returned
/// unchanged.
pub(crate) fn skill_body(content: &str) -> &str {
    let trimmed = content.trim_start_matches('\u{feff}');
    let Some(rest) = trimmed.strip_prefix("---") else {
        return content;
    };
    let Some(rest) = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
    else {
        return content;
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return rest[offset + line.len()..].trim_start_matches(['\r', '\n']);
        }
        offset += line.len();
    }
    content // no closing fence: treat the whole thing as body
}

/// Case-insensitive name/description filter. `needle` must be lowercased.
pub(crate) fn matches_filter(skill: &SkillInfo, needle: &str) -> bool {
    needle.is_empty()
        || skill.name.to_lowercase().contains(needle)
        || skill.description.to_lowercase().contains(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, description: &str) -> SkillInfo {
        SkillInfo {
            name: name.into(),
            description: description.into(),
            path: String::new(),
            enabled: true,
            source: "user".into(),
        }
    }

    #[test]
    fn body_strips_well_formed_frontmatter() {
        let md = "---\nname: x\ndescription: y\n---\n\n## Body\n";
        assert_eq!(skill_body(md), "## Body\n");
    }

    #[test]
    fn body_handles_crlf_and_bom() {
        let md = "\u{feff}---\r\nname: x\r\n---\r\nBody\r\n";
        assert_eq!(skill_body(md), "Body\r\n");
    }

    #[test]
    fn body_without_frontmatter_is_unchanged() {
        assert_eq!(skill_body("just text"), "just text");
        assert_eq!(skill_body("--- not a fence"), "--- not a fence");
    }

    #[test]
    fn body_without_closing_fence_is_unchanged() {
        let md = "---\nname: x\nno closing fence";
        assert_eq!(skill_body(md), md);
    }

    #[test]
    fn filter_matches_name_and_description_case_insensitively() {
        let s = info("Git-Flow", "Release branching");
        assert!(matches_filter(&s, ""));
        assert!(matches_filter(&s, "git"));
        assert!(matches_filter(&s, "branch"));
        assert!(!matches_filter(&s, "docker"));
    }
}
