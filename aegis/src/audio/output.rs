//! Speaker output side: holds the rodio DeviceSink open for the lifetime of
//! the app so each turn doesn't pay the ~10-30ms cost of negotiating with
//! the OS audio system. Hand out a fresh `Player` per turn via `new_player()`
//! (cheap; just wires up to the existing sink's mixer).
//!
//! Must be initialized on the thread that will own it (rodio's internals may
//! not be Send).

pub struct AudioOutput {
    sink: rodio::MixerDeviceSink,
    pub channels: std::num::NonZeroU16,
    pub sample_rate: std::num::NonZeroU32,
}

impl AudioOutput {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        let sink = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
        eprintln!("[audio] output sink opened");
        Ok(AudioOutput {
            sink,
            channels: std::num::NonZeroU16::new(crate::providers::tts_cartesia::STREAM_CHANNELS)
                .expect("STREAM_CHANNELS must be non-zero"),
            sample_rate: std::num::NonZeroU32::new(
                crate::providers::tts_cartesia::STREAM_SAMPLE_RATE,
            )
            .expect("STREAM_SAMPLE_RATE must be non-zero"),
        })
    }

    /// Hand out a fresh Player attached to the cached sink. Cheap.
    pub fn new_player(&self) -> rodio::Player {
        rodio::Player::connect_new(self.sink.mixer())
    }
}
