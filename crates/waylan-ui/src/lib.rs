pub mod candidate_format;
pub mod renderer;
pub mod theme;

pub use candidate_format::{format_candidate_display, format_candidate_index, CIRCLED_NUMBERS};
pub use renderer::layout::{CandidateHitTarget, HitRegion};
pub use renderer::CandidateBoxRenderer;
pub use theme::{Color, UiTheme};
