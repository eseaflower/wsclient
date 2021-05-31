use std::{mem, ptr};

use glyph_brush::{ab_glyph::FontArc, HorizontalAlign, Layout, Section, Text, VerticalAlign};

use super::{
    bindings::gl,
    vertex::{Quad, Vertex},
    view_state::ViewState,
};
#[derive(Debug, Clone)]
struct GlyphQuad {
    vertices: Vec<Vertex>,
}
pub struct TextRenderer {
    cached_quads: Vec<GlyphQuad>,
    glyph_texture: u32,
    glyph_texture_width: u32,
    glyph_texture_height: u32,
    glyph_brush: glyph_brush::GlyphBrush<GlyphQuad>,
}
impl TextRenderer {
    pub fn new(bindings: &gl::Gl) -> Self {
        // let font =
        //     FontArc::try_from_slice(include_bytes!("../../fonts/open-sans/OpenSans-Regular.ttf"))
        //         .expect("Failed to load font");
        let font = FontArc::try_from_slice(include_bytes!("../fonts/segoe-ui/Segoe UI.ttf"))
            .expect("Failed to load font");
        let glyph_brush = glyph_brush::GlyphBrushBuilder::using_font(font).build();
        // Create the texture handle
        let glyph_texture = Self::create_glyph_texture(bindings);
        let glyph_texture_width = 256;
        let glyph_texture_height = 256;
        // Allocate the default size (256,256) for the glyph texture
        Self::allocate_glyph_texture(
            bindings,
            glyph_texture,
            glyph_texture_width,
            glyph_texture_height,
        );
        Self {
            glyph_brush,
            glyph_texture,
            glyph_texture_width,
            glyph_texture_height,
            cached_quads: Vec::default(),
        }
    }

    fn create_glyph_texture(bindings: &gl::Gl) -> u32 {
        unsafe {
            let mut texture_id = mem::MaybeUninit::uninit();
            bindings.GenTextures(1, texture_id.as_mut_ptr());
            let texture_id = texture_id.assume_init();
            texture_id
        }
    }

    fn allocate_glyph_texture(bindings: &gl::Gl, texture_id: u32, width: u32, height: u32) {
        // Create a texture and load the pointer-image into it.
        unsafe {
            bindings.BindTexture(gl::TEXTURE_2D, texture_id);
            // Set texture filter params
            bindings.TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as _);
            bindings.TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as _);
            bindings.TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as _);
            bindings.TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as _);
            // Create the Texture object empty
            bindings.TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::R8 as _,
                width as _,
                height as _,
                0,
                gl::RED,
                gl::UNSIGNED_BYTE,
                ptr::null() as _,
            );
            bindings.BindTexture(gl::TEXTURE_2D, 0);
        }
    }

    pub fn draw(
        &mut self,
        bindings: &gl::Gl,
        sections: Vec<Section>,
        viewport_size: (f32, f32),
    ) -> (u32, Vec<Vertex>, Vec<u16>) {
        // Queue all text render operations
        for s in sections {
            self.glyph_brush.queue(s);
        }

        loop {
            // Get the current glyph texture dims (needs to be in the loop in case we recreate the glyph texture)
            let current_texture_size = (
                self.glyph_texture_width as f32,
                self.glyph_texture_height as f32,
            );
            // Get a copy of the texture id to prevent 'self' from beeing borrowed
            let glyph_texture = self.glyph_texture;

            // Process the queued texts.
            let draw_result = self.glyph_brush.process_queued(
                |glyph_rect, glyph_data| unsafe {
                    // Set proper alignment (do we need this on every store?)
                    bindings.PixelStorei(gl::UNPACK_ALIGNMENT, 1);
                    bindings.TextureSubImage2D(
                        glyph_texture,
                        0,
                        glyph_rect.min[0] as _,
                        glyph_rect.min[1] as _,
                        glyph_rect.width() as _,
                        glyph_rect.height() as _,
                        gl::RED,
                        gl::UNSIGNED_BYTE,
                        glyph_data.as_ptr() as _,
                    );
                },
                |glyph_vertex| {
                    let mut quad = Quad::new();
                    quad.set_viewport_size(viewport_size);
                    // Get texture coordinates
                    let tex_coords = glyph_vertex.tex_coords;
                    let glyph_width = tex_coords.width() * current_texture_size.0;
                    let glyph_height = tex_coords.height() * current_texture_size.1;
                    quad.map_texture_coords_with_offset(
                        (glyph_width, glyph_height),
                        current_texture_size,
                        (tex_coords.min.x, tex_coords.min.y),
                    );

                    // Create a view state with correct translation
                    let pixel_coords = glyph_vertex.pixel_coords;
                    let center_x = (pixel_coords.min.x + pixel_coords.max.x) / 2.0;
                    let center_y = (pixel_coords.min.y + pixel_coords.max.y) / 2.0;
                    let view_state = ViewState::for_pointer(Some((center_x, center_y))).unwrap();
                    let vertices = quad.get_vertex(&view_state);
                    // let vertices = vec![
                    //     Vertex::debug_new(-1.0_f32, -1.0_f32),
                    //     Vertex::debug_new(-1.0_f32, 1.0_f32),
                    //     Vertex::debug_new(1.0_f32, 1.0_f32),
                    //     Vertex::debug_new(1.0_f32, -1.0_f32),
                    // ];

                    // dbg!(&glyph_vertex);
                    // dbg!(&vertices);
                    // dbg!(&current_texture_size);

                    GlyphQuad { vertices }
                },
            );

            // Handle the result, break out of the loop on success.
            match draw_result {
                Ok(glyph_brush::BrushAction::Draw(quads)) => {
                    self.cached_quads = quads;
                    break;
                }
                Ok(glyph_brush::BrushAction::ReDraw) => {
                    break;
                }
                Err(glyph_brush::BrushError::TextureTooSmall { suggested }) => {
                    // Allocate square textures.
                    let max_dim = suggested.0.max(suggested.1);
                    let power = (max_dim as f32).log2().ceil();
                    let dim = 2.0_f32.powf(power) as u32;

                    // Create a larger texture
                    Self::allocate_glyph_texture(bindings, self.glyph_texture, dim, dim);
                    self.glyph_texture_width = dim;
                    self.glyph_texture_height = dim;
                    self.glyph_brush.resize_texture(dim, dim);
                }
            }
        }

        // At this point we should have a result in the cached_quads and glyph_texture
        // Merge all quads into a single draw call.
        let mut merged_vertices = Vec::new();
        let mut merged_indices = Vec::new();
        for (i, q) in self.cached_quads.iter().enumerate() {
            // Add all vertices to the merged list
            q.vertices
                .iter()
                .for_each(|v| merged_vertices.push(v.clone()));
            // Replicate the indices from quad, with an offset into the merged
            let index_offset = (i * 4) as u16;
            Quad::INDICES
                .iter()
                .for_each(|idx| merged_indices.push(idx + index_offset));
        }
        // Return the result of the draw (texture, vertices and indicies)
        (self.glyph_texture, merged_vertices, merged_indices)
    }
}

