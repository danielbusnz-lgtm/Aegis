//! Audio I/O. Mic input on one side, speaker output on the other; the two
//! sides share no state and are kept in separate modules. Re-exported flatly
//! so callers use `audio::Mic`, `audio::AudioOutput`, etc.

mod input;
mod output;

// test_stt only uses the input side, so the re-export looks unused there.
// The main aegis binary uses both.
#[allow(unused_imports)]
pub use input::*;
#[allow(unused_imports)]
pub use output::*;
