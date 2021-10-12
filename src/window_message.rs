use glutin::event::Event;

use crate::message::{CaseMeta, Protocols};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ViewSample {
    pub sample: gstreamer::Sample,
    pub id: usize,
    pub timer: std::time::Instant,
}

#[derive(Debug, Clone)]
pub enum WindowMessage {
    Cases((Option<Protocols>, Vec<CaseMeta>)),
    PipelineError,
    Timer(Duration),
    Sample(usize),
    Datachannel(gstreamer_webrtc::WebRTCDataChannel),
    UpdateLayout,
    JitterStats,
}

impl<'a> Into<Event<'a, WindowMessage>> for WindowMessage {
    fn into(self) -> Event<'a, WindowMessage> {
        Event::UserEvent(self)
    }
}
