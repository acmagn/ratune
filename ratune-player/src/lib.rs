pub mod adts_normalize;
pub mod engine;
pub mod queue;
pub mod stream;
pub mod tap;
pub mod track;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub type SampleBuffer = Arc<Mutex<VecDeque<f32>>>;

pub use engine::{spawn_player, DownloadedTrack, PlayerCommand, PlayerEvent};
pub use tap::SampleTap;
