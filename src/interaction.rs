use crate::view_state::ViewState;
use async_tungstenite::tungstenite::protocol::frame;
use glutin::{
    dpi::PhysicalPosition,
    event::{ElementState, ModifiersState, MouseButton},
};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum InteractionMode {
    Zoom,
    Pan,
    Scroll,
    FastScroll,
    Wl,
    Variate,
}
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum SyncOperation {
    Scroll(i32),
}

#[derive(Debug)]
pub struct InteractionState {
    anchor: Option<PhysicalPosition<f64>>,
    mouse_position: Option<PhysicalPosition<f64>>,
    mouse_scale: f32,

    scroll_delta: Option<f32>,
    frame_acc: f32,

    left_mouse: bool,
    right_mouse: bool,
    middle_mouse: bool,
    ctrl_pressed: bool,

    mode: Option<InteractionMode>,

    image_count: Option<usize>,
    viewstate: ViewState,

    synchronized: bool,
    cine: bool,
    cine_timer: Option<std::time::Instant>,
    cine_fps: f32,
}

impl InteractionState {
    const CINE_FPS: f32 = 10f32;
    const CINE_ADJUST: f32 = 10f32;

    pub fn new() -> Self {
        InteractionState {
            anchor: None,
            mouse_position: None,
            mouse_scale: 1f32,
            scroll_delta: None,
            frame_acc: 0_f32,
            left_mouse: false,
            right_mouse: false,
            middle_mouse: false,
            ctrl_pressed: false,
            mode: None,
            image_count: None,
            viewstate: ViewState::new(),
            synchronized: false,
            cine: false,
            cine_timer: None,
            cine_fps: Self::CINE_FPS,
        }
    }

    pub fn set_image_count(&mut self, image_count: usize) {
        self.image_count = Some(image_count);
        self.viewstate.set_frame(Some(0));
    }

    pub fn handle_move(&mut self, position: PhysicalPosition<f64>, scale: f32) {
        self.mouse_position = Some(position);
        self.mouse_scale = scale;
    }

