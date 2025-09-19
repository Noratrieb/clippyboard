use std::{path::PathBuf, sync::Arc};

use eyre::OptionExt;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct HistoryItem {
    pub id: u64,
    pub mime: String,
    #[serde(
        deserialize_with = "deserialize_data",
        serialize_with = "serialize_data"
    )]
    pub data: Arc<[u8]>,
    pub created_time: u64,
}

fn deserialize_data<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<[u8]>, D::Error> {
    Box::<[u8]>::deserialize(deserializer).map(Into::into)
}

fn serialize_data<S: Serializer>(data: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error> {
    let data: &[u8] = data;
    data.serialize(serializer)
}

pub const MESSAGE_READ: u8 = 1;
/// Argument: One u64-bit LE value, the ID
pub const MESSAGE_COPY: u8 = 2;
pub const MESSAGE_CLEAR: u8 = 3;

pub fn socket_path() -> eyre::Result<PathBuf> {
    if let Some(path) = std::env::var_os("CLIPPYBOARD_SOCKET") {
        return Ok(path.into());
    }

    Ok(dirs::runtime_dir()
        .ok_or_eyre("missing XDG_RUNTIME_DIR")?
        .join("clippyboard.sock"))
}
