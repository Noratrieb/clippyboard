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
    last_copied: Arc<AtomicU64>,
    items: Arc<Mutex<Vec<Entry>>>,
) -> eyre::Result<()> {
    let mut request = [0; 1];
    let Ok(()) = peer.read_exact(&mut request) else {
        return Ok(());
    };
    match request[0] {
        super::MESSAGE_STORE => {
            handle_store(next_id, last_copied, &items).wrap_err("handling store message")?;
        }
        super::MESSAGE_READ => {
            let items = items.lock().unwrap();

            ciborium::into_writer(items.as_slice(), BufWriter::new(peer))
                .wrap_err("writing items to socket")?;
        }
        super::MESSAGE_COPY => {
            handle_copy(peer, last_copied, items).wrap_err("handling copy message")?;
        }
        _ => {}
    };
    Ok(())
}

fn handle_copy(
    mut peer: UnixStream,
    last_copied: Arc<AtomicU64>,
    items: Arc<Mutex<Vec<Entry>>>,
) -> Result<(), eyre::Error> {
    let mut id = [0; 8];
    peer.read_exact(&mut id).wrap_err("failed to read id")?;
    let id = u64::from_le_bytes(id);
    let mut items = items.lock().unwrap();
    let Some(idx) = items.iter().position(|item| item.id == id) else {
        return Ok(());
    };
    let entry = items.remove(idx);
    items.push(entry.clone());
    let mut opts = wl_clipboard_rs::copy::Options::new();
    opts.clipboard(wl_clipboard_rs::copy::ClipboardType::Regular)
        .seat(wl_clipboard_rs::copy::Seat::All);
    let result = wl_clipboard_rs::copy::copy(
        opts,
        wl_clipboard_rs::copy::Source::Bytes(entry.data.into_boxed_slice()),
        wl_clipboard_rs::copy::MimeType::Specific(entry.mime),
    );
    last_copied.store(entry.id, std::sync::atomic::Ordering::Relaxed);
    if let Err(err) = result {
        println!("WARNING: Copy failed: {err:?}");
    }
    Ok(())
}

fn handle_store(
    next_id: Arc<AtomicU64>,
    last_copied: Arc<AtomicU64>,
    items: &Arc<Mutex<Vec<Entry>>>,
) -> Result<(), eyre::Error> {
    let time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();

    let mime_types =
        wl_clipboard_rs::paste::get_mime_types(ClipboardType::Regular, Seat::Unspecified)
            .wrap_err("getting mime types")?;

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

    let new_entry = Entry {
        id: next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        mime: mime.to_string(),
        data,
        created_time: u64::try_from(time.as_millis()).unwrap(),
    };

    let mut items = items.lock().unwrap();
    if items
        .last()
        .is_some_and(|last| last.mime == new_entry.mime && last.data == new_entry.data)
    {
        println!("INFO: Skipping store of new item because it is identical to last one");
        return Ok(());
    }

    let last_copied = last_copied.load(std::sync::atomic::Ordering::Relaxed);
    if let Some(item) = items.iter().find(|item| item.id == last_copied)
        && item.mime == new_entry.mime
        && item.data == new_entry.data
    {
        println!("INFO: Skipping store of new item because the copy came from us");
        return Ok(());
    }

    items.push(new_entry);

    let mut running_total = 0;
    let mut cutoff = None;
    for (idx, item) in items.iter().enumerate().rev() {
        running_total += item.data.len() + std::mem::size_of::<Entry>();
        if running_total > crate::MAX_HISTORY_BYTE_SIZE {
            cutoff = Some(idx);
        }
    }
    if let Some(cutoff) = cutoff {
        println!(
            "INFO: Dropping old {} items because limit of {} bytes was reached for the history",
            cutoff + 1,
            crate::MAX_HISTORY_BYTE_SIZE
        );
        items.splice(0..=cutoff, []);
    }

    println!(
        "INFO: Successfully stored clipboard value of mime type {mime} (new history size {running_total})"
    );
    Ok(())
}

pub fn main(socket_path: &PathBuf) -> eyre::Result<()> {
    let _ = std::fs::remove_file(&socket_path); // lol
    let socket = UnixListener::bind(&socket_path)
        .wrap_err_with(|| format!("binding path {}", socket_path.display()))?;

    let next_id = Arc::new(AtomicU64::new(0));
    let items = Arc::new(Mutex::new(Vec::<Entry>::new()));
    // for deduplication because the event stream will tell us that we just copied something :)
    let last_copied = Arc::new(AtomicU64::new(u64::MAX));

    println!("INFO: Listening on {}", socket_path.display());

    for peer in socket.incoming() {
        match peer {
            Ok(peer) => {
                let next_id = next_id.clone();
                let items = items.clone();
                let last_copied = last_copied.clone();
                std::thread::spawn(move || {
                    let result = handle_peer(peer, next_id, last_copied, items);
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