    pub fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState) {
        match button {
            MouseButton::Left => {
                self.left_mouse = state == ElementState::Pressed;
            }
            MouseButton::Right => {
                self.right_mouse = state == ElementState::Pressed;
            }
            MouseButton::Middle => {
                self.middle_mouse = state == ElementState::Pressed;
            }
            _ => {}
        }
    }

    pub fn handle_modifiers(&mut self, state: ModifiersState) {
        self.ctrl_pressed = state.ctrl();
    }

    pub fn handle_mouse_wheel(&mut self, delta: f32) {
        self.scroll_delta = Some(delta);
    }

    fn mode_from_state(&self) -> Option<InteractionMode> {
        if self.left_mouse {
            if self.right_mouse {
                return Some(InteractionMode::FastScroll);
            }
            // Pan/Zoom
            if self.ctrl_pressed {
                return Some(InteractionMode::Zoom);
            }
            return Some(InteractionMode::Pan);
        }
        if self.scroll_delta.is_some() {
            return Some(InteractionMode::Scroll);
        }
        if self.middle_mouse {
            if self.ctrl_pressed {
                return Some(InteractionMode::Variate);
            }
            return Some(InteractionMode::Wl);
        }
        None
    }

    fn same_mode(&self, mode: Option<InteractionMode>) -> bool {
        mode.map_or(self.mode.is_none(), |new| {
            self.mode.map_or(false, |old| new == old)
        })
    }

    fn update_frame(&mut self, delta: f32) -> i32 {
        self.frame_acc += delta;

        let abs_delta = self.frame_acc.abs().round() as i32;
        let delta_sign = self.frame_acc.signum() as i32;
        if abs_delta != 0 {
            self.frame_acc = 0_f32;
        }

        let current_frame = self.viewstate.frame.unwrap_or(0) as i32;
        let image_count = self.image_count.unwrap_or(1) as i32;
        let next_frame = if self.cine {
            let frame_delta = delta_sign * (abs_delta % image_count);
            let next_frame = current_frame + frame_delta;
            if next_frame < 0 {
                image_count + next_frame
            } else if next_frame >= image_count {
                next_frame - image_count
            } else {
                next_frame
            }
        } else {
            let frame_delta = delta_sign * abs_delta;
            let next_frame = current_frame + frame_delta;
            next_frame.max(0).min(image_count - 1)
        };
        // let next_frame = (current_frame + frame_delta)
        //     .max(0)
        //     .min((self.image_count.unwrap_or(1) as i32) - 1); // We need the index of the frame

        // Return true if the frame has changed.
        if next_frame != current_frame {
            self.viewstate.set_frame(Some(next_frame as u32));
        }
        next_frame - current_frame
    }

    pub fn hide_cursor(&self) -> bool {
        let mode = self.mode_from_state();
        match mode {
            Some(InteractionMode::Pan) => true,
            _ => false,
        }
    }

    pub fn update(&mut self) -> (bool, Option<SyncOperation>) {
        // Check which interaction mode we should be in. If it differs from what is set,
        // we need to "exit old"/"enter new".
        let mode = self.mode_from_state();
        let mut mode_change = false;
        if !self.same_mode(mode) {
            log::trace!("Mode change from {:?} to {:?}", self.mode, mode);
            self.mode = mode;
            // Reset anchor
            self.anchor = None;
            mode_change = true;
        }
        let anchor = self.anchor.or(self.mouse_position);
        let movement = self.mouse_position.map(|p| {
            // Sine we have a mouse position it is safe to unwrap the anchor
            let a = anchor.unwrap();
            (p.x - a.x, p.y - a.y)
        });

        let mut updated = false;
        self.viewstate.cursor = None;
        let mut sync_op = None;
        if let Some(mode) = self.mode {
            match mode {
                InteractionMode::Zoom => {
                    if let Some(movement) = movement {
                        let factor = (1_f32 - movement.1 as f32 / 256.0_f32).max(0_f32);
                        self.viewstate.update_magnification(factor);
                        updated = true;
                    }
                }
                InteractionMode::Pan => {
                    if let Some(movement) = movement {
                        let delta = (movement.0 as f32, movement.1 as f32);
                        self.viewstate.update_position(delta);
                        // If we are paning set the cursor,
                        self.viewstate.cursor =
                            self.mouse_position.map(|p| (p.x as f32, p.y as f32));
                        updated = true;
                    }
                }
                InteractionMode::Scroll => {
                    if let Some(delta) = self.scroll_delta {
                        // Move frames forward
                        let frame_diff = self.update_frame(-delta);
                        if frame_diff != 0 {
                            updated = true;
                            // Check sync?
                            if self.synchronized {
                                sync_op = Some(SyncOperation::Scroll(frame_diff));
                            }
                        }
                    }
                }
                InteractionMode::FastScroll => {
                    if let Some(movement) = movement {
                        // let delta = (movement.1 as f32 / 10.0_f32) * self.mouse_scale;
                        let delta = (movement.1 as f32)
                            * self.mouse_scale
                            * self.image_count.unwrap_or(1) as f32;
                        // Use delta to move the frames forward.
                        let frame_diff = self.update_frame(delta);
                        if frame_diff != 0 {
                            updated = true;
                            // Check sync?
                            if self.synchronized {
                                sync_op = Some(SyncOperation::Scroll(frame_diff));
                            }
                        }
                    }
                }
                InteractionMode::Wl => {
                    if let Some(movement) = movement {
                        let delta_c = (1_f32 + movement.1 as f32 / 256.0_f32).max(0_f32);
                        let delta_w = (1_f32 + movement.0 as f32 / 256.0_f32).max(0_f32);
                        self.viewstate.update_center(delta_c);
                        self.viewstate.update_width(delta_w);
                        updated = true;
                    }
                }
                InteractionMode::Variate => {
                    if let Some(movement) = movement {
                        let delta = (movement.1 as f32) * self.mouse_scale;
                        self.viewstate.update_variate(Some(delta));
                        updated = true;
                    }
                }
            }
        }

        // We should have consumed the scroll delta
        self.scroll_delta = None;
        // The state has been updated given the current mouse position
        self.anchor = self.mouse_position;
        (updated || mode_change, sync_op)
    }

    pub fn get_render_state(&self) -> ViewState {
        self.viewstate.clone()
    }

    pub fn set_render_state(&mut self, state: ViewState) {
        self.viewstate = state;
    }

    pub fn toggle_sync(&mut self) {
        self.synchronized = !self.synchronized;
    }

    pub fn is_synchronized(&self) -> bool {
        self.synchronized
    }

    pub fn toggle_cine(&mut self) {
        self.cine = !self.cine;
        self.cine_timer = if self.cine {
            Some(std::time::Instant::now())
        } else {
            None
        };
    }

    pub fn adjust_cine_speec(&mut self, direction: i32) {
        self.cine_fps += (direction as f32) * Self::CINE_ADJUST;
        println!("New cine FPS {}", self.cine_fps);
    }

    pub fn cine_update(&mut self) -> bool {
        // Check if we are in cine-mode and update the frame accoringly (by setting a scroll delta)
        // return true if we updated something.
        if self.cine {
            if let Some(timer) = &mut self.cine_timer {
                // Check how far we should move
                let frames = timer.elapsed().as_secs_f32() * self.cine_fps;
                if frames.abs() >= 1f32 {
                    self.scroll_delta = Some(-frames);
                    *timer = std::time::Instant::now();
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test() {
        let a: Option<InteractionMode> = Some(InteractionMode::Pan);
        let b: Option<InteractionMode> = None; //Some(InteractionMode::Scroll);
        let same = a.map_or(b.is_none(), |new| b.map_or(false, |old| new == old));
        assert_eq!(same, false);
    }
}
