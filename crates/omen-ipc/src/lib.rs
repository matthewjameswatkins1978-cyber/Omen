pub mod codec;
pub mod endpoint;
pub mod error;
pub mod handshake;
pub mod protocol;
pub mod transport;

pub use codec::{MAX_FRAME_SIZE, read_frame, read_json_frame, write_frame, write_json_frame};
pub use endpoint::default_endpoint_address;
pub use error::LocalIpcError;
pub use handshake::{
    ClientHello, DaemonHello, PROTOCOL_FAMILY, PROTOCOL_VERSION_1, SUPPORTED_PROTOCOL_VERSIONS,
    negotiate_protocol_version,
};
pub use protocol::*;
pub use transport::{PlatformListener, PlatformStream};
