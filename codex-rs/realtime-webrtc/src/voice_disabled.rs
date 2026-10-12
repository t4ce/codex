//! API-compatible voice stubs for builds without the native media runtime.

use crate::AudioDevice;
use crate::AudioDeviceKind;
use crate::AudioDeviceSelection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionError {
    NegotiationTimedOut,
    Failed,
    HelperStartup,
    RuntimeInitialization,
    Transport,
    AudioDevices,
    AudioControls,
    AudioSession,
    Shutdown,
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("voice support is disabled in this build; rebuild with the `voice` feature")
    }
}

impl std::error::Error for ConnectionError {}

pub struct VoiceHost;

impl VoiceHost {
    pub async fn connect<T>(_: &T, _: &str) -> anyhow::Result<Self> {
        anyhow::bail!("voice support is disabled in this build; rebuild with the `voice` feature")
    }

    pub async fn list_devices(&mut self, _: AudioDeviceKind) -> anyhow::Result<Vec<AudioDevice>> {
        anyhow::bail!("voice support is disabled in this build; rebuild with the `voice` feature")
    }

    pub async fn close(self) -> anyhow::Result<()> {
        anyhow::bail!("voice support is disabled in this build; rebuild with the `voice` feature")
    }
}

pub struct RealtimeWebrtcSession;

impl RealtimeWebrtcSession {
    pub fn is_supported() -> bool {
        false
    }

    pub fn start<T>(_: T, _: AudioDeviceSelection) -> anyhow::Result<StartedRealtimeWebrtcSession> {
        anyhow::bail!("voice support is disabled in this build; rebuild with the `voice` feature")
    }
}

#[derive(Clone, Debug)]
pub struct RealtimeWebrtcSessionHandle;

impl RealtimeWebrtcSessionHandle {
    pub fn close(&self) {}

    pub fn apply_answer_sdp(&self, _: String) -> Result<(), ConnectionError> {
        Err(ConnectionError::Failed)
    }

    pub fn set_microphone_muted(&self, _: bool) -> Result<(), ConnectionError> {
        Err(ConnectionError::Failed)
    }

    pub fn set_speaker_suppressed(&self, _: bool) {}

    pub fn take_microphone_peak(&self) -> u16 {
        0
    }

    pub fn take_speaker_peak(&self) -> u16 {
        0
    }

    pub fn take_error(&self) -> Option<(ConnectionError, String)> {
        Some((ConnectionError::Failed, ConnectionError::Failed.to_string()))
    }
}

pub struct StartedRealtimeWebrtcSession {
    pub offer_sdp: String,
    pub handle: RealtimeWebrtcSessionHandle,
}

impl std::fmt::Debug for StartedRealtimeWebrtcSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StartedRealtimeWebrtcSession { offer_sdp: [REDACTED] }")
    }
}

#[cfg(test)]
mod tests {
    use super::AudioDeviceSelection;
    use super::RealtimeWebrtcSession;

    #[test]
    fn disabled_voice_runtime_rejects_session_start() {
        assert!(!RealtimeWebrtcSession::is_supported());
        assert!(RealtimeWebrtcSession::start((), AudioDeviceSelection::default()).is_err());
    }
}
