use super::HistoryItem;
use super::MAX_ENTRY_SIZE;
use eframe::egui::ahash::HashSet;
use eyre::Context;
use eyre::ContextCompat;
use eyre::bail;
use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::fs::OFlags;
use rustix::fs::fcntl_setfl;
use rustix::io::FdFlags;
use std::collections::HashMap;
use std::future::poll_fn;
use std::io::PipeReader;
use std::io::{BufReader, BufWriter, PipeWriter, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, atomic::AtomicU64};
use std::time::Duration;
use std::time::SystemTime;
use tracing::error;
use tracing::info;
use tracing::warn;
use tracing_subscriber::EnvFilter;
use wayland_client::EventQueue;
use wayland_client::protocol::wl_callback::WlCallback;
use wayland_client::protocol::wl_display::WlDisplay;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Dispatch, Proxy, QueueHandle, backend::ObjectId, event_created_child};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::{
    EVT_DATA_OFFER_OPCODE, ExtDataControlDeviceV1,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1::ExtDataControlManagerV1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::ExtDataControlOfferV1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::ExtDataControlSourceV1;

struct SharedState {
    next_item_id: AtomicU64,
    // for deduplication because the event stream will tell us that we just copied something :)
    last_copied_item_id: AtomicU64,
    items: Mutex<Vec<HistoryItem>>,
    notify_write_send: PipeWriter,

    data_control_manager: OnceLock<ExtDataControlManagerV1>,
    data_control_devices: Mutex<HashMap</*seat global name */ u32, ExtDataControlDeviceV1>>,
    qh: QueueHandle<WlState>,
    d: WlDisplay,
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
    shared_state: Arc<SharedState>,

    /// wl_seat that arrived before the data control manager so we weren't able to grab their device immediatly.
    deferred_seats: Vec<WlSeat>,

    offers: HashMap<ObjectId, InProgressOffer>,
    current_primary_selection: Option<CurrentSelection>,
    current_selection: Option<CurrentSelection>,
}

impl Dispatch<WlCallback, String> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &WlCallback,
        _event: <WlCallback as Proxy>::Event,
        data: &String,
        _conn: &wayland_client::Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        info!("Received sync back {data}");
    }
}

impl Dispatch<WlRegistry, ()> for WlState {
    fn event(
        state: &mut Self,
        proxy: &WlRegistry,
        event: <WlRegistry as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            wayland_client::protocol::wl_registry::Event::Global {
                name,
                interface,
                version: _, // we only need version 1
            } => {
                if interface == WlSeat::interface().name {
                    info!("A new seat was connected");
                    let seat: WlSeat = proxy.bind(name, 1, qhandle, ());

                    match state.shared_state.data_control_manager.get() {
                        None => {
                            state.deferred_seats.push(seat);
                        }
                        Some(manager) => {
                            let device = manager.get_data_device(&seat, qhandle, ());
                            state
                                .shared_state
                                .data_control_devices
                                .lock()
                                .unwrap()
                                .insert(name, device);
                        }
                    }
                } else if interface == ExtDataControlManagerV1::interface().name {
                    let manager: ExtDataControlManagerV1 = proxy.bind(name, 1, qhandle, ());

                    for seat in state.deferred_seats.drain(..) {
                        let device = manager.get_data_device(&seat, qhandle, ());
                        state
                            .shared_state
                            .data_control_devices
                            .lock()
                            .unwrap()
                            .insert(name, device);
                    }

                    state
                        .shared_state
                        .data_control_manager
                        .set(manager)
                        .expect("ext_data_control_manager_v1 already set, global appeared twice?");
                }
            }
            wayland_client::protocol::wl_registry::Event::GlobalRemove { name } => {
                // try to remove, if it's not a wl_seat it may not exist
                state
                    .shared_state
                    .data_control_devices
                    .lock()
                    .unwrap()
                    .remove(&name);
            }
            _ => {}
        }
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

