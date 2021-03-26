use glutin::{
    dpi::PhysicalSize,
    event::{self, Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopProxy},
    platform::{windows::RawHandle, ContextTraitExt},
    window::{Window, WindowBuilder},
    ContextWrapper, NotCurrent, PossiblyCurrent,
};
// use event_loop::{ControlFlow, EventLoopProxy};
use gst::{prelude::*, DebugGraphDetails};
use gst_gl::{GLContextExt, VideoFrameGLExt};
use gst_video::{VideoOverlayExt, VideoOverlayExtManual};
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_gl as gst_gl;
use gstreamer_sdp as gst_sdp;
use gstreamer_video as gst_video;
use gstreamer_webrtc as gst_webrtc;

use anyhow::Result;
use futures::channel::{self, mpsc::UnboundedSender};
use raw_window_handle::HasRawWindowHandle;
use window_message::WindowMessage;
// use winit::{
//     dpi::{PhysicalSize, Size},
//     event::{ElementState, Event, WindowEvent},
//     event_loop::{self, EventLoop},
//     window::WindowBuilder,
// };

use std::{
    convert::TryFrom,
    ops::{Add, Deref},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use crate::{element_timer::ElementTimer, glvideo::GlRenderer, view_state::ViewState, AppConfig};
use crate::{
    interaction::InteractionState,
    message::{self, AppMessage, CaseMeta, DataMessage},
};
use crate::{vertex::Quad, window_message};
use message::{ClientConfig, RenderState, ViewportSize};

#[derive(Debug)]
struct SharedState {
    proxy: Option<EventLoopProxy<WindowMessage>>,
    samples: Vec<gst::Sample>,
    timers: Vec<ElementTimer>,
}

#[derive(Debug, Clone)]
pub struct App(pub Arc<AppInner>);
#[derive(Debug)]
pub struct AppInner {
    signaller: UnboundedSender<AppMessage>,
    webrtcbin: gst::Element,
    pipeline: gst::Pipeline,
    datachannel: Mutex<Option<gst_webrtc::WebRTCDataChannel>>,
    shared: Mutex<SharedState>,
}

impl Deref for App {
    type Target = AppInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for AppInner {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
impl App {
    pub fn new(signaller: UnboundedSender<AppMessage>) -> Self {
        // let pipeline = gst::parse_launch(
        //     "videotestsrc pattern=ball is-live=true ! vp8enc deadline=1 ! rtpvp8pay pt=96 ! webrtcbin.
        //                         webrtcbin name=webrtcbin").expect("FOOOO");

        // let pipeline = pipeline.downcast::<gst::Pipeline>().expect("slkjgfslkdjf");
        // let webrtcbin = pipeline.get_by_name("webrtcbin").expect("ldfskjg");
        let pipeline = gst::Pipeline::new(None);
        let webrtcbin = gst::ElementFactory::make("webrtcbin", Some("webrtcbin"))
            .expect("Failed to create webrtcbin");
        pipeline
            .add(&webrtcbin)
            .expect("Failed to add element to pipeline");
        webrtcbin.set_property_from_str("stun-server", "stun://stun.l.google.com:19302");
        webrtcbin.set_property_from_str("bundle-policy", "max-bundle");

        // Create oneshot channels for delayed init data.

        let inner = AppInner {
            signaller,
            pipeline,
            webrtcbin,
            datachannel: Mutex::new(None),
            shared: Mutex::new(SharedState {
                proxy: None,
                samples: Vec::default(),
                timers: Vec::default(),
            }),
        };
        let app = App(Arc::new(inner));

        app.setup_ice_callback();
        app.setup_stream_callback();
        app.setup_datachannel();

        app
    }

    fn setup_datachannel(&self) {
        let weak_app = Arc::downgrade(&self.0);
        self.webrtcbin
            .connect("on-data-channel", false, move |values| {
                log::debug!("Got datachannel callback");
                if let Some(app) = weak_app.upgrade().map(App) {
                    let dc = values[1]
                        .get::<gst_webrtc::WebRTCDataChannel>()
                        .expect("Failed to get datachannel from values")
                        .unwrap();
                    *app.datachannel.lock().unwrap() = Some(dc);
                } else {
                    log::error!("Failed to upgrade weak_app");
                }
                None
            })
            .expect("Failed to attach data-channel signal");
    }

    fn setup_stream_callback(&self) {
        let weak_app = Arc::downgrade(&self.0);
        self.webrtcbin.connect_pad_added(move |_webrtc, pad| {
            log::debug!("Got webrtc pad");
            let app = weak_app.upgrade().map(App);
            if let Some(app) = app {
                if let Err(e) = app.on_incomming_stream(pad) {
                    gst::gst_element_error!(
                        app.pipeline,
                        gst::LibraryError::Failed,
                        ("Failed to handle incomming stream {:?}", e)
                    );
                }
            } else {
                log::warn!("Failed to upgrade weak_app");
            }
        });
    }

    fn on_incomming_stream(&self, pad: &gst::Pad) -> Result<()> {
        if pad.get_direction() != gst::PadDirection::Src {
            return Ok(());
        }
        log::debug!("Linking new stream");

        // let pipeline_description = "identity name=ident ! application/x-rtp, media=(string)video, clock-rate=(int)90000, encoding-name=(string)H264, payload=(int)96
        //  ! rtph264depay name=depay ! h264parse ! avdec_h264 ! videoconvert ! videoscale ! d3d11upload ! d3d11videosink";

        // let pipeline_description = "rtph264depay name=depay ! h264parse ! avdec_h264 ! d3d11upload ! d3d11convert ! d3d11videosink sync=false";
        let pipeline_description =
            "rtph264depay name=depay ! h264parse name=parse ! avdec_h264 name=decoder ! glupload name=upload ! glcolorconvert name=convert ! appsink name=appsink";
        // let pipeline_description = "rtph264depay name=depay ! h264parse ! avdec_h264 ! glupload ! glcolorconvert ! glimagesinkelement sync=false max-lateness=1 processing-deadline=1 enable-last-sample=false";

        // let pipeline_description = "rtph264depay name=depay ! h264parse ! avdec_h264 ! fakesink sync=false";
        let decodebin = gst::parse_bin_from_description(pipeline_description, true)
            .expect("Failed to parse decodebin");

        let appsink = decodebin
            .get_by_name("appsink")
            .expect("Failed to get appsink");
        let appsink = appsink
            .downcast::<gst_app::AppSink>()
            .expect("Failed to cast to appsink");

        let weak_app = Arc::downgrade(&self.0);
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    if let Some(app) = weak_app.upgrade().map(App) {
                        log::trace!("Got sample callback");
                        let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        app.queue_sample(sample);
                        Ok(gst::FlowSuccess::Ok)
                    } else {
                        Err(gst::FlowError::Error)
                    }
                })
                .build(),
        );
        // Set caps
        appsink
            .set_property("enable-last-sample", &false)
            .expect("Failed to set enable-last-sample");
        appsink
            .set_property("emit-signals", &false)
            .expect("Failed to set emit-signals");
        appsink
            .set_property("max-buffers", &1u32)
            .expect("Failed to set max-buffers");
        appsink
            .set_property("sync", &false)
            .expect("Failed to disable sync on sink");
        appsink
            .set_property("drop", &true)
            .expect("Failed to set drop on sink");

        let caps = gst::Caps::builder("video/x-raw")
            .features(&[&gst_gl::CAPS_FEATURE_MEMORY_GL_MEMORY])
            .field("format", &gst_video::VideoFormat::Rgba.to_str())
            .field("texture-target", &"2D")
            .build();
        appsink.set_caps(Some(&caps));

        self.pipeline
            .add(&decodebin)
            .expect("Failed to add decodebin element to pipeline");
        decodebin
            .sync_state_with_parent()
            .expect("Failed to sync decodebin with parent");

        let sinkpad = decodebin
            .get_static_pad("sink")
            .expect("Failed to get sink pad of decodebin");
        pad.link(&sinkpad)
            .expect("Failed to link incomming stream to decodebin");

        let depay = decodebin
            .get_by_name("parse")
            .expect("Failed to get depay");
        let convert = decodebin
            .get_by_name("convert")
            .expect("Failed to get appsink");

        let timer = ElementTimer::new("depay-convert", depay, convert);
        {
            let mut shared = self.shared.lock().unwrap();
            shared.timers.push(timer);
        }

        Ok(())
    }

    fn setup_ice_callback(&self) {
        let weak_app = Arc::downgrade(&self.0);
        self.webrtcbin
            .connect("on-ice-candidate", false, move |values| {
                let app = weak_app.upgrade().map(App).unwrap();

                let sdp_mline_index = values[1]
                    .get_some::<u32>()
                    .expect("Failed to get sdp line index");
                let candidate = values[2]
                    .get::<String>()
                    .expect("Failed to get ice candidate")
                    .unwrap();
                let msg = AppMessage::Ice {
                    sdp_mline_index,
                    candidate,
                };
                app.send_app_message(msg)
                    .expect("Failed to send ice candidate");

                None
            })
            .expect("Failed to attach signal");
    }

    fn handle_sdp(&self, type_: &str, sdp: &str) {
        if type_ != "offer" {
            panic!("Only SDP offers are supported, got: {}", type_);
        }
        log::debug!("Got SDP offer from server: {}", sdp);
        let msg =
            gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes()).expect("Failed to parse SDP offer");
        let offer =
            gst_webrtc::WebRTCSessionDescription::new(gst_webrtc::WebRTCSDPType::Offer, msg);
        self.webrtcbin
            .emit("set-remote-description", &[&offer, &None::<gst::Promise>])
            .expect("Failed to set remote description");

        let weak_app = Arc::downgrade(&self.0);
        let promise = gst::Promise::with_change_func(move |reply| {
            let app = weak_app.upgrade().map(App);
            if let Some(app) = app {
                if let Err(err) = app.on_answer_created(reply) {
                    gst::gst_element_error!(
                        app.pipeline,
                        gst::LibraryError::Failed,
                        ("Failed to send SDP answer: {:?}", err)
                    );
                }
            } else {
                log::error!("Failed to upgrade app to strong ref");
            }
        });

        self.webrtcbin
            .emit("create-answer", &[&None::<gst::Structure>, &promise])
            .expect("Failed to emit create-answer signal");
    }

    fn on_answer_created(
        &self,
        reply: Result<Option<&gst::StructureRef>, gst::PromiseError>,
    ) -> Result<()> {
        // Unwrap the reply
        let reply = match reply {
            Ok(Some(reply)) => reply,
            Ok(None) => {
                log::error!("Answer creation got no response");
                anyhow::bail!("Promise was None");
            }

            Err(e) => {
                log::error!("Error receiving answer response: {:?}", e);
                anyhow::bail!("Promise resolved to Err")
            }
        };

        let answer = reply
            .get_value("answer")
            .unwrap()
            .get::<gst_webrtc::WebRTCSessionDescription>()
            .expect("Invalid argument")
            .unwrap();
        self.webrtcbin
            .emit("set-local-description", &[&answer, &None::<gst::Promise>])
            .unwrap();

        log::debug!(
            "sending SDP answer to peer: {}",
            answer.get_sdp().as_text().unwrap()
        );

        let msg = AppMessage::Sdp {
            type_: "answer".to_string(),
            sdp: answer.get_sdp().as_text().unwrap(),
        };
        self.send_app_message(msg)
            .expect("Failed to send answer message");

        Ok(())
    }
    fn handle_ice(&self, sdp_mline_index: u32, candidate: &str) {
        self.webrtcbin
            .emit("add-ice-candidate", &[&sdp_mline_index, &candidate])
            .expect("Failed to add ice candidate");
    }

    pub fn handle_app_message(&self, msg: AppMessage) -> Result<()> {
        match msg {
            AppMessage::Sdp { type_, sdp } => self.handle_sdp(&type_, &sdp),
            AppMessage::Ice {
                sdp_mline_index,
                candidate,
            } => self.handle_ice(sdp_mline_index, &candidate),
            AppMessage::Case(cases) => {
                self.send_window_message(WindowMessage::Cases(cases));
            }
            _ => log::error!("Unexpected message {:?}", msg),
        };
        Ok(())
    }

    fn send_window_message(&self, msg: WindowMessage) {
        // Acquire the mutex, then send the message
        let shared = self.shared.lock().unwrap();
        Self::internal_send_window_message(shared.proxy.as_ref(), msg);
    }

    // Sends the window message while holding the mutex
    fn internal_send_window_message(
        proxy: Option<&EventLoopProxy<WindowMessage>>,
        msg: WindowMessage,
    ) {
        if let Some(proxy) = proxy {
            proxy
                .send_event(msg)
                .expect("Failed to send window message");
        }
    }

    // Queue the sample we received
    fn queue_sample(&self, sample: gst::Sample) {
        let mut shared = self.shared.lock().unwrap();
        // Insert the sample
        shared.samples.push(sample);
        Self::internal_send_window_message(shared.proxy.as_ref(), WindowMessage::NewSample);
    }

    // Drain all queued samples, and return the last.
    fn get_last_sample(&self) -> Option<gst::Sample> {
        let mut shared = self.shared.lock().unwrap();
        // Consume all queued samples, and return the last (if any)
        shared.samples.drain(..).last()
    }

    fn send_app_message(&self, msg: AppMessage) -> Result<()> {
        self.signaller.unbounded_send(msg).map_err(|e| e.into())
    }

    fn try_send_message(&self, msg: DataMessage) -> bool {
        if let Some(ref datachannel) = *self.datachannel.lock().unwrap() {
            match datachannel.get_property_ready_state() {
                gstreamer_webrtc::WebRTCDataChannelState::Open => {
                    if let Ok(msg) = String::try_from(msg) {
                        log::trace!("DC sending: {}", &msg);
                        datachannel.send_string(Some(&msg));
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            }
        } else {
            false
        }
    }

    fn connect(&self, config: &AppConfig) {
        let cfg = ClientConfig {
            id: "NativeClient".to_string(),
            viewport: ViewportSize {
                width: config.viewport_size.0,
                height: config.viewport_size.1,
            },
            bitrate: config.bitrate,
            gpu: config.gpu,
            preset: config.preset.clone(),
            lossless: config.lossless,
            video_scaling: config.video_scaling,
            fullrange: !config.narrow,
        };
        log::info!("Connecting with {:?}", &cfg);

        let msg = AppMessage::Connect(vec![cfg]);
        self.send_app_message(msg)
            .expect("Failed to send connect message");
    }

    fn create_shared_context(
        ctx: ContextWrapper<NotCurrent, Window>,
    ) -> (ContextWrapper<NotCurrent, Window>, gst_gl::GLContext) {
        let ctx = unsafe { ctx.make_current().expect("Failed to make context current") };

        // Build gstreamer sharable context
        let (gl_context, gl_display, platform) = match unsafe { ctx.raw_handle() } {
            RawHandle::Wgl(wgl_context) => {
                let gl_display = gst_gl::GLDisplay::new();
                (
                    wgl_context as usize,
                    gl_display.upcast::<gst_gl::GLDisplay>(),
                    gst_gl::GLPlatform::WGL,
                )
            }
            #[allow(unreachable_patterns)]
            handler => panic!("Unsupported platform: {:?}.", handler),
        };

        // The shared gstreamer context will be moved into the sync bus handler.
        let shared_context = unsafe {
            gst_gl::GLContext::new_wrapped(
                &gl_display,
                gl_context,
                platform,
                gst_gl::GLAPI::OPENGL3,
            )
        }
        .unwrap();
        println!("shared_context created");
        shared_context
            .activate(true)
            .expect("Couldn't activate wrapped GL context");
        println!("shared_context activated");

        // NOTE: The below is a likely condidate for causing Acess Violations during
        // some circumstances. We get the above printing, but not the below.
        // We are however running on multiple threads, so it might be another thread
        // that causes the crash.
        // The other thread scenario is at least very unlikely (if not impossible).
        // The App-thread is waiting to get the shared context to setup context sharing
        // in the pipeline. It will not start the pipeline until this is done.
        // The render thread is waiting for the filterapp to be initialized before
        // proceeding, so it is also blocked.

        shared_context
            .fill_info()
            .expect("Failed to fill context info");
        println!("shared_context info filled");

        let ctx = unsafe {
            ctx.make_not_current()
                .expect("Failed to uncurrent the context")
        };

        (ctx, shared_context)
    }

    fn create_renderer(
        ctx: &ContextWrapper<PossiblyCurrent, Window>,
        frame_size: (u32, u32),
        window_size: (u32, u32),
        own_ctx: gst_gl::GLContext,
        pipe_ctx: gst_gl::GLContext,
    ) -> GlRenderer {
        let mut renderer = GlRenderer::new(|name| ctx.get_proc_address(name), own_ctx, pipe_ctx);
        renderer.set_viewport_size((window_size.0 as f32, window_size.1 as f32));
        renderer.set_frame_size((frame_size.0 as f32, frame_size.1 as f32));
        renderer
    }

    fn get_pipe_context(&self) -> gst_gl::GLContext {
        let e = self
            .pipeline
            .get_by_name("upload")
            .expect("Failed to get upload element");

        e.get_property("context")
            .expect("No property 'context' found")
            .get::<gst_gl::GLContext>()
            .expect("Failed to cast to GLContext")
            .expect("Context is empty")
    }

    pub fn main_loop(self, config: AppConfig) -> Result<()> {
        log::debug!("Starting app main loop on current thread");

        let event_loop = EventLoop::<WindowMessage>::with_user_event();
        let size = config.viewport_size;
        let window_builder = WindowBuilder::new().with_inner_size(PhysicalSize {
            width: size.0,
            height: size.1,
        });

        let main_context = glutin::ContextBuilder::new()
            .with_gl(glutin::GlRequest::Specific(glutin::Api::OpenGl, (4, 5)))
            .with_gl_profile(glutin::GlProfile::Core)
            .with_vsync(false)
            .build_windowed(window_builder, &event_loop)
            .expect("Failed to build GL main context");

        // Create GStreamer context
        let (main_context, own_context) = Self::create_shared_context(main_context);

        // Set the event loop proxy on App
        {
            let mut shared = self.shared.lock().unwrap();
            shared.proxy = Some(event_loop.create_proxy());
        }
        // Start the pipeline
        self.pipeline
            .set_state(gst::State::Playing)
            .expect("Failed to set the pipeline to playing");

        // Connect to server
        self.connect(&config);

        // We really need to ensure that connect() has been handled before we send another
        // ws-request, otherwise the server might error out.
        self.send_app_message(AppMessage::GetCases)
            .expect("Failed to send GetCases");

        // Set a bus-sync handler for setting the window handle to the videorenderer

        // Handle GStreamer bus error messages
        let bus = self.pipeline.get_bus().expect("Failed to get pipeline bus");
        let window_handle = match main_context.window().raw_window_handle() {
            raw_window_handle::RawWindowHandle::Windows(h) => h.hwnd as usize,
            _ => anyhow::bail!("Unexpected window handle"),
        };

        let weak_app = Arc::downgrade(&self.0);
        let shared_context = own_context.clone();
        bus.set_sync_handler(move |_b, msg| {
            // Check if this is the message we are looking for
            if gst_video::is_video_overlay_prepare_window_handle_message(msg) {
                log::debug!(
                    "Got prepare window handle message. Current handle is {:?}",
                    window_handle
                );
                let src = msg.get_src().expect("Failed to get message source");
                let oly = src
                    .dynamic_cast::<gst_video::VideoOverlay>()
                    .expect("Failed to convert src to video overlay");

                unsafe { oly.set_window_handle(window_handle) };

                return gst::BusSyncReply::Drop;
            } else {
                match msg.view() {
                    gst::MessageView::NeedContext(ctx) => {
                        let context_type = ctx.get_context_type();
                        if context_type == "gst.gl.app_context" {
                            if let Some(el) =
                                msg.get_src().map(|s| s.downcast::<gst::Element>().unwrap())
                            {
                                log::debug!("Got request for a shared app context");
                                let mut context = gst::Context::new(context_type, true);
                                {
                                    let context = context.get_mut().unwrap();
                                    let s = context.get_mut_structure();
                                    s.set("context", &shared_context);
                                }
                                el.set_context(&context);

                                // Signal that the context is shared so we can current the main context
                                if let Some(app) = weak_app.upgrade().map(App) {
                                    app.send_window_message(WindowMessage::ContextShared);
                                }
                            }
                        }
                    }
                    gst::MessageView::Error(e) => {
                        log::error!("Pipeline error: {:?}", e);
                        // Post an error message on the message thread.
                        if let Some(app) = weak_app.upgrade().map(App) {
                            app.send_window_message(WindowMessage::PipelineError);
                        }
                    }
                    _ => {}
                }
            }

            gst::BusSyncReply::Pass
        });

        let mut cases = None;
        let mut interaction_state = InteractionState::new();
        let mut dirty = false;

        let time = std::time::Instant::now();
        let mut dump = true;

        // This is the context until we have the first sample, then we know
        // that context sharing is done and we can current the context.
        let mut tmp_ctx = Some(main_context);
        let mut main_context: Option<ContextWrapper<PossiblyCurrent, Window>> = None;
        let mut renderer: Option<GlRenderer> = None;
        let mut own_context: Option<gst_gl::GLContext> = Some(own_context);

        event_loop.run(move |event, target, flow| {
            // The actual rendering seems not dependant on this loop.
            // So we can wait for new events.
            *flow = ControlFlow::Wait;

            // Try to get the video overlay and move it into a normal reference
            match event {
                Event::UserEvent(wm) => match wm {
                    WindowMessage::Cases(new_cases) => {
                        let case_keys: Vec<_> = new_cases.iter().map(|c| c.key.clone()).collect();
                        let cases_string = case_keys.join("\n");
                        println!("Known cases:\n{}", cases_string);

                        cases = Some(new_cases);
                        let selected_case = if let Some(ref wanted) = config.case_key {
                            cases
                                .as_ref()
                                .unwrap()
                                .iter()
                                .find(|c| *c.key == *wanted)
                                .map(|c| c.clone())
                        } else {
                            cases.as_ref().unwrap().first().map(|c| c.clone())
                        };
                        if let Some(ref case) = selected_case {
                            println!("Selected case: {}", &case.key);
                            interaction_state.set_case(case.key.clone(), case.number_of_images);
                            dirty = true;
                        }
                    }
                    WindowMessage::NewSample => {
                        // When we get the first sample we can current our context
                        // and build the renderer, since now context-sharing should
                        // be set up.
                        if let Some(ctx) = tmp_ctx.take() {
                            // Move the tmp_ctx into the main context after setting it current
                            main_context = Some(unsafe {
                                ctx.make_current().expect("Failed to current context")
                            });

                            log::debug!(
                                "Using window with settings: {:?}",
                                main_context.as_ref().unwrap().get_pixel_format()
                            );

                            let own_context = own_context.take().expect("Context is empty");

                            log::debug!("Main context has been currented");
                            renderer = Some(Self::create_renderer(
                                main_context.as_ref().unwrap(),
                                size,
                                size,
                                own_context,
                                self.get_pipe_context(),
                            ));
                        }
                        // Handle all new samples!
                        if let Some(sample) = self.get_last_sample() {
                            log::trace!("Main loop got a sample");
                            if let Some(ref renderer) = renderer {
                                renderer.render(sample);
                                if let Some(ref main_context) = main_context {
                                    // Swap back-buffer
                                    main_context
                                        .swap_buffers()
                                        .expect("Failed to swap back-buffer");
                                }
                            }
                        }
                    }
                    WindowMessage::ContextShared => {}
                    WindowMessage::PipelineError => {
                        log::error!("Got error from pipeline, exiting");
                        *flow = ControlFlow::Exit;
                    }
                },
                Event::WindowEvent { event, .. } => {
                    match event {
                        WindowEvent::CloseRequested => *flow = ControlFlow::Exit,
                        WindowEvent::CursorMoved { position, .. } => {
                            interaction_state.handle_move(position);
                        }
                        WindowEvent::MouseInput { button, state, .. } => {
                            interaction_state.handle_mouse_input(button, state);
                        }
                        WindowEvent::ModifiersChanged(state) => {
                            interaction_state.handle_modifiers(state);
                        }
                        WindowEvent::MouseWheel { delta, .. } => {
                            let delta = match delta {
                                event::MouseScrollDelta::LineDelta(_, y) => y,
                                event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                            };
                            interaction_state.handle_mouse_wheel(delta);
                        }
                        _ => {}
                    }
                    // Check if we need to update the dirty flag.
                    dirty = dirty || interaction_state.update();
                }
                Event::MainEventsCleared => {
                    if dirty {
                        let state = interaction_state.get_render_state(false);
                        self.try_send_message(DataMessage::NewState(state));
                        dirty = false;
                    }

                    if time.elapsed().as_secs_f32() > 10_f32 && dump {
                        gst::debug_bin_to_dot_file(
                            &self.pipeline,
                            DebugGraphDetails::ALL,
                            "graph_3.ts",
                        );
                        dump = false;
                    }
                }
                Event::RedrawRequested(_) => {
                    // Nothing to do right now
                    // Maybe send the server request here?
                }
                _ => (),
            }
        });
    }
}
