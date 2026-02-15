use super::AgentProfile;

pub const PROFILE: AgentProfile = AgentProfile {
    name: "claude",
    binary: "claude",
    display_name: "Claude Code",
    install_hint: "curl -fsSL https://claude.ai/install.sh | bash",
};
