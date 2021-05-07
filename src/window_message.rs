use crate::message::CaseMeta;
use std::time::Duration;

#[derive(Debug)]
pub enum WindowMessage {
    Cases(Vec<CaseMeta>),
    PipelineError,
    Timer(Duration),
    Redraw(usize),
}
