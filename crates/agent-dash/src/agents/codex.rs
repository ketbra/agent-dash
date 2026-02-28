use super::{AgentProfile, InstallHint};

pub const PROFILE: AgentProfile = AgentProfile {
    name: "codex",
    binary: "codex",
    display_name: "Codex CLI",
    install_hint: InstallHint::uniform("npm install -g @openai/codex"),
};
