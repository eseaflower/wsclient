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
    message::{ClientConfig, DataMessage, LayoutRect, PaneState, RenderState, ViewportSize},
    window_message::ViewSample,
    AppConfig,
};

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
    // pub fn get_state(&mut self, force: bool) -> Option<PaneState> {
    //     let view_state = {
    //         if self.dirty || force {
    //             if !self.dirty {
    //                 println!("Forcing state update due to flag");
    //             }
    //             println!("Pane is clean");
    //             self.dirty = false;
    //             Some(self.interaction.get_render_state())
    //         } else {
    //             None
    //         }
    //     };

    //     view_state.map(|v| PaneState {
    //         view_state: v,
    //         layout: self.layout.clone(),
    //         key: self.case_key.clone(),
    //     })
    // }

    pub fn set_case(&mut self, case_key: String, number_of_images: usize) {
        self.case_key = Some(case_key);
        self.interaction.set_image_count(number_of_images);
        self.dirty = true;
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
            panes: vec![Pane::default(), Pane::default()],
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

    pub fn arrange_horizontal(&mut self) {
        if self.panes.len() <= 0 {
            log::warn!("No panes when trying to arrange");
        }
        println!("Got pane arrange!!!!!");
        let pane_width = self.layout.width as f32 / self.panes.len() as f32;
        // Align to 4 pixels.
        let pane_width = ((pane_width / 4_f32).floor() * 4_f32) as u32;
        let pane_height = self.layout.height;

        let mut curr_x = 0;
        for pane in self.panes.iter_mut() {
            {
                pane.set_layout(LayoutRect {
                    x: curr_x,
                    y: 0,
                    width: pane_width,
                    height: pane_height,
                });

                curr_x += pane_width;
            }
        }
    }

    pub fn set_layout(&mut self, layout: LayoutRect) {
        self.layout = layout.clone();
        self.arrange_horizontal();
        // Remove the stale sample
        self.current_sample.take();
        println!("Setting dirty on view");
        self.dirty = true;
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
        self.focus.map_or(false, |idx| {
            self.panes
                .get_mut(idx)
                .expect("Failed to find focused pane")
                .handle_window_event(event)
        })
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

    pub fn set_case(&mut self, case_key: String, number_of_images: usize) {
        for pane in &mut self.panes {
            pane.set_case(case_key.clone(), number_of_images);
        }
    }
}

#[derive(Debug)]
pub enum Fill {
    None,
    Vertical,
    Horizontal,
    Full,
}

#[derive(Debug)]
pub struct ViewControl {
    views: Vec<View>,
    active: Vec<usize>,
    focus: Option<usize>,
    layout: LayoutRect,
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
        }
    }

    pub fn arrange_horizontal(&mut self) {
        let active = &self.active;
        if active.len() <= 0 {
            log::warn!("No views active when trying to arrange");
        }
        let view_width = self.layout.width as f32 / active.len() as f32;
        // Align to 4 pixels.
        let view_width = ((view_width / 4_f32).floor() * 4_f32) as u32;
        let view_height = self.layout.height;

        let mut curr_x = 0;
        for idx in &self.active {
            {
                let view = self
                    .views
                    .get_mut(*idx)
                    .expect("Active view index not found");
                view.set_layout(LayoutRect {
                    x: curr_x,
                    y: 0,
                    width: view_width,
                    height: view_height,
                });

                curr_x += view_width;
            }
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
                        self.arrange_horizontal();
                        true
                    }
                    Some(VirtualKeyCode::P) => {
                        println!("P is pressed, setting two views.");
                        self.set_active(&[0, 1]);
                        self.arrange_horizontal();
                        true
                    }
                    _ => false,
                }
            }
            _ => self.handle_translated_event(event),
        }
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

    pub fn set_case(&mut self, case_key: String, number_of_images: usize) {
        // For now set on all active views
        self.active_apply_mut(|view| view.set_case(case_key.clone(), number_of_images));
    }

    pub fn set_active(&mut self, idxs: &[usize]) {
        {
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
        self.arrange_horizontal();
    }

    pub fn set_layout(&mut self, layout: LayoutRect) {
        {
            self.layout = layout;
        }
        self.arrange_horizontal();
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
}
