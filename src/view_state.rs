use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
#[serde(rename_all = "lowercase")]
pub struct ViewState {
    pub zoom: Zoom,
    pub pos: Position,
    pub frame: Option<u32>,
    pub wl: Wl,
    pub cursor: Option<(f32, f32)>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    Relative((f32, f32)),
    Aboslute((f32, f32)),
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Zoom {
    Fit(f32),
    Pixel(f32),
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
#[serde(rename_all = "lowercase")]
pub struct Wl {
    pub width: f32,
    pub center: f32,
}

impl ViewState {
    pub fn new() -> Self {
        ViewState {
            zoom: Zoom::Fit(1.0),
            pos: Position::Relative((0.0, 0.0)),
            frame: None,
            wl: Wl {
                width: 1.0,
                center: 1.0,
            },
            cursor: None,
        }
    }

    pub fn for_pointer(position: Option<(f32, f32)>) -> Option<Self> {
        if let Some(position) = position {
            return Some(ViewState {
                zoom: Zoom::Pixel(1.0),
                pos: Position::Aboslute(position),
                frame: None,
                wl: Wl {
                    width: 1.0,
                    center: 1.0,
                },
                cursor: None,
            });
        }
        None
    }

    pub fn scale(&self, scale: f32) -> Self {
        // The view state is given relative to the viewport size
        // if we use a different sized viewport we need to scale
        // some aspects of the state.
        let zoom = match self.zoom {
            Zoom::Fit(z) => Zoom::Fit(z), // Fit is already relative to the viewport size, no change
            Zoom::Pixel(z) => Zoom::Pixel(z * scale),
        };
        let pos = match self.pos {
            Position::Relative(p) => Position::Relative((p.0 * scale, p.1 * scale)),
            Position::Aboslute(p) => Position::Aboslute((p.0 * scale, p.1 * scale)),
        };
        Self {
            zoom,
            pos,
            frame: self.frame,
            wl: self.wl,
            cursor: None,
        }
    }

    pub fn set_zoom_mode(&mut self, z: Zoom) {
        self.zoom = z;
    }

    pub fn update_magnification(&mut self, mag: f32) {
        match self.zoom {
            Zoom::Fit(ref mut current) => *current *= mag,
            Zoom::Pixel(ref mut current) => *current *= mag,
        }
    }

    pub fn set_position(&mut self, pos: (f32, f32)) {
        match self.pos {
            Position::Relative(ref mut p) => *p = pos,
            Position::Aboslute(ref mut p) => *p = pos,
        }
    }

    pub fn update_position(&mut self, delta: (f32, f32)) {
        match self.pos {
            Position::Relative(ref mut p) => *p = (p.0 + delta.0, p.1 + delta.1),
            Position::Aboslute(ref mut p) => *p = (p.0 + delta.0, p.1 + delta.1),
        }
    }

    pub fn set_frame(&mut self, frame: Option<u32>) {
        self.frame = frame;
    }

    pub fn update_center(&mut self, scale: f32) {
        self.wl.center *= scale;
    }

    pub fn update_width(&mut self, scale: f32) {
        self.wl.width *= scale;
    }
}
