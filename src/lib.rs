pub mod daemon;
pub mod display;

use eyre::OptionExt;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{path::PathBuf, sync::Arc};

const MAX_ENTRY_SIZE: u64 = 50_000_000;
const MAX_HISTORY_BYTE_SIZE: usize = 100_000_000;

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct HistoryItem {
    id: u64,
    mime: String,
    #[serde(
        deserialize_with = "deserialize_data",
        serialize_with = "serialize_data"
    )]
    data: Arc<[u8]>,
    created_time: u64,
}

fn deserialize_data<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<[u8]>, D::Error> {
    Box::<[u8]>::deserialize(deserializer).map(Into::into)
}

fn serialize_data<S: Serializer>(data: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error> {
    let data: &[u8] = data;
    data.serialize(serializer)
}

const MESSAGE_READ: u8 = 1;
/// Argument: One u64-bit LE value, the ID
const MESSAGE_COPY: u8 = 2;

pub fn socket_path() -> eyre::Result<PathBuf> {
    Ok(dirs::runtime_dir()
        .ok_or_eyre("missing XDG_RUNTIME_DIR")?
        .join("clippyboard.sock"))
}
