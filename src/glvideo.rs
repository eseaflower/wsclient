use gst_gl::VideoFrameGLExt;
use gstreamer as gst;
use gstreamer_gl as gst_gl;
use gstreamer_video as gst_video;

use view_state::Zoom;

use crate::{
    text_renderer::{Partition, TextPartition, TextRenderer},
    vertex::{self, Quad},
    view::ViewControl,
    view_state::{self, ViewState},
};

use super::bindings::gl;

use std::{
    ffi::{c_void, CString},
    mem, ptr,
};

pub struct GlRenderer {
    bindings: gl::Gl,
    image_vao: u32,
    image_vertex_buffer: u32,
    _image_index_buffer: u32,
    program_argb: u32,
    program_grey: u32,
    program_text: u32,
    quad: Quad,
    state: ViewState,
    own_ctx: gst_gl::GLContext,
    pipe_ctx: gst_gl::GLContext,
    window_size: (u32, u32),
    text_vao: u32,
    text_vertex_buffer: u32,
    text_index_buffer: u32,
    text_vertex_buffer_len: usize,
    text_renderer: TextRenderer,
}

impl GlRenderer {
    pub fn new<F>(func: F, own_ctx: gst_gl::GLContext, pipe_ctx: gst_gl::GLContext) -> Self
    where
        F: FnMut(&'static str) -> *const c_void,
    {
        let bindings = gl::Gl::load_with(func);
        Self::with_bindings(bindings, own_ctx, pipe_ctx)
    }

    pub fn with_bindings(
        bindings: gl::Gl,
        own_ctx: gst_gl::GLContext,
        pipe_ctx: gst_gl::GLContext,
    ) -> Self {
        unsafe { Self::create(bindings, own_ctx, pipe_ctx) }
    }

    unsafe fn create(
        bindings: gl::Gl,
        own_ctx: gst_gl::GLContext,
        pipe_ctx: gst_gl::GLContext,
    ) -> Self {
        let program_argb = Self::compile_program(
            &bindings,
            include_str!("shaders/glvert.glsl"),
            include_str!("shaders/glfrag_argb_scaling.glsl"),
            // include_str!("shaders/glfrag_argb.glsl"),
        );

        // This program is not used anymore!
        let program_grey = Self::compile_program(
            &bindings,
            include_str!("shaders/glvert.glsl"),
            include_str!("shaders/glfrag_argb_grey.glsl"),
        );
        let program_text = Self::compile_program(
            &bindings,
            include_str!("shaders/glvert_text.glsl"),
            include_str!("shaders/glfrag_text.glsl"),
        );
        let (image_vao, image_vertex_buffer, image_index_buffer) =
            Self::create_vao(&bindings, true);
        // We need dynamic sizes of the vertex-/index-buffers.
        let (text_vao, text_vertex_buffer, text_index_buffer) = Self::create_vao(&bindings, false);

        let text_renderer = TextRenderer::new(&bindings);
        let mut state = ViewState::new();
        state.set_zoom_mode(Zoom::Pixel(1.0_f32));

        Self {
            bindings,
            image_vao,
            image_vertex_buffer,
            _image_index_buffer: image_index_buffer,
            program_argb,
            program_grey,
            quad: Quad::default(),
            state,
            own_ctx,
            pipe_ctx,
            window_size: (0, 0),
            program_text,
            text_vao,
            text_vertex_buffer,
            text_vertex_buffer_len: 0,
            text_index_buffer,
            text_renderer,
        }
    }

    unsafe fn compile_program(bindings: &gl::Gl, vs_src: &str, fs_src: &str) -> u32 {
        let vs = Self::compile_shader(bindings, vs_src, gl::VERTEX_SHADER);
        let fs = Self::compile_shader(bindings, fs_src, gl::FRAGMENT_SHADER);

        let program = bindings.CreateProgram();
        bindings.AttachShader(program, vs);
        bindings.AttachShader(program, fs);
        bindings.LinkProgram(program);

        {
            let mut success: gl::types::GLint = 1;
            bindings.GetProgramiv(program, gl::LINK_STATUS, &mut success);
            assert!(success != 0);
        }
        bindings.DetachShader(program, vs);
        bindings.DeleteShader(vs);
        bindings.DetachShader(program, fs);
        bindings.DeleteShader(fs);
        program
    }

    unsafe fn compile_shader(bindings: &gl::Gl, src: &str, shader_type: gl::types::GLenum) -> u32 {
        let shader = bindings.CreateShader(shader_type);
        let shader_src = CString::new(src).expect("Failed to include vertex shader source");
        // bindings.ShaderSource(vs, 1, [VS_SRC.as_ptr() as *const _].as_ptr(), ptr::null());
        bindings.ShaderSource(shader, 1, [shader_src.as_ptr() as _].as_ptr(), ptr::null());
        bindings.CompileShader(shader);
        {
            let mut success: gl::types::GLint = 1;
            bindings.GetShaderiv(shader, gl::COMPILE_STATUS, &mut success);
            assert!(success != 0);
        }
        shader
    }
    unsafe fn create_vao(bindings: &gl::Gl, single_quad: bool) -> (u32, u32, u32) {
        // Generate Vertex Array Object, this stores buffers/pointers/indexes
        let mut vao = mem::MaybeUninit::uninit();
        bindings.GenVertexArrays(1, vao.as_mut_ptr());
        let vao = vao.assume_init();
        // Bind the VAO (it "records" which buffers to use to draw)
        bindings.BindVertexArray(vao);

        // Create Vertex Buffer
        let mut quad_vertex_buffer = mem::MaybeUninit::uninit();
        bindings.GenBuffers(1, quad_vertex_buffer.as_mut_ptr());
        let quad_vertex_buffer = quad_vertex_buffer.assume_init();
        bindings.BindBuffer(gl::ARRAY_BUFFER, quad_vertex_buffer);
        // For a single quad we can allocate the buffer directly
        if single_quad {
            bindings.BufferData(
                gl::ARRAY_BUFFER,
                (Quad::VERTICES.len() * mem::size_of::<vertex::Vertex>()) as _,
                // vertex::VERTICES.as_ptr() as _,
                ptr::null() as _,
                gl::STREAM_DRAW,
            );
        }

        // Create Index Buffer
        let mut quad_index_buffer = mem::MaybeUninit::uninit();
        bindings.GenBuffers(1, quad_index_buffer.as_mut_ptr());
        let quad_index_buffer = quad_index_buffer.assume_init();
        bindings.BindBuffer(gl::ELEMENT_ARRAY_BUFFER, quad_index_buffer);
        // For a single quad we can allocate and fill the buffer statically.
        if single_quad {
            bindings.BufferData(
                gl::ELEMENT_ARRAY_BUFFER,
                (Quad::INDICES.len() * mem::size_of::<u16>()) as _,
                Quad::INDICES.as_ptr() as _, // Set the index buffer statically
                gl::STATIC_DRAW,
            );
        }
        // Setup attribute pointers while the VAO is bound to record this.

        // The position is in layout=0 in the shader
        bindings.VertexAttribPointer(
            0,
            vertex::NUM_VERTEX_COORDS as _,
            gl::FLOAT,
            gl::FALSE,
            mem::size_of::<vertex::Vertex>() as _,
            ptr::null(),
        );
        // Texture coords in layout=1
        bindings.VertexAttribPointer(
            1,
            vertex::NUM_TEX_COORDS as _,
            gl::FLOAT,
            gl::FALSE,
            mem::size_of::<vertex::Vertex>() as _,
            (vertex::NUM_VERTEX_COORDS * mem::size_of::<f32>()) as _,
        );
        // Enable attribute 0
        bindings.EnableVertexAttribArray(0);
        bindings.EnableVertexAttribArray(1);

        // Unbind the VAO BEFORE! unbinding the vertex- and index-buffers
        bindings.BindVertexArray(0);
        bindings.BindBuffer(gl::ELEMENT_ARRAY_BUFFER, 0);
        bindings.BindBuffer(gl::ARRAY_BUFFER, 0);
        bindings.DisableVertexAttribArray(0);
        bindings.DisableVertexAttribArray(1);
        (vao, quad_vertex_buffer, quad_index_buffer)
    }
    // unsafe fn create_vao(bindings: &gl::Gl) -> (u32, u32, u32) {
    //     // Generate Vertex Array Object, this stores buffers/pointers/indexes
    //     let mut vao = mem::MaybeUninit::uninit();
    //     bindings.GenVertexArrays(1, vao.as_mut_ptr());
    //     let vao = vao.assume_init();
    //     // Bind the VAO (it "records" which buffers to use to draw)
    //     bindings.BindVertexArray(vao);

    //     // Create Vertex Buffer
    //     let mut quad_vertex_buffer = mem::MaybeUninit::uninit();
    //     bindings.GenBuffers(1, quad_vertex_buffer.as_mut_ptr());
    //     let quad_vertex_buffer = quad_vertex_buffer.assume_init();
    //     bindings.BindBuffer(gl::ARRAY_BUFFER, quad_vertex_buffer);
    //     bindings.BufferData(
    //         gl::ARRAY_BUFFER,
    //         (Quad::VERTICES.len() * mem::size_of::<vertex::Vertex>()) as _,
    //         // vertex::VERTICES.as_ptr() as _,
    //         ptr::null() as _,
    //         gl::STREAM_DRAW,
    //     );

    //     // Create Index Buffer
    //     let mut quad_index_buffer = mem::MaybeUninit::uninit();
    //     bindings.GenBuffers(1, quad_index_buffer.as_mut_ptr());
    //     let quad_index_buffer = quad_index_buffer.assume_init();
    //     bindings.BindBuffer(gl::ELEMENT_ARRAY_BUFFER, quad_index_buffer);
    //     bindings.BufferData(
    //         gl::ELEMENT_ARRAY_BUFFER,
    //         (Quad::INDICES.len() * mem::size_of::<u16>()) as _,
    //         Quad::INDICES.as_ptr() as _, // Set the index buffer statically
    //         gl::STATIC_DRAW,
    //     );
    //     // Setup attribute pointers while the VAO is bound to record this.

    //     // The position is in layout=0 in the shader
    //     bindings.VertexAttribPointer(
    //         0,
    //         vertex::NUM_VERTEX_COORDS as _,
    //         gl::FLOAT,
    //         gl::FALSE,
    //         mem::size_of::<vertex::Vertex>() as _,
    //         ptr::null(),
    //     );
    //     // Texture coords in layout=1
    //     bindings.VertexAttribPointer(
    //         1,
    //         vertex::NUM_TEX_COORDS as _,
    //         gl::FLOAT,
    //         gl::FALSE,
    //         mem::size_of::<vertex::Vertex>() as _,
    //         (vertex::NUM_VERTEX_COORDS * mem::size_of::<f32>()) as _,
    //     );
    //     // Enable attribute 0
    //     bindings.EnableVertexAttribArray(0);
    //     bindings.EnableVertexAttribArray(1);

    //     // Unbind the VAO BEFORE! unbinding the vertex- and index-buffers
    //     bindings.BindVertexArray(0);
    //     bindings.BindBuffer(gl::ELEMENT_ARRAY_BUFFER, 0);
    //     bindings.BindBuffer(gl::ARRAY_BUFFER, 0);
    //     bindings.DisableVertexAttribArray(0);
    //     bindings.DisableVertexAttribArray(1);
    //     (vao, quad_vertex_buffer, quad_index_buffer)
    // }

    // unsafe fn update_vertex_buffer(&self, buffer: u32, vertices: &[vertex::Vertex]) {
    //     assert!(vertices.len() == Quad::VERTICES.len()); // Make sure the vertices match
    //     self.bindings.BindBuffer(gl::ARRAY_BUFFER, buffer);
    //     self.bindings.BufferSubData(
    //         gl::ARRAY_BUFFER,
    //         0,
    //         (vertices.len() * mem::size_of::<vertex::Vertex>()) as _,
    //         vertices.as_ptr() as _,
    //     );

    //     self.bindings.BindBuffer(gl::ARRAY_BUFFER, 0);
    // }

    // unsafe fn update_image_vertex_buffer(&self, vertices: &[vertex::Vertex]) {
    //     self.update_vertex_buffer(self.image_vertex_buffer, vertices);
    // }

    unsafe fn update_vertex_buffer(&self, buffer: u32, vertices: &[vertex::Vertex]) {
        self.bindings.BindBuffer(gl::ARRAY_BUFFER, buffer);
        self.bindings.BufferSubData(
            gl::ARRAY_BUFFER,
            0,
            (vertices.len() * mem::size_of::<vertex::Vertex>()) as _,
            vertices.as_ptr() as _,
        );

        self.bindings.BindBuffer(gl::ARRAY_BUFFER, 0);
    }

    unsafe fn update_image_vertex_buffer(&self, vertices: &[vertex::Vertex]) {
        assert!(vertices.len() == Quad::VERTICES.len()); // Make sure the vertices match
        self.update_vertex_buffer(self.image_vertex_buffer, vertices);
    }

    unsafe fn update_text_vertex_buffer(&mut self, vertices: &[vertex::Vertex], indicies: &[u16]) {
        if vertices.len() > self.text_vertex_buffer_len {
            // Need to allocate a new buffer.
            self.bindings
                .BindBuffer(gl::ARRAY_BUFFER, self.text_vertex_buffer);
            self.bindings.BufferData(
                gl::ARRAY_BUFFER,
                (vertices.len() * mem::size_of::<vertex::Vertex>()) as _,
                vertices.as_ptr() as _,
                gl::STREAM_DRAW,
            );
            self.bindings.BindBuffer(gl::ARRAY_BUFFER, 0);
            self.text_vertex_buffer_len = vertices.len();

            // Update the size of the index buffer
            self.bindings
                .BindBuffer(gl::ELEMENT_ARRAY_BUFFER, self.text_index_buffer);
            self.bindings.BufferData(
                gl::ELEMENT_ARRAY_BUFFER,
                (indicies.len() * mem::size_of::<u16>()) as _,
                indicies.as_ptr() as _, // Set the index buffer statically
                gl::STREAM_DRAW,
            );
            self.bindings.BindBuffer(gl::ELEMENT_ARRAY_BUFFER, 0);
        } else {
            // We have enough space in the existing buffers.
            self.bindings
                .BindBuffer(gl::ARRAY_BUFFER, self.text_vertex_buffer);
            self.bindings.BufferSubData(
                gl::ARRAY_BUFFER,
                0,
                (vertices.len() * mem::size_of::<vertex::Vertex>()) as _,
                vertices.as_ptr() as _,
            );
            self.bindings.BindBuffer(gl::ARRAY_BUFFER, 0);
            // Move index data
            self.bindings
                .BindBuffer(gl::ELEMENT_ARRAY_BUFFER, self.text_index_buffer);
            self.bindings.BufferSubData(
                gl::ELEMENT_ARRAY_BUFFER,
                0,
                (indicies.len() * mem::size_of::<u16>()) as _,
                indicies.as_ptr() as _,
            );
            self.bindings.BindBuffer(gl::ELEMENT_ARRAY_BUFFER, 0);
        }
    }

    unsafe fn draw_image(&self, vertices: &[vertex::Vertex], image_texture: u32, use_grey: bool) {
        // Update the vertex buffer
        self.update_image_vertex_buffer(vertices);

        if use_grey {
            // Use a shader that ensures real greys!
            log::warn!("Using a forced grey shader");
            self.bindings.UseProgram(self.program_grey);
        } else {
            self.bindings.UseProgram(self.program_argb);
        }
        self.bindings.BindVertexArray(self.image_vao);

        // Activate and bind the textures
        self.bindings.ActiveTexture(gl::TEXTURE0); // Activate texture unit 0
        self.bindings.BindTexture(gl::TEXTURE_2D, image_texture);

        // Set texture parameters on the sent in texture!
        self.bindings
            .TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as _);
        self.bindings
            .TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as _);
        self.bindings
            .TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as _);
        self.bindings
            .TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as _);


        self.bindings
            .DrawElements(gl::TRIANGLES, 6, gl::UNSIGNED_SHORT, ptr::null());

        // Unbind resources
        self.bindings.BindVertexArray(0);
        self.bindings.ActiveTexture(gl::TEXTURE0); // Activate texture unit 0
        self.bindings.BindTexture(gl::TEXTURE_2D, 0);
        self.bindings.UseProgram(0);
    }
    // unsafe fn draw_pointer(&self, vertices: &[vertex::Vertex]) {
    //     // Enable blending to get a transparent pointer
    //     self.bindings
    //         .BlendFunc(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA);
    //     self.bindings.Enable(gl::BLEND);

    //     // Update the vertex buffer
    //     self.update_pointer_vertex_buffer(vertices);

    //     self.bindings.UseProgram(self.program_argb);
    //     self.bindings.BindVertexArray(self.pointer_vao);

    //     // Activate and bind the textures
    //     self.bindings.ActiveTexture(gl::TEXTURE0); // Activate texture unit 0
    //     self.bindings
    //         .BindTexture(gl::TEXTURE_2D, self.pointer_texture);

    //     self.bindings
    //         .DrawElements(gl::TRIANGLES, 6, gl::UNSIGNED_SHORT, ptr::null());

    //     // Unbind resources
    //     self.bindings.BindVertexArray(0);
    //     self.bindings.ActiveTexture(gl::TEXTURE0); // Activate texture unit 0
    //     self.bindings.BindTexture(gl::TEXTURE_2D, 0);
    //     self.bindings.UseProgram(0);
    //     self.bindings.Disable(gl::BLEND);
    // }
    unsafe fn draw_text(&mut self, text: Vec<TextPartition>) {
        // Get the viewport size from the first
        let viewport_size = text
            .first()
            .map(|p| p.viewport())
            .expect("Failed to get viewport size from partition");

        // Get the dynamic content from the text renderer
        let (texture_id, vertices, indicies) = self.text_renderer.draw(
            &self.bindings,
            text.iter().map(|partition| partition.section()).collect(),
            viewport_size,
        );

        // Update the vertex and index buffers.
        self.update_text_vertex_buffer(&vertices, &indicies);

        // let err = self.bindings.GetError();
        // assert_eq!(err, gl::NO_ERROR);

        // Enable blending to get a transparent pointer
        self.bindings
            .BlendFunc(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA);
        self.bindings.Enable(gl::BLEND);

        self.bindings.UseProgram(self.program_text);
        self.bindings.BindVertexArray(self.text_vao);

        // Activate and bind the textures
        self.bindings.ActiveTexture(gl::TEXTURE0); // Activate texture unit 0
        self.bindings.BindTexture(gl::TEXTURE_2D, texture_id);

        self.bindings.DrawElements(
            gl::TRIANGLES,
            indicies.len() as _,
            gl::UNSIGNED_SHORT,
            ptr::null(),
        );

        // Unbind resources
        self.bindings.BindVertexArray(0);
        self.bindings.ActiveTexture(gl::TEXTURE0); // Activate texture unit 0
        self.bindings.BindTexture(gl::TEXTURE_2D, 0);
        self.bindings.UseProgram(0);
        self.bindings.Disable(gl::BLEND);
    }

    pub fn draw(
        &mut self,
        image_vertices: Vec<vertex::Vertex>,
        image_texture: u32,
        use_grey: bool,
        text: Option<Vec<TextPartition>>,
    ) {
        unsafe {
            // Draw the image
            self.draw_image(&image_vertices, image_texture, use_grey);
            // Place to draw the cursor (remember alpha blend)?
            // if let Some(pointer_vertices) = pointer_vertices {
            //     self.draw_pointer(&pointer_vertices);
            // }
            if let Some(text) = text {
                self.draw_text(text);
            }
        }
    }

    pub fn clear(&self) {
        unsafe {
            self.bindings.ClearColor(1.0, 0.0, 0.0, 1.0);
            self.bindings.Clear(gl::COLOR_BUFFER_BIT);
        }
    }

    pub fn set_viewport_size(&mut self, size: (f32, f32)) {
        self.quad.set_viewport_size(size);
    }
    pub fn set_frame_size(&mut self, size: (f32, f32)) {
        // We assume that the texture has the same size as the frame!
        self.quad.map_texture_coords(size, size);
    }

    pub fn render(
        &mut self,
        sample: gst::Sample,
        use_grey: bool,
        text: Option<Vec<TextPartition>>,
    ) {
        // Get the texture id from the sample.

        let buffer = sample.get_buffer_owned().unwrap();
        let info = sample
            .get_caps()
            .and_then(|caps| gst_video::VideoInfo::from_caps(caps).ok())
            .unwrap();
        {
            // Set a sync point on the pipeline context. Sync point informs us
            // of when the texture from the pipe is ready.
            let sync_meta = buffer.get_meta::<gst_gl::GLSyncMeta>().unwrap();
            sync_meta.set_sync_point(&self.pipe_ctx);
        }

        if let Ok(frame) = gst_video::VideoFrame::from_buffer_readable_gl(buffer, &info) {
            // Get the sync meta from the frame.
            let sync_meta = frame
                .buffer()
                .get_meta::<gst_gl::GLSyncMeta>()
                .expect("Failed to get sync meta");

            // Insert a wait for the pipe context on our own context
            // This ensures that the render we do can access the textures
            // that the pipeline produces.
            sync_meta.wait(&self.own_ctx);
            if let Some(image_texture) = frame.get_texture_id(0) {
                log::trace!("Got frame texture with id {}", image_texture);

                // Compute the vertices to use
                let image_vertices = self.quad.get_vertex(&self.state);
                self.draw(image_vertices, image_texture, use_grey, text);
            }
        }
    }

    pub fn render_views(&mut self, control: &ViewControl) {
        // Clear the window back-buffer before setting the scissor box.
        // This ensures that the entire view is cleared.
        self.clear();

        unsafe {
            self.bindings.Enable(gl::SCISSOR_TEST);
        }

        // Get the position of the ViewControl
        let control_layout = control.get_layout();

        let view_samples = control.active_map(|view| {
            (
                view.get_current_sample(),
                view.get_layout(),
                view.get_timestamp(),
            )
        });

        for (sample, view_layout, timestamp) in view_samples {
            // Check if we have a sample

            let view_size = (view_layout.width as f32, view_layout.height as f32);
            self.set_viewport_size(view_size);
            self.set_frame_size(view_size);

            let text = if log::log_enabled!(log::Level::Debug) {
                let mut text = TextPartition::new(Partition::BR, view_size);
                text.add_text(vec![&format!("C: {}", timestamp), "_"]);
                Some(vec![text])
            } else {
                None
            };

            // Compute the postion for the view
            let top = control_layout.y + view_layout.y;
            let left = control_layout.x + view_layout.x;
            unsafe {
                // Translate to GL coordinates. This can be negative if the window
                // is smaller than the views.
                let gl_y = self.window_size.1 as i32 - (top + view_layout.height) as i32;
                // Set transformation
                self.bindings.Viewport(
                    left as _,
                    gl_y as _,
                    view_layout.width as _,
                    view_layout.height as _,
                );
                // Set scissor box
                self.bindings.Scissor(
                    left as _,
                    gl_y as _,
                    view_layout.width as _,
                    view_layout.height as _,
                );
            }

            // Do the render, if there is a sample
            sample.map(|sample| self.render(sample.sample, false, text));
        }
        unsafe {
            self.bindings.Disable(gl::SCISSOR_TEST);
        }
    }

    pub fn set_window_size(&mut self, size: (u32, u32)) {
        self.window_size = size;
    }
}
