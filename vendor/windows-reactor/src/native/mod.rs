use super::*;
use crate::core::*;

pub(crate) enum FeedbackExpectation {
    Exact(EventPayload),
    Normalized { observation: Option<QueuedEvent> },
    Suppressed,
}

mod winui;

#[cfg(feature = "test")]
pub use winui::test::{
    schedule_live_test_exit, subscribe_live_rendering, verify_synchronous_editor_scale,
};
pub use winui::*;
