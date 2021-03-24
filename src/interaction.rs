use glutin::{dpi::PhysicalPosition, event::{ElementState, ModifiersState, MouseButton}};

use crate::{
    app::App,
    message::RenderState,
    view_state::{self, ViewState},
};
// use winit::{
//     dpi::PhysicalPosition,
//     event::{ElementState, ModifiersState, MouseButton},
// };

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum InteractionMode {
    Zoom,
    Pan,
    Scroll,
    FastScroll,
    Wl,
}

pub struct InteractionState {
    anchor: Option<PhysicalPosition<f64>>,
    mouse_position: Option<PhysicalPosition<f64>>,
    scroll_delta: Option<f32>,

    left_mouse: bool,
    right_mouse: bool,
    middle_mouse: bool,
    ctrl_pressed: bool,

    mode: Option<InteractionMode>,

    seq: u64,
    case_key: Option<String>,
    image_count: Option<usize>,
    viewstate: ViewState,
}

impl InteractionState {
    pub fn new() -> Self {
        InteractionState {
            anchor: None,
            mouse_position: None,
            scroll_delta: None,
            left_mouse: false,
            right_mouse: false,
            middle_mouse: false,
            ctrl_pressed: false,
            mode: None,
            seq: 0,
            case_key: None,
            image_count: None,
            viewstate: ViewState::new(),
        }
    }

    pub fn set_case(&mut self, key: String, image_count: usize) {
        self.case_key = Some(key);
        self.image_count = Some(image_count);
        self.viewstate.set_frame(Some(0));
    }

    pub fn handle_move(&mut self, position: PhysicalPosition<f64>) {
        self.mouse_position = Some(position);
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
            return Some(InteractionMode::Wl);
        }
        None
    }

    fn same_mode(&self, mode: Option<InteractionMode>) -> bool {
        mode.map_or(self.mode.is_none(), |new| {
            self.mode.map_or(false, |old| new == old)
        })
    }

    fn update_frame(&mut self, delta: i32) {
        let current_frame = self.viewstate.frame.unwrap_or(0) as i32;
        // Need to check bounds.
        let next_frame = (current_frame + delta)
            .max(0)
            .min(self.image_count.unwrap_or(0) as i32);
        self.viewstate.set_frame(Some(next_frame as u32));
    }

    pub fn update(&mut self) -> bool {
        // Check which interaction mode we should be in. If it differs from what is set,
        // we need to "exit old"/"enter new".
        let mode = self.mode_from_state();
        if !self.same_mode(mode) {
            println!("Mode change from {:?} to {:?}", self.mode, mode);
            self.mode = mode;
            // Reset anchor
            self.anchor = None;
        }
        // dbg!(&self.mouse_position);
        // dbg!(&self.anchor);
        let anchor = self.anchor.or(self.mouse_position);
        let movement = self.mouse_position.map(|p| {
            // Sine we have a mouse position it is safe to unwrap the anchor
            let a = anchor.unwrap();
            (p.x - a.x, p.y - a.y)
        });
        // dbg!(movement);

        let mut updated = false;
        if let Some(mode) = self.mode {
            match mode {
                InteractionMode::Zoom => {
                    if let Some(movement) = movement {
                        let factor = movement.1 as f32 / 256.0_f32;
                        self.viewstate.update_magnification(factor);
                        updated = true;
                    }
                }
                InteractionMode::Pan => {
                    if let Some(movement) = movement {
                        let delta = (movement.0 as f32, movement.1 as f32);
                        self.viewstate.update_position(delta);
                        updated = true;
                    }
                }
                InteractionMode::Scroll => {
                    if let Some(delta) = self.scroll_delta {
                        let delta = delta.ceil() as i32;
                        // Move frames forward
                        self.update_frame(delta);
                        updated = true;
                    }
                }
                InteractionMode::FastScroll => {
                    if let Some(movement) = movement {
                        let delta = (movement.1 as f32 / 256.0_f32).ceil() as i32;
                        // Use delta to move the frames forward.
                        self.update_frame(delta);
                        updated = true;
                    }
                }
                InteractionMode::Wl => {
                    if let Some(movement) = movement {
                        let delta_c = movement.1 as f32 / 256.0_f32;
                        let delta_w = movement.0 as f32 / 256.0_f32;
                        self.viewstate.update_center(delta_c);
                        self.viewstate.update_width(delta_w);
                        updated = true;
                    }
                }
            }
        }

        // We should have consumed the scroll delta
        self.scroll_delta = None;
        // The state has been updated given the current mouse position
        self.anchor = self.mouse_position;
        updated
    }

    pub fn get_render_state(&mut self, snapshot: bool) -> RenderState {
        let cursor = match self.mode {
            Some(InteractionMode::Pan) => self.mouse_position.map(|p| (p.x as f32, p.y as f32)),
            _ => None,
        };
        self.seq += 1;
        RenderState {
            view_state: self.viewstate.clone(),
            key: self.case_key.clone(),
            seq: self.seq,
            timestamp: 0_f32,
            snapshot,
            cursor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test() {
        let a:Option<InteractionMode> = Some(InteractionMode::Pan);
        let b: Option<InteractionMode> = None;//Some(InteractionMode::Scroll);
        let same = a.map_or(b.is_none(), |new| {
            b.map_or(false, |old| new == old)
        });
        assert_eq!(same, false);

    }
}