                if let Some(offer) = state.current_selection.take() {
                    let Some(mime) = ["text/plain", "image/png"]
                        .iter()
                        .find(|mime| offer.mime_types.contains(**mime))
                    else {
                        warn!(
                            "No supported mime type found. Found mime types: {:?}",
                            offer.mime_types
                        );
                        return;
                    };

                    let (reader, writer) = std::io::pipe().unwrap();
                    offer.offer.receive(mime.to_string(), writer.as_fd());

                    let history_state = state.shared_state.clone();
                    let mime = mime.to_string();
                    let time = offer.time;
                    std::thread::spawn(move || {
                        let result =
                            do_read_clipboard_into_history(&history_state, time, mime, reader);
                        if let Err(err) = result {
                            warn!("Failed to read clipboard: {:?}", err)
                        }

                        offer.offer.destroy();
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
            ext_data_control_device_v1::Event::Finished => {
                warn!("device finished :(");
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

impl Dispatch<ExtDataControlSourceV1, OfferData> for WlState {
    fn event(
        _state: &mut Self,
        proxy: &ExtDataControlSourceV1,
        event: <ExtDataControlSourceV1 as Proxy>::Event,
        data: &OfferData,
        _conn: &wayland_client::Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type: _, fd } => {
                info!("pasting {:?}", std::str::from_utf8(&data.0));

                let data = data.0.clone();

                std::thread::spawn(move || {
                    let mut writer = BufWriter::new(PipeWriter::from(fd));

                    let result = writer.write_all(&data);
                    if let Err(err) = result {
                        warn!("Failed to write to requester: {:?}", err);
                    }
                    let result = writer.into_inner();
                    if let Err(err) = result {
                        warn!("Failed to write to requester: {:?}", err);
                    }
                });
            }
            ext_data_control_source_v1::Event::Cancelled => {
                info!("We have been replaced.");
                proxy.destroy();
            }
            _ => {}
        }
    }
}

pub fn main(socket_path: &PathBuf) -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or(EnvFilter::new("info")))
        .init();

    let _ = std::fs::remove_file(&socket_path); // lol
    let socket = UnixListener::bind(&socket_path)
        .wrap_err_with(|| format!("binding path {}", socket_path.display()))?;

    let conn =
        wayland_client::Connection::connect_to_env().wrap_err("connecting to the compositor")?;

    let mut queue = conn.new_event_queue::<WlState>();

    let (notify_write_recv, notify_write_send) = std::io::pipe().expect("todo");

    let shared_state = Arc::new(SharedState {
        next_item_id: AtomicU64::new(0),
        last_copied_item_id: AtomicU64::new(u64::MAX),
        items: Mutex::new(Vec::<HistoryItem>::new()),
        notify_write_send,

        data_control_manager: OnceLock::new(),
        data_control_devices: Mutex::new(HashMap::new()),
        qh: queue.handle(),
        d: conn.display(),
    });

    shared_state.items.lock().unwrap().push(HistoryItem {
        id: 3548235782,
        mime: "text/plain".into(),
        data: b"meow".to_vec().into(),
        created_time: 0,
    });

    let history_state2 = shared_state.clone();

    let mut wl_state = WlState {
        offers: HashMap::new(),
        current_primary_selection: None,
        current_selection: None,
        deferred_seats: Vec::new(),

        shared_state: history_state2,
    };

    conn.display().get_registry(&queue.handle(), ());

    queue
        .roundtrip(&mut wl_state)
        .wrap_err("failed to set up wayland state")?;

    if wl_state.shared_state.data_control_manager.get().is_none() {
        bail!(
            "{} not found, the ext-data-control-v1 Wayland extension is likely unsupported by your compositor.\n\
            check https://wayland.app/protocols/ext-data-control-v1#compositor-support\
            ",
            ExtDataControlManagerV1::interface().name
        );
    }

    rustix::fs::fcntl_setfl(notify_write_recv.as_fd(), OFlags::NONBLOCK).expect("todo");
    rustix::fs::fcntl_setfl(conn.as_fd(), OFlags::NONBLOCK).expect("TODO");

    std::thread::spawn(move || {
        if let Err(err) = dispatch_wayland(queue, wl_state, notify_write_recv) {
            error!("error on Wayland thread: {err:?}");
            std::process::exit(1);
        }
    });

    info!("Listening on {}", socket_path.display());

    for peer in socket.incoming() {
        match peer {
            Ok(peer) => {
                let history_state = shared_state.clone();
                std::thread::spawn(move || {
                    let result = handle_peer(peer, &history_state);
                    if let Err(err) = result {
                        warn!("Error handling peer: {err:?}");
                    }
                });
            }
            Err(err) => {
                warn!("Error accepting peer: {err}");
            }
        }
    }

    Ok(())
}

fn dispatch_wayland(
    mut queue: EventQueue<WlState>,
    mut wl_state: WlState,
    notify_write_recv: PipeReader,
) -> eyre::Result<()> {
    loop {
        queue
            .dispatch_pending(&mut wl_state)
            .wrap_err("dispatching Wayland events")?;

        let read_guard = queue
            .prepare_read()
            .wrap_err("preparing read from Wayland socket")?;
        let _ = queue.flush();

        let pollfd1_read = PollFd::from_borrowed_fd(read_guard.connection_fd(), PollFlags::IN);
        let pollfd_signal = PollFd::from_borrowed_fd(notify_write_recv.as_fd(), PollFlags::IN);

        let _ = rustix::event::poll(&mut [pollfd1_read, pollfd_signal], None);

        read_guard
            .read_without_dispatch()
            .wrap_err("reading from wayland socket")?;
    }
}

#[tracing::instrument(skip(peer, shared_state), level = "info")]
fn handle_peer(mut peer: UnixStream, shared_state: &SharedState) -> eyre::Result<()> {
    let mut request = [0; 1];
    let Ok(()) = peer.read_exact(&mut request) else {
        return Ok(());
    };
    match request[0] {
        super::MESSAGE_READ => {
            let items = shared_state.items.lock().unwrap();

            ciborium::into_writer(items.as_slice(), BufWriter::new(peer))
                .wrap_err("writing items to socket")?;
        }
        super::MESSAGE_COPY => {
            handle_copy(peer, shared_state).wrap_err("handling copy message")?;
        }
        _ => {}
    };
    Ok(())
}

struct OfferData(Arc<[u8]>);

fn handle_copy(mut peer: UnixStream, shared_state: &SharedState) -> Result<(), eyre::Error> {
    let mut id = [0; 8];
    peer.read_exact(&mut id).wrap_err("failed to read id")?;
    let id = u64::from_le_bytes(id);
    let mut items = shared_state.items.lock().unwrap();
    let Some(idx) = items.iter().position(|item| item.id == id) else {
        return Ok(());
    };
    let item = items.remove(idx);
    items.push(item.clone());

    drop(items);

    do_copy_into_clipboard(item, &shared_state).wrap_err("doing copy")?;

    (&shared_state.notify_write_send)
        .write_all(&[0])
        .wrap_err("notifying wayland thread")?;

    Ok(())
}

fn do_copy_into_clipboard(
    entry: HistoryItem,
    shared_state: &SharedState,
) -> Result<(), eyre::Error> {
    for device in &*shared_state.data_control_devices.lock().unwrap() {
        shared_state
            .d
            .sync(&shared_state.qh, "before create_data_source".into());
        let data_source = shared_state
            .data_control_manager
            .get()
            .expect("data manger not found")
            .create_data_source(&shared_state.qh, OfferData(entry.data.clone()));

        shared_state
            .d
            .sync(&shared_state.qh, "after create_data_source".into());

        if entry.mime == "text/plain" {
            // Just like wl_clipboard_rs, we also offer some extra mimes for text.
            let text_mimes = [
                "text/plain;charset=utf-8",
                "text/plain",
                "STRING",
                "UTF8_STRING",
                "TEXT",
            ];
            for mime in text_mimes {
                data_source.offer(mime.to_string());
            }
        } else {
            data_source.offer(entry.mime.clone());
        }

        shared_state
            .d
            .sync(&shared_state.qh, "before set_selection".into());

        info!("setting the selection");

        device.1.set_selection(Some(&data_source));

        shared_state
            .d
            .sync(&shared_state.qh, "setting the selection".into());
    }

    shared_state
        .last_copied_item_id
        .store(entry.id, std::sync::atomic::Ordering::Relaxed);

    Ok(())
}

fn do_read_clipboard_into_history(
    history_state: &SharedState,
    time: std::time::Duration,
    mime: String,
    data_reader: impl Read,
) -> Result<(), eyre::Error> {
    let mut data_reader = BufReader::new(data_reader).take(MAX_ENTRY_SIZE);
    let mut data = Vec::new();
    data_reader
        .read_to_end(&mut data)
        .wrap_err("reading content data")?;
    let new_entry = HistoryItem {
        id: history_state
            .next_item_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        mime: mime.to_string(),
        data: data.into(),
        created_time: u64::try_from(time.as_millis()).unwrap(),
    };
    let mut items = history_state.items.lock().unwrap();
    if items
        .last()
        .is_some_and(|last| last.mime == new_entry.mime && last.data == new_entry.data)
    {
        info!("INFO: Skipping store of new item because it is identical to last one");
        return Ok(());
    }
    let last_copied = history_state
        .last_copied_item_id
        .load(std::sync::atomic::Ordering::Relaxed);
    if let Some(item) = items.iter().find(|item| item.id == last_copied)
        && item.mime == new_entry.mime
        && item.data == new_entry.data
    {
        info!("Skipping store of new item because the copy came from us");
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
        info!(
            "Dropping old {} items because limit of {} bytes was reached for the history",
            cutoff + 1,
            crate::MAX_HISTORY_BYTE_SIZE
        );
        items.splice(0..=cutoff, []);
    }
    info!(
        "Successfully stored clipboard value of mime type {mime} (new history size {running_total})"
    );
    Ok(())
}
