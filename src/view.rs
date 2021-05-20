use std::{
    convert::TryFrom,
    ops::Deref,
    sync::{Arc, Mutex},
};

use glutin::{
    dpi::PhysicalPosition,
    event::{ElementState, MouseScrollDelta, VirtualKeyCode, WindowEvent},
};
use gstreamer as gst;
use gstreamer_video as gst_video;
use gstreamer_webrtc as gst_webrtc;

use crate::{
    interaction::InteractionState,
    message::{
        CaseMeta, ClientConfig, DataMessage, LayoutCfg, LayoutRect, PaneState, Protocols,
        RenderState, ViewportSize,
    },
    view,
    window_message::ViewSample,
    AppConfig,
};

fn tile(view_size: (u32, u32), rows: usize, columns: usize) -> Vec<LayoutRect> {
    // Align to 4 pixels
    let view_width = view_size.0 as f32 / columns as f32;
    let view_width = ((view_width / 4_f32).floor() * 4_f32) as u32;
    let view_height = view_size.1 as f32 / rows as f32;
    let view_height = ((view_height / 4_f32).floor() * 4_f32) as u32;

    let mut layouts = Vec::with_capacity(rows * columns);
    for y_idx in 0..rows as u32 {
        for x_idx in 0..columns as u32 {
            layouts.push(LayoutRect {
                x: x_idx * view_width,
                y: y_idx * view_height,
                width: view_width,
                height: view_height,
            });
        }
    }
    layouts
}

#[derive(Debug)]
pub struct Pane {
    layout: LayoutRect,
    interaction: InteractionState,
    dirty: bool,
    case_key: Option<String>,
}

impl Default for Pane {
    fn default() -> Self {
        Pane {
            layout: LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            interaction: InteractionState::new(),
            dirty: false,
            case_key: None,
        }
    }
}

impl Pane {
    pub fn handle_window_event(&mut self, event: &WindowEvent) -> bool {
        let layout_position = PhysicalPosition::new(self.layout.x as f64, self.layout.y as f64);

        match event {
            WindowEvent::CursorMoved { position, .. } => {
                // Translate positions relative to the top left corner.
                let translated = PhysicalPosition::new(
                    position.x - layout_position.x as f64,
                    position.y - layout_position.y as f64,
                );
                self.interaction.handle_move(translated);
                true
            }
            WindowEvent::MouseInput { button, state, .. } => {
                self.interaction.handle_mouse_input(*button, *state);
                true
            }
            WindowEvent::ModifiersChanged(state) => {
                self.interaction.handle_modifiers(*state);
                true
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let delta = match *delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                self.interaction.handle_mouse_wheel(delta);
                true
            }
            _ => false,
        }
    }

    pub fn hide_cursor(&self) -> bool {
        self.interaction.hide_cursor()
    }

    pub fn update(&mut self) {
        self.dirty = self.interaction.update() || self.dirty;
    }

    pub fn get_state(&mut self) -> PaneState {
        self.dirty = false;

        PaneState {
            view_state: self.interaction.get_render_state(),
            layout: self.layout.clone(),
            key: self.case_key.clone(),
        }
    }

    pub fn set_case(&mut self, case: Option<CaseMeta>) {
        if let Some(case) = case {
            self.case_key = Some(case.key);
            self.interaction.set_image_count(case.number_of_images);
        } else {
            self.case_key = None;
            self.interaction.set_image_count(0);
        }
        self.dirty = true;
    }

    pub fn get_case_key(&self) -> Option<&String> {
        self.case_key.as_ref()
    }

    pub fn contains(&self, position: &PhysicalPosition<f64>) -> bool {
        self.layout.contains(position)
    }

    pub fn set_layout(&mut self, layout: LayoutRect) {
        self.layout = layout;
        // Assume the layout changes us so we are dirty
        println!("Pane is dirty");
        self.dirty = true;
    }

    pub fn invalidate(&mut self) {
        self.dirty = true;
    }
}

impl LayoutRect {
    pub fn contains(&self, position: &PhysicalPosition<f64>) -> bool {
        let left = self.x as f64;
        let right = (self.x + self.width) as f64;
        let top = self.y as f64;
        let bottom = (self.y + self.height) as f64;

        position.x >= left && position.x <= right && position.y >= top && position.y <= bottom
    }
}

