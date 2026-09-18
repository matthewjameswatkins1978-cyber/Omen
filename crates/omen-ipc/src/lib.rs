pub mod codec;
pub mod error;
pub mod handshake;
pub mod protocol;

pub use codec::{read_frame, read_json_frame, write_frame, write_json_frame, MAX_FRAME_SIZE};
pub use error::LocalIpcError;
pub use handshake::{
    negotiate_protocol_version, ClientHello, DaemonHello, PROTOCOL_FAMILY, PROTOCOL_VERSION_1,
    SUPPORTED_PROTOCOL_VERSIONS,
};
pub use protocol::*;
