use gst::prelude::*;
use gstreamer as gst;
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Debug)]
pub struct ElementTimer {
    name: String,
    pending: Arc<Mutex<Vec<Instant>>>,
    sink: gst::Pad,
    source: gst::Pad,
    sink_id: Option<gst::PadProbeId>,
    source_id: Option<gst::PadProbeId>,
}

impl ElementTimer {
    const MAX_PENDING: usize = 100;

    pub fn new(name: &str, sink_element: gst::Element, source_element: gst::Element) -> Self {
        let pending = Arc::new(Mutex::new(Vec::new()));
        let probe_mask: gst::PadProbeType =
            gst::PadProbeType::PUSH | gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST;

        // Assume we want to measure between sink/source
        let sink = sink_element
            .get_sink_pads()
            .pop()
            .expect("Failed to get sink pad");
        let source = source_element
            .get_src_pads()
            .pop()
            .expect("Faile to get source pad");

        let clone = Arc::clone(&pending);
        // Install push-buffer probes on the pads.
        let sink_id = sink.add_probe(probe_mask, move |_pad, _info| {
            if let Ok(mut pending) = clone.lock() {
                if pending.len() < Self::MAX_PENDING {
                    pending.push(Instant::now());
                } else {
                    log::warn!("Pending timer messages exeeded max");
                }
                gst::PadProbeReturn::Ok
            } else {
                log::error!("Failed to lock mutex, removing sink probe");
                gst::PadProbeReturn::Remove
            }
        });

        let prefix = format!("== {}", name);
        let clone = Arc::clone(&pending);
        let source_id = source.add_probe(probe_mask, move |_pad, _info| {
            if let Ok(mut pending) = clone.lock() {
                log::trace!("== Pending items: {}", pending.len());

                if let Some(start) = pending.pop() {
                    log::trace!("{} - {:#?}", prefix, start.elapsed());
                }
                gst::PadProbeReturn::Ok
            } else {
                log::error!("Failed to lock mutex, removing source probe");
                gst::PadProbeReturn::Remove
            }
        });

        Self {
            name: name.to_owned(),
            pending,
            sink,
            source,
            sink_id,
            source_id,
        }
    }
}

impl Drop for ElementTimer {
    fn drop(&mut self) {
        if let Some(id) = self.sink_id.take() {
            log::debug!("Removing sink probe");
            self.sink.remove_probe(id);
        }
        if let Some(id) = self.source_id.take() {
            log::debug!("Removing source probe");
            self.source.remove_probe(id);
        }
    }
}
