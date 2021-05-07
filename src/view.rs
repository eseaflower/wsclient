use std::{
    convert::TryFrom,
    ops::Deref,
    sync::{Arc, Mutex},
};

use glutin::{
    dpi::PhysicalPosition,
    event::{MouseScrollDelta, WindowEvent},
    event_loop::EventLoopProxy,
};
use gstreamer as gst;
use gstreamer_webrtc as gst_webrtc;

use crate::{
    interaction::InteractionState,
    message::{ClientConfig, DataMessage, LayoutRect, PaneState, RenderState, ViewportSize},
    window_message::WindowMessage,
    AppConfig,
};

#[derive(Debug)]
pub struct Pane {
    layout: LayoutRect,
    interaction: InteractionState,
    dirty: bool,
    case_key: Option<String>,
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

    pub fn get_state(&mut self) -> Option<PaneState> {
        let view_state = {
            if self.dirty {
                self.dirty = false;
                Some(self.interaction.get_render_state())
            } else {
                None
            }
        };
        view_state.map(|v| PaneState {
            view_state: v,
            layout: self.layout.clone(),
            key: self.case_key.clone(),
        })
    }

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

#[derive(Debug, Clone)]
pub struct View(pub Arc<ViewInner>);
impl Deref for View {
    type Target = ViewInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug)]
struct SharedView {
    layout: LayoutRect,
    current_sample: Option<gst::Sample>,
    datachannel: Option<gst_webrtc::WebRTCDataChannel>,
    bitrate: f32,
    proxy: Option<EventLoopProxy<WindowMessage>>,
    dirty: bool,
    panes: Vec<Pane>,
    focus: Option<usize>,
    seq: u64,
}

#[derive(Debug)]
pub struct ViewInner {
    video_id: usize,
    data_id: String,

    gpu: bool,
    preset: String,
    lossless: bool,
    video_scaling: f32,
    fullrange: bool,

