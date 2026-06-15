//! Logic helpers shared by the manager/wizard views (MCP, Skills, Agents).
//!
//! These tabs all follow the same load-bearing invariant: global Code Puppy
//! data flows through a *workspace's* sidecar channel, so each manager picks
//! the first ready workspace. The egui widgets that once lived here were
//! removed in G5; the GPUI shell renders these screens itself.

/// Mirror Code Puppy's registry rule: alphanumeric plus `-`/`_`, non-empty.
pub fn validate_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a name is required".into());
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err("name must be alphanumeric (hyphens and underscores allowed)".into());
    }
    Ok(())
}

/// Form vs. raw-paste mode for a create/edit wizard. The form is the guided
/// step-by-step builder; paste lets you drop in a whole config and validate it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    Form,
    Paste,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(validate_name("my-server_2").is_ok());
        assert!(validate_name(" padded ").is_ok()); // trimmed before checking
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("has space").is_err());
        assert!(validate_name("bad/slash").is_err());
        assert!(validate_name("dot.dot").is_err());
    }
}
