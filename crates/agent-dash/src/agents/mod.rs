pub mod claude;

/// Describes a supported agent binary.
#[derive(Debug, Clone)]
pub struct AgentProfile {
    /// Short name used in CLI (e.g., "claude")
    pub name: &'static str,
    /// Binary name to find in $PATH
    pub binary: &'static str,
    /// Human-readable name
    pub display_name: &'static str,
    /// How to install the agent if it's not found
    pub install_hint: &'static str,
}

/// Look up an agent profile by name. Returns None if unknown.
pub fn lookup(name: &str) -> Option<&'static AgentProfile> {
    match name {
        "claude" => Some(&claude::PROFILE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_claude() {
        let profile = lookup("claude").unwrap();
        assert_eq!(profile.name, "claude");
        assert_eq!(profile.binary, "claude");
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup("unknown-agent").is_none());
    }
}