#[derive(Debug)]
pub struct View {
    video_id: usize,
    data_id: String,
    gpu: bool,
    preset: String,
    lossless: bool,
    video_scaling: f32,
    fullrange: bool,
    bitrate: f32,
    dirty: bool,
    layout: LayoutRect,
    current_sample: Option<gst::Sample>,
    datachannel: Option<gst_webrtc::WebRTCDataChannel>,
    panes: Vec<Pane>,
    focus: Option<usize>,
    seq: u64,
}

impl View {
    pub fn new(
        video_id: usize,
        layout: LayoutRect,
        bitrate: f32,
        gpu: bool,
        preset: String,
        lossless: bool,
        video_scaling: f32,
        fullrange: bool,
    ) -> Self {
        // This is the expected name of the data channel.
        let data_id = format!("video{}-data", video_id);
        // Holds data that needs to be mutated
        Self {
            video_id,
            data_id,
            // Quality Settings
            gpu,
            preset,
            lossless,
            video_scaling,
            fullrange,
            layout,
            current_sample: None,
            datachannel: None,
            bitrate,
            dirty: false,
            panes: vec![Pane::default()],
            focus: None,
            seq: 0,
        }
    }

    pub fn video_id(&self) -> usize {
        self.video_id
    }
    pub fn data_id(&self) -> &str {
        &self.data_id
    }

    pub fn set_datachannel(&mut self, datachannel: gst_webrtc::WebRTCDataChannel) -> bool {
        if self.datachannel.is_some() {
            return false;
        }
        // Check the id of the channel.
        let label = datachannel
            .get_property_label()
            .expect("Failed to get datachannel label")
            .to_string();
        if &label != &self.data_id {
            return false;
        }
        // Everything seems to line up
        self.datachannel = Some(datachannel);

        // Invalidate this view, this will force an update.
        for pane in self.panes.iter_mut() {
            pane.invalidate();
        }

        true
    }

    fn accept_sample(&self, sample: &gst::Sample) -> bool {
        if self.dirty {
            // Check if the size of the sample is within bounds.
            let info = sample
                .get_caps()
                .and_then(|caps| gst_video::VideoInfo::from_caps(caps).ok())
                .unwrap();
            let area = info.width() * info.height();
            let expected = ((self.layout.width * self.layout.height) as f32
                * (self.video_scaling * self.video_scaling)) as u32;
            let diff = (1.0_f32 - (area as f32 / expected as f32)).abs();
            println!("Diff is {}", diff);
            diff < 0.1
        } else {
            true
        }
    }

    pub fn push_sample(&mut self, sample: gst::Sample) {
        if self.accept_sample(&sample) {
            self.current_sample = Some(sample);
            self.dirty = false;
        } else {
            println!("Not accepting sample");
        }
    }

    pub fn push_render_state(&self, state: RenderState) {
        self.try_send_message(DataMessage::NewState(state));
    }

    fn try_send_message(&self, msg: DataMessage) {
        if let Some(ref datachannel) = self.datachannel {
            match datachannel.get_property_ready_state() {
                gstreamer_webrtc::WebRTCDataChannelState::Open => {
                    if let Ok(msg) = String::try_from(msg) {
                        log::trace!("DC sending: {}", &msg);
                        datachannel.send_string(Some(&msg));
                    }
                }
                _ => {}
            }
        }
    }

    pub fn get_client_config(&self) -> ClientConfig {
        ClientConfig {
            id: format!("NativeClient_{}", self.video_id),
            viewport: ViewportSize {
                width: self.layout.width as _,
                height: self.layout.height as _,
            },
            bitrate: self.bitrate,
            gpu: self.gpu,
            preset: self.preset.clone(),
            lossless: self.lossless,
            video_scaling: self.video_scaling,
            fullrange: self.fullrange,
        }
    }

    pub fn get_current_sample(&self) -> Option<gst::Sample> {
        // Check if we have a sample, if so create copy and return it.
        // clone() should be cheap since it is a reference to a texture id.
        self.current_sample.as_ref().map(Clone::clone)
    }

    pub fn get_layout(&self) -> LayoutRect {
        self.layout
    }

    pub fn set_layout(&mut self, layout: LayoutRect) {
        self.layout = layout.clone();
        // Remove the stale sample
        self.current_sample.take();
        println!("Setting dirty on view");
        self.dirty = true;
    }

