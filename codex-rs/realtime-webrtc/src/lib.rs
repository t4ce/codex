#[cfg(feature = "voice")]
mod client;
#[cfg(feature = "voice")]
mod helper_exit;
#[cfg(all(feature = "voice", any(target_os = "linux", test)))]
mod linux_alsa;
#[cfg(feature = "voice")]
mod message_reader;
mod protocol;
#[cfg(feature = "voice")]
mod session;
#[cfg(not(feature = "voice"))]
mod voice_disabled;

#[cfg(feature = "voice")]
pub use client::ConnectionError;
#[cfg(feature = "voice")]
pub use client::VoiceHost;
#[cfg(feature = "voice")]
pub use helper_exit::HelperExitStage;
pub use protocol::AudioControls;
pub use protocol::AudioDevice;
pub use protocol::AudioDeviceKind;
pub use protocol::AudioDeviceSelection;
pub use protocol::AudioState;
pub use protocol::MAX_FRAME_BYTES;
pub use protocol::Message;
pub use protocol::RUNTIME_ENVIRONMENT;
pub use protocol::SessionDescription;
pub use protocol::decode_frame;
pub use protocol::encode_frame;
pub use protocol::read_message;
#[cfg(feature = "voice")]
pub use session::RealtimeWebrtcSession;
#[cfg(feature = "voice")]
pub use session::RealtimeWebrtcSessionHandle;
#[cfg(feature = "voice")]
pub use session::StartedRealtimeWebrtcSession;
#[cfg(not(feature = "voice"))]
pub use voice_disabled::{
    ConnectionError, RealtimeWebrtcSession, RealtimeWebrtcSessionHandle,
    StartedRealtimeWebrtcSession, VoiceHost,
};
