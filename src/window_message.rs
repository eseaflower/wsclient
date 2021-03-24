use gstreamer_webrtc as gst_webrtc;

use crate::message::CaseMeta;

#[derive(Debug)]
pub enum WindowMessage {
    Cases(Vec<CaseMeta>),
    NewSample,
    ContextShared,
    PipelineError,
}