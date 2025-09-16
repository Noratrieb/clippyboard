use super::HistoryItem;
use super::MAX_ENTRY_SIZE;
use eframe::egui::ahash::HashSet;
use eyre::Context;
use std::collections::HashMap;
use std::io::BufWriter;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use std::time::SystemTime;
use wayland_client::Dispatch;
use wayland_client::Proxy;
use wayland_client::backend::ObjectId;
use wayland_client::event_created_child;
use wayland_client::globals::GlobalListContents;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::{
    EVT_DATA_OFFER_OPCODE, ExtDataControlDeviceV1,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1::ExtDataControlManagerV1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::ExtDataControlOfferV1;
use wl_clipboard_rs::paste::ClipboardType;
use wl_clipboard_rs::paste::MimeType;
use wl_clipboard_rs::paste::Seat;

struct HistoryState {
    next_item_id: Arc<AtomicU64>,
    last_copied_item_id: Arc<AtomicU64>,
    items: Arc<Mutex<Vec<HistoryItem>>>,
}

struct InProgressOffer {
    mime_types: HashSet<String>,
    time: Duration,
}

#[derive(Debug)]
struct CurrentSelection {
    mime_types: HashSet<String>,
    offer: ExtDataControlOfferV1,
    time: Duration,
}

struct WlState {
    history_state: Arc<HistoryState>,

    offers: HashMap<ObjectId, InProgressOffer>,
    current_primary_selection: Option<CurrentSelection>,
    current_selection: Option<CurrentSelection>,
}

impl Dispatch<WlRegistry, GlobalListContents> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegistry,
        _event: <WlRegistry as wayland_client::Proxy>::Event,
        _data: &GlobalListContents,
        _conn: &wayland_client::Connection,
        _qhandle: &wayland_client::QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ExtDataControlManagerV1, ()> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &ExtDataControlManagerV1,
        _event: <ExtDataControlManagerV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        _qhandle: &wayland_client::QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlSeat, ()> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &WlSeat,
        _event: <WlSeat as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        _qhandle: &wayland_client::QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ExtDataControlDeviceV1, ()> for WlState {
    fn event(
        state: &mut Self,
        _proxy: &ExtDataControlDeviceV1,
        event: <ExtDataControlDeviceV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        _qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            // A new offer is being prepared, register it and don't do anything yet
            ext_data_control_device_v1::Event::DataOffer { id } => {
                state.offers.insert(
                    id.id(),
                    InProgressOffer {
                        mime_types: Default::default(),
                        time: SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap(),
                    },
                );
            }

            // The selection has been confirmed, we just properly got a new offer that we should use.
            ext_data_control_device_v1::Event::Selection { id } => {
                let new_offer = match id {
                    Some(id) => {
                        let offer = state.offers.remove(&id.id());

                        offer.map(|offer| CurrentSelection {
                            offer: id,
                            mime_types: offer.mime_types,
                            time: offer.time,
                        })
                    }
                    None => None,
                };

                if let Some(current) = &state.current_selection {
                    current.offer.destroy();
                }

                state.current_selection = new_offer;

                if let Some(offer) = &state.current_selection {
                    let Some(mime) = ["text/plain", "image/png"]
                        .iter()
                        .find(|mime| offer.mime_types.contains(**mime))
                    else {
                        eprintln!(
                            "WARN: No supported mime type found. Found mime types: {:?}",
                            offer.mime_types
                        );
                        return;
                    };

                    let (reader, writer) = std::io::pipe().unwrap();
                    offer.offer.receive(mime.to_string(), writer.as_fd());

                    let history_state = state.history_state.clone();
                    let mime = mime.to_string();
                    let time = offer.time;
                    std::thread::spawn(move || {
                        let result =
                            do_read_clipboard_into_history(&history_state, time, mime, reader);
                        if let Err(err) = result {
                            eprintln!("WARN: Failed to read clipboard: {:?}", err)
                        }
                    });
                }
            }
            ext_data_control_device_v1::Event::PrimarySelection { id } => {
                let new_offer = match id {
                    Some(id) => {
                        let offer = state.offers.remove(&id.id());

                        offer.map(|offer| CurrentSelection {
                            offer: id,
                            mime_types: offer.mime_types,
                            time: offer.time,
                        })
                    }
                    None => None,
                };

                if let Some(current) = &state.current_primary_selection {
                    current.offer.destroy();
                }

                state.current_primary_selection = new_offer;
            }
            _ => {}
        }
    }

    event_created_child!(WlState, ExtDataControlDeviceV1, [
        EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for WlState {
    fn event(
        state: &mut Self,
        proxy: &ExtDataControlOfferV1,
        event: <ExtDataControlOfferV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        _qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_offer_v1::Event::Offer { mime_type } => {
                if let Some(offer) = state.offers.get_mut(&proxy.id()) {
                    offer.mime_types.insert(mime_type);
                }
            }
            _ => {}
        }
    }
}

pub fn main(socket_path: &PathBuf) -> eyre::Result<()> {
    let _ = std::fs::remove_file(&socket_path); // lol
    let socket = UnixListener::bind(&socket_path)
        .wrap_err_with(|| format!("binding path {}", socket_path.display()))?;

    let conn =
        wayland_client::Connection::connect_to_env().wrap_err("connecting to the compositor")?;

    let (globals, mut queue) =
        registry_queue_init::<WlState>(&conn).wrap_err("initializing wayland connection")?;

    let data_manager = globals.bind::<ExtDataControlManagerV1, _, _>(&queue.handle(), 1..=1, ()).wrap_err("getting ext_data_control_manager_v1, is ext-data-control-v1 not supported by the compositor?")?;

    let seat = globals
        .bind::<WlSeat, _, _>(&queue.handle(), 1..=1, ())
        .wrap_err("getting seat")?;

    let data_device = data_manager.get_data_device(&seat, &queue.handle(), ());

    let history_state = Arc::new(HistoryState {
        next_item_id: Arc::new(AtomicU64::new(0)),
        last_copied_item_id: Arc::new(AtomicU64::new(u64::MAX)),
        items: Arc::new(Mutex::new(Vec::<HistoryItem>::new())),
        // for deduplication because the event stream will tell us that we just copied something :)
    });

    let history_state2 = history_state.clone();
    std::thread::spawn(move || {
        let mut state = WlState {
            offers: HashMap::new(),
            current_primary_selection: None,
            current_selection: None,

            history_state: history_state2,
        };
        loop {
            queue.blocking_dispatch(&mut state);
        }
    });

    println!("INFO: Listening on {}", socket_path.display());

    for peer in socket.incoming() {
        match peer {
            Ok(peer) => {
                let history_state = history_state.clone();
                std::thread::spawn(move || {
                    let result = handle_peer(peer, &history_state);
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

fn handle_peer(mut peer: UnixStream, history_state: &HistoryState) -> eyre::Result<()> {
    let mut request = [0; 1];
    let Ok(()) = peer.read_exact(&mut request) else {
        return Ok(());
    };
    match request[0] {
        super::MESSAGE_STORE => {
            handle_store(history_state).wrap_err("handling store message")?;
        }
        super::MESSAGE_READ => {
            let items = history_state.items.lock().unwrap();

            ciborium::into_writer(items.as_slice(), BufWriter::new(peer))
                .wrap_err("writing items to socket")?;
        }
        super::MESSAGE_COPY => {
            handle_copy(peer, history_state).wrap_err("handling copy message")?;
        }
        _ => {}
    };
    Ok(())
}

fn handle_copy(mut peer: UnixStream, history_state: &HistoryState) -> Result<(), eyre::Error> {
    let mut id = [0; 8];
    peer.read_exact(&mut id).wrap_err("failed to read id")?;
    let id = u64::from_le_bytes(id);
    let mut items = history_state.items.lock().unwrap();
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
    history_state
        .last_copied_item_id
        .store(entry.id, std::sync::atomic::Ordering::Relaxed);
    if let Err(err) = result {
        println!("WARNING: Copy failed: {err:?}");
    }
    Ok(())
}

fn handle_store(history_state: &HistoryState) -> Result<(), eyre::Error> {
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

    do_read_clipboard_into_history(&history_state, time, mime.to_string(), data_readear)?;
    Ok(())
}

fn do_read_clipboard_into_history(
    history_state: &HistoryState,
    time: std::time::Duration,
    mime: String,
    data_reader: impl Read,
) -> Result<(), eyre::Error> {
    let mut data_reader = data_reader.take(MAX_ENTRY_SIZE);
    let mut data = Vec::new();
    data_reader
        .read_to_end(&mut data)
        .wrap_err("reading content data")?;
    let new_entry = HistoryItem {
        id: history_state
            .next_item_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        mime: mime.to_string(),
        data,
        created_time: u64::try_from(time.as_millis()).unwrap(),
    };
    let mut items = history_state.items.lock().unwrap();
    if items
        .last()
        .is_some_and(|last| last.mime == new_entry.mime && last.data == new_entry.data)
    {
        println!("INFO: Skipping store of new item because it is identical to last one");
        return Ok(());
    }
    let last_copied = history_state
        .last_copied_item_id
        .load(std::sync::atomic::Ordering::Relaxed);
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
        running_total += item.data.len() + std::mem::size_of::<HistoryItem>();
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
