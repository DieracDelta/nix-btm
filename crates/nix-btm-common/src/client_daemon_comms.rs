use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

use crate::handle_internal_json::{Drv, DrvParseError};

// needed because drv serialization is already done differently to accomodate
// for json. Don't need that for cbor
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum DrvWire {
    Str(String),
    Parts { hash: String, name: String },
}

impl From<Drv> for DrvWire {
    fn from(d: Drv) -> Self {
        DrvWire::Parts {
            hash: d.hash,
            name: d.name,
        }
    }
}
impl TryFrom<DrvWire> for Drv {
    type Error = DrvParseError;

    fn try_from(w: DrvWire) -> Result<Self, Self::Error> {
        match w {
            DrvWire::Str(s) => s.parse(), // via FromStr
            DrvWire::Parts { hash, name } => Ok(Drv { hash, name }),
        }
    }
}