    pub fn partition(&mut self, rows: usize, columns: usize) {
        // Make sure we have the correct amount of panes
        self.panes.resize_with(rows * columns, || Pane::default());
        let view_size = (self.layout.width, self.layout.height);
        let layouts = tile(view_size, rows, columns);
        for (pane, layout) in self.panes.iter_mut().zip(layouts.into_iter()) {
            dbg!(&layout);
            pane.set_layout(layout);
        }
    }

    pub fn contains(&self, position: &PhysicalPosition<f64>) -> bool {
        self.layout.contains(position)
    }

    fn handle_focus(&mut self, position: &PhysicalPosition<f64>) {
        self.focus = None;
        for (idx, pane) in self.panes.iter().enumerate() {
            // Get the first view that contains the pointer position
            if pane.contains(position) {
                self.focus = Some(idx);
                break;
            }
        }
    }

    fn clear_focus(&mut self) {
        self.focus = None;
    }

    pub fn get_focused_pane(&mut self) -> Option<&mut Pane> {
        if let Some(idx) = self.focus {
            Some(
                self.panes
                    .get_mut(idx)
                    .expect("Failed to find focused pane"),
            )
        } else {
            None
        }
    }

    pub fn handle_window_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CursorMoved {
                position,
                device_id,
                modifiers,
            } => {
                let layout_position =
                    { PhysicalPosition::new(self.layout.x as f64, self.layout.y as f64) };
                // Translate positions relative to the top left corner.
                let translated = PhysicalPosition::new(
                    position.x - layout_position.x as f64,
                    position.y - layout_position.y as f64,
                );
                self.handle_focus(&translated);

                let event = WindowEvent::CursorMoved {
                    position: translated,
                    device_id: *device_id,
                    modifiers: *modifiers,
                };
                self.handle_translated_event(&event)
            }
            _ => self.handle_translated_event(event),
        }
    }

    fn handle_translated_event(&mut self, event: &WindowEvent) -> bool {
        // The event has been translated and the focused pane has been updated.
        self.get_focused_pane()
            .map_or(false, |pane| pane.handle_window_event(event))
    }

    pub fn hide_cursor(&self) -> bool {
        self.focus.map_or(false, |idx| {
            self.panes
                .get(idx)
                .expect("Failed to find focused pane")
                .hide_cursor()
        })
    }

    pub fn update(&mut self) {
        for pane in &mut self.panes {
            pane.update();
        }
    }

    pub fn push_state(&mut self) {
        let dirty = self.panes.iter().any(|p| p.dirty) || self.dirty;
        if dirty {
            let pane_states: Vec<_> = self.panes.iter_mut().map(|p| p.get_state()).collect();
            self.push_render_state(RenderState {
                layout: self.layout.clone(),
                seq: self.seq,
                panes: pane_states,
                snapshot: false,
                timestamp: 0_f32,
            });
            // Increase the sequence number
            self.seq += 1;
        }
    }

    pub fn set_case(&mut self, case: CaseMeta) {
        for pane in &mut self.panes {
            pane.set_case(Some(case.clone()));
        }
    }

    pub fn set_cases<I: Iterator<Item = Option<CaseMeta>>>(&mut self, iter: &mut I) {
        // Take a CaseMeta for each pane.
        for pane in &mut self.panes {
            // Get the next case (flatten to Option<Option<...>>)
            let case = iter.next().and_then(|c| c);
            pane.set_case(case);
        }
    }
}

#[derive(Debug)]
pub struct ViewControl {
    views: Vec<View>,
    active: Vec<usize>,
    focus: Option<usize>,
    layout: LayoutRect,
    default_case_key: Option<String>,
    default_protocol_key: Option<String>,
    current_protocol_key: Option<String>,
    protocols: Option<Protocols>,
    cases: Option<Vec<CaseMeta>>,
    partition: (usize, usize),
}

impl ViewControl {
    // When using the nvh264enc HW encoder we require at least dimensions 33x17 ???
    const DEFAULT_VIEW_WIDTH: u32 = 64;
    const DEFAULT_VIEW_HEIGHT: u32 = 64;

