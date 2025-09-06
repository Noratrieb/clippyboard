mod display;
mod daemon;

use std::{io::Write, os::unix::net::UnixStream};
use eyre::{Context, OptionExt, bail};

const MAX_ENTRY_SIZE: u64 = 100_000_000;

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct Entry {
    id: u64,
    mime: String,
    data: Vec<u8>,
    created_time: u64,
}

const MESSAGE_STORE: u8 = 0;
const MESSAGE_READ: u8 = 1;
/// Argument: One u64-bit LE value, the ID
const MESSAGE_COPY: u8 = 2;

fn main() -> eyre::Result<()> {
    let Some(mode) = std::env::args().nth(1) else {
        bail!("missing mode");
    };

    let socket_path = dirs::runtime_dir()
        .ok_or_eyre("missing XDG_RUNTIME_DIR")?
        .join("clippyboard.sock");

    match mode.as_str() {
        "daemon" => daemon::main(&socket_path)?,
        "store" => {
            let mut socket = UnixStream::connect(&socket_path).wrap_err_with(|| {
                format!(
                    "connecting to socket at {}. is the daemon running?",
                    socket_path.display()
                )
            })?;

            socket
                .write_all(&[MESSAGE_STORE])
                .wrap_err("writing request type")?;
        }
        "display" => display::main(&socket_path)?,
        _ => panic!("invalid mode"),
    }

    Ok(())
}