#[derive(Debug, Clone)]
pub enum Partition {
    TL,
    TR,
    BL,
    BR,
}

impl Partition {
    fn screen_position(&self, viewport_size: (f32, f32)) -> (f32, f32) {
        match self {
            Partition::TL => (0_f32, 0_f32),
            Partition::TR => (viewport_size.0, 0_f32),
            Partition::BL => (0_f32, viewport_size.1),
            Partition::BR => (viewport_size.0, viewport_size.1),
        }
    }
    fn bounds(&self, viewport_size: (f32, f32)) -> (f32, f32) {
        (viewport_size.0 / 2_f32, viewport_size.1 / 2_f32)
    }
    fn horizontal_alignment(&self) -> HorizontalAlign {
        match self {
            Partition::TL | Partition::BL => HorizontalAlign::Left,
            Partition::TR | Partition::BR => HorizontalAlign::Right,
        }
    }
    fn vertical_alignment(&self) -> VerticalAlign {
        match self {
            Partition::TL | Partition::TR => VerticalAlign::Top,
            Partition::BL | Partition::BR => VerticalAlign::Bottom,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextPartition {
    partition: Partition,
    viewport_size: (f32, f32),
    text: Option<String>,
}

impl TextPartition {
    pub fn new(partition: Partition, viewport_size: (f32, f32)) -> Self {
        Self {
            partition,
            viewport_size,
            text: None,
        }
    }

    pub fn viewport(&self) -> (f32, f32) {
        self.viewport_size
    }

    fn pixel_scale(&self) -> f32 {
        // Compute the pixel scale of the text, which depends on the viewport size
        // Base on height of the viewport? 512 -> 16
        self.viewport_size.1 * 20_f32 / 512_f32
    }

    pub fn add_text(&mut self, lines: Vec<&str>) {
        self.text = Some(lines.join("\n"));
    }

    pub fn section(&self) -> Section {
        let text = if let Some(ref text) = self.text {
            text.as_str()
        } else {
            "" // Lifetimes are covariant
        };
        Section::default()
            .with_layout(
                Layout::default_wrap()
                    .h_align(self.partition.horizontal_alignment())
                    .v_align(self.partition.vertical_alignment()),
            )
            .with_screen_position(self.partition.screen_position(self.viewport_size))
            .with_bounds(self.partition.bounds(self.viewport_size))
            .add_text(Text::new(text).with_scale(self.pixel_scale()))
    }
}