    pub fn new(number_of_views: usize, config: &AppConfig) -> Self {
        let views: Vec<_> = (0..number_of_views)
            .map(|i| {
                View::new(
                    i,
                    LayoutRect {
                        x: 0,
                        y: 0,
                        width: Self::DEFAULT_VIEW_WIDTH,
                        height: Self::DEFAULT_VIEW_HEIGHT,
                    },
                    config.bitrate,
                    config.gpu,
                    config.preset.clone(),
                    config.lossless,
                    config.video_scaling,
                    !config.narrow,
                )
            })
            .collect();

        Self {
            views,
            active: vec![0],
            focus: None,
            layout: LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            default_case_key: config.case_key.clone(),
            default_protocol_key: config.protocol_key.clone(),
            current_protocol_key: None,
            cases: None,
            protocols: None,
            partition: (1, 1),
        }
    }

    pub fn get_layout(&self) -> LayoutRect {
        self.layout.clone()
    }

    pub fn get_config(&self) -> Vec<ClientConfig> {
        self.views.iter().map(|v| v.get_client_config()).collect()
    }

    pub fn active_apply_mut<F: Fn(&mut View)>(&mut self, f: F) {
        for idx in &self.active {
            let view = self
                .views
                .get_mut(*idx)
                .expect("Failed to find active view index");
            f(view);
        }
    }

    pub fn active_map<T, F: Fn(&View) -> T>(&self, f: F) -> Vec<T> {
        let mut result = Vec::with_capacity(self.active.len());
        for idx in &self.active {
            let view = self
                .views
                .get(*idx)
                .expect("Failed to find active view index");
            result.push(f(view));
        }
        result
    }

    fn handle_focus(&mut self, position: &PhysicalPosition<f64>) {
        self.focus = None;
        for idx in &self.active {
            // Get the first view that contains the pointer position
            let view = self.views.get(*idx).expect("Active index not found");
            if view.contains(position) {
                self.focus = Some(*idx);
                break;
            }
        }
    }

    fn get_focused_view(&mut self) -> Option<&mut View> {
        if let Some(idx) = self.focus {
            Some(
                self.views
                    .get_mut(idx)
                    .expect("Failed to find focused view"),
            )
        } else {
            None
        }
    }

    fn clear_focus(&mut self) {
        self.focus = None;
        self.views.iter_mut().for_each(|v| v.clear_focus());
    }

