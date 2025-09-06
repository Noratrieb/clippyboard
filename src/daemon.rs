use super::Entry;
use super::MAX_ENTRY_SIZE;
use eyre::Context;
use std::io::BufWriter;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::time::SystemTime;
use wl_clipboard_rs::paste::ClipboardType;
use wl_clipboard_rs::paste::MimeType;
use wl_clipboard_rs::paste::Seat;

pub(crate) fn handle_peer(
    mut peer: UnixStream,
    next_id: Arc<AtomicU64>,
    items: Arc<Mutex<Vec<Entry>>>,
) -> eyre::Result<()> {
    let mut request = [0; 1];
    peer.read_exact(&mut request)
        .wrap_err("failed to read message type")?;
    match request[0] {
        super::MESSAGE_STORE => {
            let mime_types =
                wl_clipboard_rs::paste::get_mime_types(ClipboardType::Regular, Seat::Unspecified)
                    .wrap_err("getting mime types")?;

            let time = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap();

            let Some(mime) = ["text/plain", "image/png"]
                .iter()
                .find(|mime| mime_types.contains(**mime))
            else {
                eprintln!("WARN: No supported mime type found. Found mime types: {mime_types:?}");
                return Ok(());
            };

            let (data_readear, _) = wl_clipboard_rs::paste::get_contents(
                ClipboardType::Regular,
                Seat::Unspecified,
                MimeType::Specific(mime),
            )
            .wrap_err("getting contents")?;

            let mut data_reader = data_readear.take(MAX_ENTRY_SIZE);

            let mut data = Vec::new();
            data_reader
                .read_to_end(&mut data)
                .wrap_err("reading content data")?;

            items.lock().unwrap().push(Entry {
                id: next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                mime: mime.to_string(),
                data,
                created_time: u64::try_from(time.as_millis()).unwrap(),
            });

            println!("INFO Successfully stored clipboard value of mime type {mime}");
        }
        super::MESSAGE_READ => {
            let items = items.lock().unwrap();

            ciborium::into_writer(items.as_slice(), BufWriter::new(peer))
                .wrap_err("writing items to socket")?;
        }
        super::MESSAGE_COPY => {
            let mut id = [0; 8];
            peer.read_exact(&mut id).wrap_err("failed to read id")?;
            let id = u64::from_le_bytes(id);

            let items = items.lock().unwrap();

            let Some(idx) = items.iter().position(|item| item.id == id) else {
                return Ok(());
            };

            let entry = items[idx].clone();

            // select
            let mut opts = wl_clipboard_rs::copy::Options::new();
            opts.clipboard(wl_clipboard_rs::copy::ClipboardType::Regular);
            let result = wl_clipboard_rs::copy::copy(
                opts,
                wl_clipboard_rs::copy::Source::Bytes(entry.data.into_boxed_slice()),
                wl_clipboard_rs::copy::MimeType::Specific(entry.mime),
            );
            if let Err(err) = result {
                println!("WARNING: Copy failed: {err:?}");
            }
        }
        _ => {}
    };
    Ok(())
}

pub fn main(socket_path: &PathBuf) -> eyre::Result<()> {
    let _ = std::fs::remove_file(&socket_path); // lol
    let socket = UnixListener::bind(&socket_path)
        .wrap_err_with(|| format!("binding path {}", socket_path.display()))?;

    let next_id = Arc::new(AtomicU64::new(0));
    let items = Arc::new(Mutex::new(Vec::<Entry>::new()));

    println!("INFO: Listening on {}", socket_path.display());

    for peer in socket.incoming() {
        match peer {
            Ok(peer) => {
                let next_id = next_id.clone();
                let items = items.clone();
                std::thread::spawn(move || {
                    let result = handle_peer(peer, next_id, items);
                    if let Err(err) = result {
                        eprintln!("ERROR: Error handling peer: {err:?}");
                    }
                });
            }
            Err(err) => {
                eprintln!("ERROR: Error accepting peer: {err}");
            }
        }
    }

    Ok(())
}
