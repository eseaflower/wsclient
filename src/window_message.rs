use crate::message::CaseMeta;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ViewSample {
    pub sample: gstreamer::Sample,
    pub id: usize,
}

#[derive(Debug, Clone)]
pub enum WindowMessage {
    Cases(Vec<CaseMeta>),
    PipelineError,
    Timer(Duration),
    Sample(usize),
    Datachannel(gstreamer_webrtc::WebRTCDataChannel),
    UpdateLayout,
}
