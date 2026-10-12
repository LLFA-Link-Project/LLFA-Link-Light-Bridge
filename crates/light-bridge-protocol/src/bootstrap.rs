//! WiVRn/UDS automatic-setup envelope, independent of the pose/control TCP v3 codec.
use light_bridge_usbip::{
    bootstrap::{Packet, MAX_PACKET, VERSION},
    kernel::KResult,
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wire {
    pub version: u32,
    pub packet: Packet,
}
impl Wire {
    pub fn new(packet: Packet) -> KResult<Self> {
        packet.validate()?;
        Ok(Self {
            version: VERSION,
            packet,
        })
    }
    pub fn validate(self) -> KResult<Packet> {
        if self.version != VERSION {
            return Err("bootstrap wire version unsupported".into());
        }
        self.packet.validate()?;
        Ok(self.packet)
    }
}
pub fn encode(packet: Packet) -> KResult<Vec<u8>> {
    let bytes = serde_json::to_vec(&Wire::new(packet)?).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_PACKET {
        return Err("bootstrap packet too large".into());
    }
    Ok(bytes)
}
pub fn decode(bytes: &[u8]) -> KResult<Packet> {
    if bytes.is_empty() || bytes.len() > MAX_PACKET {
        return Err("bootstrap packet size invalid".into());
    }
    let wire: Wire = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    wire.validate()
}
