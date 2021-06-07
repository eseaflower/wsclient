use std::{
    convert::TryFrom,
    ops::Deref,
    sync::{Arc, Mutex},
};

use glutin::{
    dpi::PhysicalPosition,
    event::{ElementState, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent},
};
use gstreamer as gst;
use gstreamer_video as gst_video;
use gstreamer_webrtc as gst_webrtc;

use crate::{
    interaction::{InteractionState, SyncOperation},
    message::{
        CaseMeta, ClientConfig, DataMessage, LayoutCfg, LayoutRect, PaneState, Protocols,
        RenderState, ViewportSize,
    },
    util::bitrate::Schedule,
    view,
    view_state::ViewState,
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
    id: String,
    layout: LayoutRect,
    interaction: InteractionState,
    dirty: bool,
    case: Option<CaseMeta>,
}

impl Default for Pane {
    fn default() -> Self {
        Pane {
            id: String::from("default"),
            layout: LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            interaction: InteractionState::new(),
            dirty: false,
            case: None,
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
                self.interaction
                    .handle_move(translated, 1f32 / self.layout.height as f32);
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
            WindowEvent::KeyboardInput { input, .. } if input.state == ElementState::Pressed => {
                match input.virtual_keycode {
                    Some(VirtualKeyCode::S) => {
                        self.interaction.toggle_sync();
                        log::debug!(
                            "Sync on pane {} is {}",
                            &self.id,
                            self.interaction.is_synchronized()
                        );
                        true
                    }
                    Some(VirtualKeyCode::C) => {
                        self.interaction.toggle_cine();
                        true
                    }
                    Some(VirtualKeyCode::I) => {
                        self.interaction.adjust_cine_speec(1);
                        true
                    }
                    Some(VirtualKeyCode::U) => {
                        self.interaction.adjust_cine_speec(-1);
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    pub fn hide_cursor(&self) -> bool {
        self.interaction.hide_cursor()
    }

    pub fn update(&mut self) -> Option<SyncOperation> {
        let (updated, sync) = self.interaction.update();
        self.dirty = updated || self.dirty;
        sync
    }

    pub fn update_sync(&mut self, sync: &(String, SyncOperation)) {
        if self.interaction.is_synchronized() && self.id != sync.0 {
            // We did not issue the sync-op, apply
            match sync.1 {
                SyncOperation::Scroll(delta) => {
                    // "Hack" the frame move by issuing a mouse wheel event.
                    // Invert the delta.
                    self.interaction.handle_mouse_wheel(-delta as f32);
                }
            }
        }
        // Run normal update.
        self.update();
    }

    pub fn get_state(&mut self) -> PaneState {
        self.dirty = false;

        PaneState {
            view_state: self.interaction.get_render_state(),
            layout: self.layout.clone(),
            key: self.case.as_ref().map(|c| c.key.clone()),
        }
    }

    pub fn set_case(&mut self, case: Option<CaseMeta>) {
        // Reset the interaction state
        self.interaction = InteractionState::new();
        self.case = case;

        if let Some(case) = &self.case {
            self.interaction.set_image_count(case.number_of_images);
        } else {
            self.interaction.set_image_count(0);
        }
        self.dirty = true;
    }

    pub fn get_case_key(&self) -> Option<&String> {
        self.case.as_ref().map(|c| &c.key)
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

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn set_id(&mut self, id: String) {
        self.id = id;
    }

    pub fn park_state(&self) -> (Option<CaseMeta>, ViewState) {
        (self.case.clone(), self.interaction.get_render_state())
    }

    pub fn set_viewstate(&mut self, state: ViewState) {
        self.interaction.set_render_state(state);
        self.dirty = true;
    }

    pub fn handle_timer_event(&mut self) -> bool {
        self.interaction.cine_update()
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
    bitrate_scale: f32,
    dirty: bool,
    layout: LayoutRect,
    current_sample: Option<ViewSample>,
    datachannel: Option<gst_webrtc::WebRTCDataChannel>,
    panes: Vec<Pane>,
    focus: Option<usize>,
    seq: u64,
    timer: std::time::Instant,
    schedule: Schedule,
}

impl View {
    const BITRATE_SCALE_DELTA: f32 = 0.1;

    pub fn new(
        video_id: usize,
        layout: LayoutRect,
        bitrate_scale: f32,
        gpu: bool,
        preset: String,
        lossless: bool,
        video_scaling: f32,
        fullrange: bool,
        schedule: Schedule,
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
            bitrate_scale,
            dirty: false,
            panes: vec![Pane::default()],
            focus: None,
            seq: 0,
            timer: std::time::Instant::now(),
            schedule,
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

    fn accept_sample(&self, sample: &ViewSample) -> bool {
        if self.dirty {
            // Check if the size of the sample is within bounds.
            let info = sample
                .sample
                .get_caps()
                .and_then(|caps| gst_video::VideoInfo::from_caps(caps).ok())
                .unwrap();
            let area = info.width() * info.height();
            let expected = ((self.layout.width * self.layout.height) as f32
                * (self.video_scaling * self.video_scaling)) as u32;
            let diff = (1.0_f32 - (area as f32 / expected as f32)).abs();
            diff < 0.1
        } else {
            true
        }
    }

    pub fn push_sample(&mut self, sample: ViewSample) {
        if self.accept_sample(&sample) {
            self.current_sample = Some(sample);
            self.dirty = false;
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
            bitrate: self.get_bitrate(),
            gpu: self.gpu,
            preset: self.preset.clone(),
            lossless: self.lossless,
            video_scaling: self.video_scaling,
            fullrange: self.fullrange,
        }
    }

    pub fn get_current_sample(&self) -> Option<ViewSample> {
        // Check if we have a sample, if so create copy and return it.
        // clone() should be cheap since it is a reference to a texture id.
        self.current_sample.as_ref().map(Clone::clone)
    }

    pub fn get_layout(&self) -> LayoutRect {
        self.layout
    }

    pub fn set_layout(&mut self, layout: LayoutRect) {
        self.layout = layout;
        // Update the video scaling when the size changes.
        self.video_scaling = self
            .schedule
            .scaling((self.layout.width, self.layout.height));
        // Remove the stale sample
        self.current_sample.take();
        self.dirty = true;
    }

    pub fn partition(&mut self, rows: usize, columns: usize) {
        // Make sure we have the correct amount of panes
        self.panes.resize_with(rows * columns, || Pane::default());
        let view_size = (self.layout.width, self.layout.height);
        let layouts = tile(view_size, rows, columns);
        for (id_suffix, (pane, layout)) in
            self.panes.iter_mut().zip(layouts.into_iter()).enumerate()
        {
            log::debug!("New layout {:?}", &layout);
            pane.set_layout(layout);
            // Generate a unique name for each pane.
            pane.set_id(format!("{}:{}", self.video_id, id_suffix));
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
            WindowEvent::KeyboardInput { input, .. } if input.state == ElementState::Pressed => {
                match input.virtual_keycode {
                    Some(VirtualKeyCode::B) => {
                        self.adjust_bitrate_scaling(1);
                        true
                    }
                    Some(VirtualKeyCode::V) => {
                        self.adjust_bitrate_scaling(-1);
                        true
                    }
                    _ => self.handle_translated_event(event),
                }
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

    pub fn update_sync(&mut self, sync: &(String, SyncOperation)) {
        for pane in &mut self.panes {
            pane.update_sync(sync);
        }
    }

    pub fn get_timestamp(&self) -> f32 {
        (self.timer.elapsed().as_millis() % 1000) as f32
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
                timestamp: self.get_timestamp(),
                bitrate: self.get_bitrate(),
                scaling: self.video_scaling,
            });
            // Increase the sequence number
            self.seq += 1;
        }
    }

    pub fn set_case(&mut self, case: Option<CaseMeta>) {
        for pane in &mut self.panes {
            pane.set_case(case.clone());
        }
    }

    pub fn set_viewstate(&mut self, state: ViewState) {
        for pane in &mut self.panes {
            pane.set_viewstate(state.clone());
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

    pub fn park_state(&self) -> Vec<(Option<CaseMeta>, ViewState)> {
        self.panes.iter().map(|p| p.park_state()).collect()
    }

    pub fn restore_parked(&mut self, parked: Vec<(Option<CaseMeta>, ViewState)>) {
        for (pane, state) in self.panes.iter_mut().zip(parked.into_iter()) {
            pane.set_case(state.0);
            pane.set_viewstate(state.1);
        }
    }

    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub fn handle_timer_event(&mut self) -> Vec<(String, SyncOperation)> {
        let mut sync_ops = Vec::new();
        for pane in self.panes.iter_mut() {
            if pane.handle_timer_event() {
                // Run the update and collect sync operations
                pane.update()
                    .map(|s| sync_ops.push((pane.id().to_string(), s)));
            }
        }
        sync_ops
    }

    fn get_bitrate(&self) -> f32 {
        if self.lossless {
            // If scale is 0, we want lossless
            return 0_f32;
        }

        // Check the rate schedule
        self.schedule
            .bitrate((self.layout.width, self.layout.height))
            * self.bitrate_scale
    }

    pub fn adjust_bitrate_scaling(&mut self, direction: i32) {
        self.bitrate_scale += direction as f32 * Self::BITRATE_SCALE_DELTA;
        self.bitrate_scale = self.bitrate_scale.max(0.1);
        println!(
            "Setting new bitrate scale {}, {}x{} -> {}",
            self.bitrate_scale,
            self.layout.width,
            self.layout.height,
            self.get_bitrate()
        );
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
    last_click: std::time::Instant,
    parked: Option<ParkedState>,
}

impl ViewControl {
    // When using the nvh264enc HW encoder we require at least dimensions 33x17 ???
    // 145x49 on Turing
    const DEFAULT_VIEW_WIDTH: u32 = 256;
    const DEFAULT_VIEW_HEIGHT: u32 = 256;

    pub fn new(config: &AppConfig) -> Self {
        let views: Vec<_> = (0..config.n_views)
            .map(|i| {
                View::new(
                    i,
                    LayoutRect {
                        x: 0,
                        y: 0,
                        width: Self::DEFAULT_VIEW_WIDTH,
                        height: Self::DEFAULT_VIEW_HEIGHT,
                    },
                    config.bitrate_scale,
                    config.gpu,
                    config.preset.clone(),
                    config.lossless,
                    config.video_scaling,
                    !config.narrow,
                    config.schedule,
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
            last_click: std::time::Instant::now(),
            parked: None,
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
                    Some(VirtualKeyCode::Down) => {
                        self.select_next_case();
                        true
                    }
                    Some(VirtualKeyCode::Up) => {
                        self.select_previous_case();
                        true
                    }
                    Some(VirtualKeyCode::Right) => {
                        self.select_next_protocol();
                        true
                    }
                    Some(VirtualKeyCode::Left) => {
                        self.select_previous_protocol();
                        true
                    }
                    _ => self.handle_translated_event(event),
                }
            }
            WindowEvent::MouseInput { state, button, .. }
                if *state == ElementState::Pressed && *button == MouseButton::Left =>
            {
                let time_since_last_click = self.last_click.elapsed().as_millis();
                self.last_click = std::time::Instant::now();
                if time_since_last_click < 200 {
                    self.toggle_1x1();
                    true
                } else {
                    self.handle_translated_event(event)
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
            log::info!("No next case found");
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
            log::info!("No protocol found");
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
    pub fn update_focused(&mut self) {
        let sync_update = self
            .get_focused_view()
            .and_then(|v| v.get_focused_pane())
            .and_then(|pane| pane.update().map(|op| (pane.id().to_string(), op)));

        self.apply_update(sync_update);
    }

    fn apply_update(&mut self, sync_update: Option<(String, SyncOperation)>) {
        if let Some(sync_update) = sync_update {
            log::trace!("Running sync op from pane {}", &sync_update.0);
            // Run the sync update.
            self.active_apply_mut(|v| v.update_sync(&sync_update));
        } else {
            self.active_apply_mut(View::update);
        }
    }

    pub fn push_state(&mut self) {
        self.active_apply_mut(View::push_state);
    }

    pub fn set_case(&mut self, case: Option<CaseMeta>) {
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

        // Invalidate all views.
        self.invalidate();
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
            view.push_sample(sample);
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
            self.set_case(Some(case));
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
                    self.set_case(Some(selected));
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
            log::info!("Each partition gets its own view");
            // Each view is partitioned 1x1
            (rows, columns, (1, 1))
        } else if rows <= self.views.len() {
            log::info!("Each row gets its own view");
            // Use row views, each partitioned into 1xcolumns
            (rows, 1, (1, columns))
        } else {
            log::info!("All partitions in a single view");
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

    fn toggle_1x1(&mut self) {
        if self.parked.is_some() {
            // Restore previous parked state
            log::debug!("Restoring parked views");
            self.restore_parked();
        } else {
            let focused_settings = self.get_focused_view().and_then(|v| {
                v.get_focused_pane()
                    .and_then(|pane| Some((pane.case.clone(), pane.interaction.get_render_state())))
            });

            if let Some(settings) = focused_settings {
                // We have a focused pane.

                // Save state to restore.
                self.park_state();
                log::debug!("Parking views");

                self.partition(1, 1);
                self.active_apply_mut(|v| {
                    v.set_case(settings.0.clone());
                    v.set_viewstate(settings.1)
                })
            }
        }
    }

    fn park_state(&mut self) {
        let states = self.active_map(|v| v.park_state());
        self.parked = Some(ParkedState {
            partition: self.partition,
            states,
        });
    }

    fn restore_parked(&mut self) {
        if let Some(parked) = self.parked.take() {
            self.partition(parked.partition.0, parked.partition.1);

            for (idx, parked_view) in self.active.iter().zip(parked.states.into_iter()) {
                let view = self.views.get_mut(*idx).expect("Failed to get view");
                view.restore_parked(parked_view);
            }
        }
    }

    pub fn invalidate(&mut self) {
        self.active_apply_mut(View::invalidate);
    }

    pub fn handle_timer_event(&mut self) {
        // Let each View/Pane handle the timer event, then run update
        let mut sync_ops = Vec::new();
        for idx in &self.active {
            let view = self.views.get_mut(*idx).expect("Failed to get view");
            let view_ops = view.handle_timer_event();
            view_ops.into_iter().for_each(|s| sync_ops.push(s));
        }
        if sync_ops.len() > 0 {
            // For now we just execute the first op.
            sync_ops
                .into_iter()
                .take(1)
                .for_each(|s| self.apply_update(Some(s)));
        }
    }
}

#[derive(Debug)]
struct ParkedState {
    partition: (usize, usize),
    states: Vec<Vec<(Option<CaseMeta>, ViewState)>>,
}
