include!("../../build-support/winres.rs");

fn main() {
    embed_windows_resources(
        "../../assets/icons/agentdesk-quicksupport.ico",
        "agentdesk-quicksupport",
        "AgentDesk QuickSupport - asistencia remota",
        false,
    );
}
