fn main() -> Result<(), eyre::Error> {
    let socket_path: std::path::PathBuf = clippyboard::socket_path()?;
    clippyboard::daemon::main(&socket_path)
}
