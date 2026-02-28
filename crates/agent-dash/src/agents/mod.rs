pub mod claude;
pub mod codex;
pub mod copilot;

/// Platform-specific installation hints.
#[derive(Debug, Clone)]
pub struct InstallHint {
    pub linux: &'static str,
    pub macos: &'static str,
    pub windows: &'static str,
}

impl InstallHint {
    /// Create an install hint that is the same on all platforms.
    pub const fn uniform(hint: &'static str) -> Self {
        Self {
            linux: hint,
            macos: hint,
            windows: hint,
        }
    }

    /// Return the install hint for the current platform.
    pub fn current(&self) -> &'static str {
        if cfg!(target_os = "windows") {
            self.windows
        } else if cfg!(target_os = "macos") {
            self.macos
        } else {
            self.linux
        }
    }
}

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
    pub install_hint: InstallHint,
}

/// All supported agent names (for help text).
pub const SUPPORTED_NAMES: &[&str] = &["claude", "codex", "copilot"];

/// Look up an agent profile by name. Returns None if unknown.
pub fn lookup(name: &str) -> Option<&'static AgentProfile> {
    match name {
        "claude" => Some(&claude::PROFILE),
        "codex" => Some(&codex::PROFILE),
        "copilot" => Some(&copilot::PROFILE),
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
    fn lookup_codex() {
        let profile = lookup("codex").unwrap();
        assert_eq!(profile.name, "codex");
        assert_eq!(profile.binary, "codex");
    }

    #[test]
    fn lookup_copilot() {
        let profile = lookup("copilot").unwrap();
        assert_eq!(profile.name, "copilot");
        assert_eq!(profile.binary, "copilot");
        assert_eq!(profile.display_name, "GitHub Copilot");
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup("unknown-agent").is_none());
    }
}
