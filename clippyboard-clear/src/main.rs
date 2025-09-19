use std::{io::Write, os::unix::net::UnixStream};

use eyre::Context;

fn main() -> eyre::Result<()> {
    let socket_path = clippyboard_shared::socket_path()?;

    let mut socket = UnixStream::connect(&socket_path).wrap_err_with(|| {
        format!(
            "connecting to socket at {}. is the daemon running?",
            socket_path.display()
        )
    })?;
    socket
        .write_all(&[clippyboard_shared::MESSAGE_CLEAR])
        .wrap_err("writing clear message to socket")?;

    Ok(())
}
