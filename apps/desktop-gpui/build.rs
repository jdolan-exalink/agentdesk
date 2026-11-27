include!("../../build-support/winres.rs");

fn main() {
    embed_windows_resources(
        "../../assets/icons/agentdesk-client.ico",
        "agentdesk-desktop-gpui",
        "AgentDesk - cliente de escritorio remoto",
        false,
    );
}