    pub fn handle_window_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CursorMoved {
                position,
                device_id,
                modifiers,
            } => {
                let layout_position =
                    { PhysicalPosition::new(self.layout.x as f64, self.layout.y as f64) };
                // Translate positions relative to the top left corner.
                let translated = PhysicalPosition::new(
                    position.x - layout_position.x as f64,
                    position.y - layout_position.y as f64,
                );
                self.handle_focus(&translated);

                let event = WindowEvent::CursorMoved {
                    position: translated,
                    device_id: *device_id,
                    modifiers: *modifiers,
                };
                self.handle_translated_event(&event)
            }
            WindowEvent::KeyboardInput { input, .. } if input.state == ElementState::Pressed => {
                match input.virtual_keycode {
                    Some(VirtualKeyCode::S) => {
                        println!("S is pressed, setting single view");
                        self.set_active(&[0]);
                        true
                    }
                    Some(VirtualKeyCode::P) => {
                        println!("P is pressed, setting two views.");
                        self.set_active(&[0, 1]);
                        true
                    }
                    Some(VirtualKeyCode::Down) => {
                        println!("Down pressed");
                        self.select_next_case();
                        true
                    }
                    Some(VirtualKeyCode::Up) => {
                        println!("Up pressed");
                        self.select_previous_case();
                        true
                    }
                    Some(VirtualKeyCode::Right) => {
                        println!("Up pressed");
                        self.select_next_protocol();
                        true
                    }
                    Some(VirtualKeyCode::Left) => {
                        println!("Down pressed");
                        self.select_previous_protocol();
                        true
                    }
                    _ => false,
                }
            }
            _ => self.handle_translated_event(event),
        }
    }

    fn change_case(&mut self, direction: i32) {
        // Get the case currently selected in the focused pane
        let current_case = self
            .get_focused_view()
            .and_then(|v| v.get_focused_pane())
            .and_then(|pane| pane.get_case_key().map(Clone::clone));
        let new_index = current_case.map_or(Some(0), |current_case| {
            self.cases.as_ref().map(|cases| {
                let current_index = cases.iter().position(|case| *case.key == current_case);
                let reminder =
                    current_index.map_or(0, |idx| idx as i32 + direction) % cases.len() as i32;
                if reminder < 0 {
                    (reminder + cases.len() as i32) as usize
                } else {
                    reminder as usize
                }
            })
        });

        if let Some(case_index) = new_index {
            let case = self
                .cases
                .as_ref()
                .and_then(|cases| cases.get(case_index).map(Clone::clone));
            self.get_focused_view()
                .map(|view| view.get_focused_pane().map(|pane| pane.set_case(case)));
        } else {
            println!("No next case found");
        }
    }

    fn change_protocol(&mut self, direction: i32) {
        let layout = self.current_protocol_key.as_ref().and_then(|protocol_key| {
            self.protocols.as_ref().and_then(|p| {
                let current_index = p.layout.iter().position(|l| &l.name == protocol_key);
                let next_index =
                    current_index.map_or(0, |idx| idx as i32 + direction) % p.layout.len() as i32;
                let next_index = if next_index < 0 {
                    (next_index + p.layout.len() as i32) as usize
                } else {
                    next_index as usize
                };
                p.layout.get(next_index).map(Clone::clone)
            })
        });

        if let Some(layout) = layout {
            self.set_protocol(layout);
        } else {
            println!("No protocol found");
        }
    }

    fn select_next_case(&mut self) {
        self.change_case(1);
    }
    fn select_previous_case(&mut self) {
        self.change_case(-1);
    }
    fn select_next_protocol(&mut self) {
        self.change_protocol(1);
    }
    fn select_previous_protocol(&mut self) {
        self.change_protocol(-1);
    }

    fn handle_translated_event(&mut self, event: &WindowEvent) -> bool {
        // The event has been translated and the focused pane has been updated.
        if let Some(view) = self.get_focused_view() {
            view.handle_window_event(event)
        } else {
            false
        }
    }

    pub fn hide_cursor(&mut self) -> bool {
        if let Some(view) = self.get_focused_view() {
            view.hide_cursor()
        } else {
            false
        }
    }
    pub fn update(&mut self) {
        self.active_apply_mut(View::update);
    }
    pub fn push_state(&mut self) {
        self.active_apply_mut(View::push_state);
    }

    pub fn set_case(&mut self, case: CaseMeta) {
        // For now set on all active views
        self.active_apply_mut(|view| view.set_case(case.clone()));
    }

    pub fn set_protocol(&mut self, protocol: LayoutCfg) {
        log::info!(
            "Setting protocol partition {}x{}",
            protocol.rows,
            protocol.columns
        );
        self.current_protocol_key = Some(protocol.name);

        self.partition(protocol.rows, protocol.columns);

        // Assign cases to panes. We need to collect into a vector so we can
        // borrow self mutably later.
        let cases: Vec<_> = protocol
            .panes
            .iter()
            .map(|p| self.get_case_for_key(&p.case))
            .collect();

        // "Reuse" the iterator over all views
        let mut cases = cases.into_iter();
        for idx in &self.active {
            let view = self
                .views
                .get_mut(*idx)
                .expect("Failed to find active view");
            // This will consume the next cases in the iterator.
            view.set_cases(&mut cases);
        }
    }

    pub fn set_active(&mut self, idxs: &[usize]) {
        self.active = idxs
            .iter()
            .filter_map(|idx| {
                if *idx < self.views.len() {
                    Some(*idx)
                } else {
                    None
                }
            })
            .collect();
    }

    pub fn set_layout(&mut self, layout: LayoutRect) {
        self.layout = layout;
        self.update_partitions();
    }

    pub fn set_window_size(&mut self, size: (u32, u32)) {
        // Recompute the position for the layout, given the new window size.
        // Center the layout in the window.
        self.layout.x = ((size.0 as f32 - self.layout.width as f32) / 2_f32).max(0_f32) as u32;
        self.layout.y = ((size.1 as f32 - self.layout.height as f32) / 2_f32).max(0_f32) as u32;
    }

    pub fn set_datachannel(&mut self, datachannel: gst_webrtc::WebRTCDataChannel) {
        // Find the target view for this datachannel
        let label = datachannel
            .get_property_label()
            .expect("No datachannel label")
            .to_string();

        let view = self.views.iter_mut().find(|v| v.data_id() == &label);
        if let Some(view) = view {
            view.set_datachannel(datachannel);
        } else {
            log::error!("Failed to find view for datachannel with label {}", label);
        }
    }

    pub fn push_sample(&mut self, sample: ViewSample) {
        // We should be able to find the view based on index.
        if let Some(view) = self.views.get_mut(sample.id) {
            view.push_sample(sample.sample);
        } else {
            log::error!("Failed to find view with index {}", sample.id);
        }
    }

    pub fn get_case_string(&self) -> String {
        if let Some(cases) = &self.cases {
            let case_keys: Vec<_> = cases.iter().map(|c| c.key.clone()).collect();
            case_keys.join("\n")
        } else {
            String::from("<No cases loaded>")
        }
    }

    pub fn get_protocol_string(&self) -> String {
        if let Some(protocols) = &self.protocols {
            let layout_names: Vec<_> = protocols.layout.iter().map(|l| l.name.clone()).collect();
            layout_names.join("\n")
        } else {
            String::from("<No protocols loaded>")
        }
    }

    fn get_case_for_key(&self, case_key: &str) -> Option<CaseMeta> {
        self.cases.as_ref().and_then(|c| {
            // Get the matching case or the first
            c.iter()
                .find(|case| *case.key == *case_key)
                .map(Clone::clone)
        })
    }

    pub fn select_case_from_key(&mut self, case_key: &str) {
        // Try to find the case based on key
        if let Some(case) = self.get_case_for_key(case_key) {
            println!("Selected case: {}", &case.key);
            self.set_case(case);
        } else {
            log::warn!("Failed to find case with key {}", case_key);
        }
    }

    pub fn select_default_case(&mut self) {
        let case_key = self.default_case_key.as_ref().map(Clone::clone);
        match case_key {
            Some(case_key) => {
                self.select_case_from_key(&case_key);
            }
            None => {
                log::info!("No default case set, using first case");
                let selected = self
                    .cases
                    .as_ref()
                    .and_then(|c| c.first().map(Clone::clone));
                if let Some(selected) = selected {
                    self.set_case(selected);
                } else {
                    log::warn!("No cases found!");
                }
            }
        }
    }

    pub fn select_protocol_from_key(&mut self, protocol_key: &str) {
        // Try to find the case based on key
        let selected = self.protocols.as_ref().and_then(|p| {
            // Get the matching case or the first
            p.layout
                .iter()
                .find(|layout| *layout.name == *protocol_key)
                .map(|layout| layout.clone())
        });

        if let Some(layout) = selected {
            println!("Selected layout: {}", &layout.name);
            self.set_protocol(layout);
        } else {
            log::warn!("Failed to find layout with key {}", protocol_key);
        }
    }

    pub fn select_default_display(&mut self) {
        if let Some(protocol_key) = self.default_protocol_key.as_ref().map(|p| p.clone()) {
            log::info!("Using preferred protocol key {}", &protocol_key);
            self.select_protocol_from_key(&protocol_key);
        } else {
            log::info!("No preferred protocol set, checking case key");
            self.select_default_case();
        }
    }

    pub fn set_case_meta(&mut self, protocols: Option<Protocols>, cases: Vec<CaseMeta>) {
        self.cases = Some(cases);
        self.protocols = protocols;
    }

    pub fn partition(&mut self, rows: usize, columns: usize) {
        self.partition = (rows, columns);
        self.update_partitions();
    }

    pub fn update_partitions(&mut self) {

        // Make sure to clear the focus since we might change the
        // set of views/panes.
        self.clear_focus();

        let (rows, columns) = self.partition;
        // Check if we can split each partition to its own view.
        // Otherwise check if each row can use a separate view
        // In the last case we use a single view with all partitions.
        let (rows, columns, (pane_rows, pane_columns)) = if rows * columns <= self.views.len() {
            println!("Each partition gets its own view");
            // Each view is partitioned 1x1
            (rows, columns, (1, 1))
        } else if rows <= self.views.len() {
            println!("Each row gets its own view");
            // Use row views, each partitioned into 1xcolumns
            (rows, 1, (1, columns))
        } else {
            println!("All partitions in a single view");
            // The view is partitioned rows x columns
            (1, 1, (rows, columns))
        };

        let view_size = (self.layout.width, self.layout.height);
        let view_layouts = tile(view_size, rows, columns);
        // Activate the number of views needed.
        self.set_active(&(0..view_layouts.len()).collect::<Vec<_>>());
        for (idx, layout) in self.active.iter().zip(view_layouts.into_iter()) {
            let view = self.views.get_mut(*idx).expect("Failed to get active view");
            view.set_layout(layout);
            view.partition(pane_rows, pane_columns);
        }
    }
}
