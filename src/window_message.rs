use crate::message::CaseMeta;
use std::time::Duration;

#[derive(Debug)]
pub enum WindowMessage {
    Cases(Vec<CaseMeta>),
    NewSample,
    PipelineError,
    Timer(Duration),
}
