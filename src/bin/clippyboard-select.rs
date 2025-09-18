fn main() -> Result<(), eyre::Error> {
    let socket_path = clippyboard::socket_path()?;
    clippyboard::display::main(&socket_path)
}