    shared: Mutex<SharedView>,
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
        let shared = SharedView {
            layout,
            current_sample: None,
            datachannel: None,
            bitrate,
            proxy: None,
            dirty: false,
            panes: vec![Pane {
                layout: layout.clone(),
                interaction: InteractionState::new(),
                dirty: false,
                case_key: None,
            }],
            focus: None,
            seq: 0,
        };
        let inner = ViewInner {
            video_id,
            data_id,
            // Quality Settings
            gpu,
            preset,
            lossless,
            video_scaling,
            fullrange,
            shared: Mutex::new(shared),
        };
        Self(Arc::new(inner))
    }

    pub fn video_id(&self) -> usize {
        self.video_id
    }
    pub fn data_id(&self) -> &str {
        &self.data_id
    }

    pub fn set_datachannel(&self, datachannel: gst_webrtc::WebRTCDataChannel) -> bool {
        let mut shared = self.shared.lock().unwrap();
        if shared.datachannel.is_some() {
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
        shared.datachannel = Some(datachannel);

        // Invalidate this view, this will force an update.
        for pane in shared.panes.iter_mut() {
            pane.invalidate();
        }

        true
    }

    pub fn set_event_proxy(&self, proxy: EventLoopProxy<WindowMessage>) {
        let mut shared = self.shared.lock().unwrap();
        shared.proxy = Some(proxy);
    }

    pub fn push_sample(&self, sample: gst::Sample) {
        let mut shared = self.shared.lock().unwrap();
        shared.current_sample = Some(sample);
        shared
            .proxy
            .as_ref()
            .map(|p| p.send_event(WindowMessage::Redraw(self.video_id)));
    }

    pub fn push_render_state(&self, state: RenderState) {
        self.try_send_message(DataMessage::NewState(state));
    }

    fn try_send_message(&self, msg: DataMessage) {
        let shared = self.shared.lock().unwrap();
        if let Some(ref datachannel) = shared.datachannel {
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
        let shared = self.shared.lock().unwrap();
        ClientConfig {
            id: format!("NativeClient_{}", self.video_id),
            viewport: ViewportSize {
                width: shared.layout.width as _,
                height: shared.layout.height as _,
            },
            bitrate: shared.bitrate,
            gpu: self.gpu,
            preset: self.preset.clone(),
            lossless: self.lossless,
            video_scaling: self.video_scaling,
            fullrange: self.fullrange,
        }
    }

    pub fn get_current_sample(&self) -> Option<gst::Sample> {
        let shared = self.shared.lock().unwrap();
        // Check if we have a sample, if so create copy and return it.
        // clone() should be cheap since it is a reference to a texture id.
        shared.current_sample.as_ref().map(Clone::clone)
    }

    pub fn get_layout(&self) -> LayoutRect {
        let shared = self.shared.lock().unwrap();
        shared.layout
    }

    pub fn set_layout(&self, layout: LayoutRect) {
        dbg!(&layout);

        let mut shared = self.shared.lock().unwrap();
        shared.layout = layout.clone();
        // For now we have a single pane
        assert!(shared.panes.len() == 1);
        // The panes are positioned relative to the view
        shared.panes[0].set_layout(LayoutRect {
            x: 0,
            y: 0,
            width: layout.width,
            height: layout.height,
        });
    }

    pub fn contains(&self, position: &PhysicalPosition<f64>) -> bool {
        let layout = &self.shared.lock().unwrap().layout;
        layout.contains(position)
    }

    fn handle_focus(&self, position: &PhysicalPosition<f64>) {
        let mut shared = self.shared.lock().unwrap();
        shared.focus = None;
        for idx in 0..shared.panes.len() {
            // Get the first view that contains the pointer position
            let pane = shared.panes.get(idx).expect("Focused pane index not found");
            if pane.contains(position) {
                shared.focus = Some(idx);
                break;
            }
        }
    }

    pub fn handle_window_event(&self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CursorMoved {
                position,
                device_id,
                modifiers,
            } => {
                let layout_position = {
                    let shared = self.shared.lock().unwrap();
                    PhysicalPosition::new(shared.layout.x as f64, shared.layout.y as f64)
                };
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

    fn handle_translated_event(&self, event: &WindowEvent) -> bool {
        // The event has been translated and the focused pane has been updated.
        let mut shared = self.shared.lock().unwrap();
        shared.focus.map_or(false, |idx| {
            shared
                .panes
                .get_mut(idx)
                .expect("Failed to find focused pane")
                .handle_window_event(event)
        })
    }

    pub fn hide_cursor(&self) -> bool {
        let mut shared = self.shared.lock().unwrap();
        shared.focus.map_or(false, |idx| {
            shared
                .panes
                .get_mut(idx)
                .expect("Failed to find focused pane")
                .hide_cursor()
        })
    }

    pub fn update(&self) {
        let mut shared = self.shared.lock().unwrap();
        for pane in &mut shared.panes {
            pane.update();
        }
    }

    pub fn push_state(&self) {
        let pane_state = {
            let mut shared = self.shared.lock().unwrap();
            if shared.panes.iter().any(|pane| pane.dirty) {
                let panes = &mut shared.panes;
                assert!(panes.len() == 1);
                // How to handle multiple panes?
                panes[0].get_state()
            } else {
                None
            }
        };
        if let Some(pane_state) = pane_state {
            // Update sequence number for this view
            let (seq, view_layout) = {
                let mut shared = self.shared.lock().unwrap();
                shared.seq += 1;
                (shared.seq, shared.layout.clone())
            };
            self.push_render_state(RenderState {
                layout: view_layout,
                seq,
                panes: vec![pane_state],
                snapshot: false,
                timestamp: 0_f32,
            });
        }
    }

    pub fn set_case(&self, case_key: String, number_of_images: usize) {
        for pane in &mut self.shared.lock().unwrap().panes {
            pane.set_case(case_key.clone(), number_of_images);
        }
    }
}

#[derive(Debug)]
struct SharedViewControl {
    active: Vec<usize>,
    focus: Option<usize>,
    layout: LayoutRect,
}
#[derive(Debug)]
pub struct ViewControl {
    views: Vec<View>,
    shared: Mutex<SharedViewControl>,
}

impl ViewControl {
    pub fn new(number_of_views: usize, config: &AppConfig) -> Self {
        let views: Vec<_> = (0..number_of_views)
            .map(|i| {
                View::new(
                    i,
                    LayoutRect {
                        x: 0,
                        y: 0,
                        width: 0,
                        height: 0,
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

        let shared = SharedViewControl {
            active: (0..views.len()).collect(),
            focus: None,
            layout: LayoutRect {
                x: 0,
                y: 0,
                width: config.viewport_size.0,
                height: config.viewport_size.1,
            },
        };

        Self {
            views,
            shared: Mutex::new(shared),
        }
    }

    pub fn get_n_views(&self) -> usize {
        self.views.len()
    }

    pub fn find_by_id(&self, id: usize) -> Option<&View> {
        self.views.iter().find(|v| v.video_id() == id)
    }

    pub fn find_by_label(&self, label: &str) -> Option<&View> {
        self.views.iter().find(|v| v.data_id() == label)
    }

    pub fn arrange_horizontal(&self) {
        let shared = self.shared.lock().unwrap();
        let active = &shared.active;
        if active.len() <= 0 {
            log::warn!("No views active when trying to arrange");
        }
        let view_width = shared.layout.width as f32 / active.len() as f32;
        // Align to 4 pixels.
        let view_width = ((view_width / 4_f32).floor() * 4_f32) as u32;
        let view_height = shared.layout.height;

        let mut curr_x = 0;
        for idx in &shared.active {
            {
                let view = self.views.get(*idx).expect("Active view index not found");
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
        self.shared.lock().unwrap().layout.clone()
    }

    pub fn get_config(&self) -> Vec<ClientConfig> {
        self.views.iter().map(|v| v.get_client_config()).collect()
    }

    pub fn set_event_proxy(&self, proxy: &EventLoopProxy<WindowMessage>) {
        self.views
            .iter()
            .for_each(|v| v.set_event_proxy(proxy.clone()));
    }

    pub fn get_active(&self) -> Vec<&View> {
        self.shared
            .lock()
            .unwrap()
            .active
            .iter()
            .map(|idx| self.views.get(*idx).expect("Active view index not found"))
            .collect()
    }

    fn handle_focus(&self, position: &PhysicalPosition<f64>) {
        let mut shared = self.shared.lock().unwrap();
        shared.focus = None;
        for idx in &shared.active {
            // Get the first view that contains the pointer position
            let view = self.views.get(*idx).expect("Active index not found");
            if view.contains(position) {
                shared.focus = Some(*idx);
                break;
            }
        }
    }

    fn get_focused_view(&self) -> Option<&View> {
        if let Some(idx) = self.shared.lock().unwrap().focus {
            Some(self.views.get(idx).expect("Failed to find focused view"))
        } else {
            None
        }
    }

    pub fn handle_window_event(&self, event: &WindowEvent) -> bool {
        if let WindowEvent::CursorMoved { position, .. } = event {
            // When the cursor is moved check if the focus changes to another view
            self.handle_focus(position);
        }
        if let Some(view) = self.get_focused_view() {
            view.handle_window_event(event)
        } else {
            false
        }
    }

    pub fn hide_cursor(&self) -> bool {
        if let Some(view) = self.get_focused_view() {
            view.hide_cursor()
        } else {
            false
        }
    }
    pub fn update(&self) {
        for view in self.get_active() {
            view.update();
        }
    }
    pub fn push_state(&self) {
        for view in self.get_active() {
            view.push_state();
        }
    }

    pub fn set_case(&self, case_key: String, number_of_images: usize) {
        // For now set on all active views
        for view in self.get_active() {
            view.set_case(case_key.clone(), number_of_images);
        }
    }

    pub fn set_active(&self, idxs: &[usize]) {
        {
            let mut shared = self.shared.lock().unwrap();
            shared.active = idxs
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
}
