use super::AgentProfile;

pub const PROFILE: AgentProfile = AgentProfile {
    name: "codex",
    binary: "codex",
    display_name: "Codex CLI",
    install_hint: "npm install -g @openai/codex",
};
