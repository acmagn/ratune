pub mod engine;
pub mod queue;
pub mod scrobble;
pub mod stream;
pub mod tap;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub type SampleBuffer = Arc<Mutex<VecDeque<f32>>>;

pub use engine::{spawn_player, PlayerCommand, PlayerEvent};
pub use tap::SampleTap;
