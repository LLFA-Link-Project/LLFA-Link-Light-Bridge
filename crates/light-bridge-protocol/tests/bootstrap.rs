#![cfg(target_os = "linux")]
use light_bridge_protocol::bootstrap::{self, Wire};
use light_bridge_usbip::bootstrap::{Packet, Stamp};
#[test]
fn strict_bootstrap_schema_version_truncation_duplicates_and_replay_identity() {
    let packet = Packet::Abort {
        stamp: Stamp {
            instance: 1,
            session: 2,
            host_id: 3,
            generation: 4,
        },
        reason: "cancel".into(),
    };
    let bytes = bootstrap::encode(packet).unwrap();
    assert!(matches!(
        bootstrap::decode(&bytes).unwrap(),
        Packet::Abort { .. }
    ));
    for n in 0..bytes.len() {
        assert!(bootstrap::decode(&bytes[..n]).is_err());
    }
    let text = String::from_utf8(bytes).unwrap();
    assert!(bootstrap::decode(text.replace("\"version\":1", "\"version\":2").as_bytes()).is_err());
    assert!(bootstrap::decode(
        text.replace("\"version\":1", "\"version\":0,\"version\":1")
            .as_bytes()
    )
    .is_err());
    assert!(
        bootstrap::decode(text.replace("\"instance\":1", "\"instance\":0").as_bytes()).is_err()
    );
    let invalid = Wire {
        version: 1,
        packet: Packet::Select {
            stamp: Stamp {
                instance: 1,
                session: 2,
                host_id: 3,
                generation: 4,
            },
            serial: "\0".into(),
        },
    };
    assert!(invalid.validate().is_err());
}
