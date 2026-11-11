use super::*;
pub fn run_core_client() -> io::Result<()> {
    run_client_with_mode(
        None,
        None,
        "connecting to Core",
        Some(core::Options::from_environment()?),
    )
}
