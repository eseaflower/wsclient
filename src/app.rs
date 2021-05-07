use event::{ElementState, VirtualKeyCode};
use glutin::{
    dpi::PhysicalSize,
    event::{self, Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopProxy},
    platform::{windows::RawHandle, ContextTraitExt},
    window::{Window, WindowBuilder},
    ContextWrapper, NotCurrent, PossiblyCurrent,
};
// use event_loop::{ControlFlow, EventLoopProxy};
use gst::{prelude::*, StructureRef};
use gst_gl::{ContextGLExt, GLContextExt};
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_gl as gst_gl;
use gstreamer_sdp as gst_sdp;
use gstreamer_video as gst_video;
use gstreamer_webrtc as gst_webrtc;

use anyhow::Result;
use futures::channel::mpsc::UnboundedSender;
use std::{
    ops::Deref,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use window_message::WindowMessage;

use crate::message::AppMessage;
use crate::{
    glvideo::GlRenderer,
    util::{element_timer::ElementTimer, window_timer::start_window_timer},
    view::ViewControl,
    AppConfig,
};
use crate::{view::View, window_message};

// use super::view::ViewCollection;

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
    datachannels: Mutex<Vec<gst_webrtc::WebRTCDataChannel>>,
    shared: Mutex<SharedState>,
    view_control: ViewControl,
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
    pub fn new(signaller: UnboundedSender<AppMessage>, view_control: ViewControl) -> Self {
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

        // TODO: Remove

        // webrtcbin
        //     .set_property("latency", &200_u32)
        //     .expect("Failed to setr latency");

        webrtcbin
            .connect("on-new-transceiver", false, move |vals| {
                println!("Got transceiver callback");

                let t = vals[1]
                    .get::<gst_webrtc::WebRTCRTPTransceiver>()
                    .expect("Not the type")
                    .expect("Trans is None");
                // dbg!(&t);
                t.set_property("do-nack", &true)
                    .expect("Failed to set nack");
                None
            })
            .expect("Failed to attach handler");

        // let rtpbin = pipeline
        //     .get_by_name("rtpbin")
        //     .expect("Failed to get rtpbin");
        // rtpbin
        //     .set_property("do-retransmission", &true)
        //     .expect("Failed to set retransmission");
        // rtpbin
        //     .set_property("drop-on-latency", &false)
        //     .expect("Failed to set drop on latency");
        // rtpbin
        //     .set_property("do-lost", &true)
        //     .expect("Failed to set do-lost");
        // rtpbin.set_property_from_str("buffer-mode", "slave");

        // END TODO

        let inner = AppInner {
            signaller,
            pipeline,
            webrtcbin,
            datachannels: Mutex::new(Vec::new()),
            shared: Mutex::new(SharedState {
                proxy: None,
                samples: Vec::default(),
                timers: Vec::default(),
            }),
            view_control,
        };
        let app = App(Arc::new(inner));

        app.setup_bus_handling();
        app.setup_ice_callback();
        app.setup_stream_callback();
        app.setup_datachannel();

        app
    }

    fn setup_bus_handling(&self) {
        let bus = self.pipeline.get_bus().expect("Failed to get pipeline bus");
        let weak_app = Arc::downgrade(&self.0);
        bus.connect_message(move |_bus, msg| {
            match msg.view() {
                gst::MessageView::Error(e) => {
                    log::error!("Pipeline error: {:?}", e);
                    // Post an error message on the message thread.
                    if let Some(app) = weak_app.upgrade().map(App) {
                        app.send_window_message(WindowMessage::PipelineError);
                    }
                }
                _ => {}
            }
        });
    }

    fn setup_datachannel(&self) {
        let weak_app = Arc::downgrade(&self.0);
        self.webrtcbin
            .connect("on-data-channel", false, move |values| {
                log::debug!("Got datachannel callback");
                if let Some(app) = weak_app.upgrade().map(App) {
                    let datachannel = values[1]
                        .get::<gst_webrtc::WebRTCDataChannel>()
                        .expect("Failed to get datachannel from values")
                        .unwrap();
                    let label = datachannel
                        .get_property_label()
                        .expect("No datachannel label")
                        .to_string();
                    dbg!(&label);
                    // Find the view that wants this datachannel
                    // let view = app.views.iter().find(|v| v.data_id() == &label);
                    let view = app.view_control.find_by_label(&label);
                    match view {
                        Some(view) => {
                            log::debug!("Setting datachannel with label {}", label);
                            view.set_datachannel(datachannel);
                        }
                        None => {
                            log::warn!("Got unexpected datachannel with label {}", label);
                        }
                    }
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

        self.webrtcbin.connect_pad_removed(|_, _| {
            println!("Pad removed...");
        });
    }

    fn on_incomming_stream(&self, pad: &gst::Pad) -> Result<()> {
        if pad.get_direction() != gst::PadDirection::Src {
            return Ok(());
        }

        let transceiver = pad
            .get_property("transceiver")
            .expect("Failed to get pad property")
            .get::<gst_webrtc::WebRTCRTPTransceiver>()
            .expect("Failed to cast prop")
            .expect("Transceiver was empty");

        let mlineidx = transceiver.get_property_mlineindex();

        let view = self
            .view_control
            .find_by_id(mlineidx as usize)
            .expect(&format!(
                "Failed to find active view with index {}",
                mlineidx
            ));

        log::debug!("Linking new stream with id {}", mlineidx);
        // let pipeline_description = "identity name=ident ! application/x-rtp, media=(string)video, clock-rate=(int)90000, encoding-name=(string)H264, payload=(int)96
        //  ! rtph264depay name=depay ! h264parse ! avdec_h264 ! videoconvert ! videoscale ! d3d11upload ! d3d11videosink";

        // let pipeline_description = "rtph264depay name=depay ! h264parse ! avdec_h264 ! d3d11upload ! d3d11convert ! d3d11videosink sync=false";
        let pipeline_template =
            "rtph264depay name=depay{idx} ! h264parse name=parse{idx} ! avdec_h264 name=decoder{idx} ! glupload name=upload{idx} ! glcolorconvert name=convert{idx} ! appsink name=appsink{idx}";
        // let pipeline_description = "rtph264depay name=depay ! h264parse ! avdec_h264 ! glupload ! glcolorconvert ! glimagesinkelement sync=false max-lateness=1 processing-deadline=1 enable-last-sample=false";
        let pipeline_description = pipeline_template.replace("{idx}", &mlineidx.to_string());
        // let pipeline_description = "rtph264depay name=depay ! h264parse ! avdec_h264 ! fakesink sync=false";
        let decodebin = gst::parse_bin_from_description(&pipeline_description, true)
            .expect("Failed to parse decodebin");

        let appsink_name = format!("appsink{}", mlineidx);
        dbg!(pipeline_description);
        dbg!(&appsink_name);

        let appsink = decodebin
            .get_by_name(&appsink_name)
            .expect("Failed to get appsink");
        let appsink = appsink
            .downcast::<gst_app::AppSink>()
            .expect("Failed to cast to appsink");

        let weak_view = Arc::downgrade(&view.0);
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    if let Some(view) = weak_view.upgrade().map(View) {
                        log::trace!("Got sample callback");
                        println!("Got sample from mline {}", mlineidx);
                        let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        // Push the sample to the view.
                        view.push_sample(sample);
                        Ok(gst::FlowSuccess::Ok)
                    } else {
                        log::error!("Failed to upgrade view");
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

        let sinkpad = decodebin
            .get_static_pad("sink")
            .expect("Failed to get sink pad of decodebin");
        pad.link(&sinkpad)
            .expect("Failed to link incomming stream to decodebin");

        decodebin
            .sync_state_with_parent()
            .expect("Failed to sync decodebin with parent");

        let depay = decodebin
            .get_by_name(&format!("parse{}", mlineidx))
            .expect("Failed to get depay");
        let convert = decodebin
            .get_by_name(&format!("convert{}", mlineidx))
            .expect("Failed to get appsink");

        let timer = ElementTimer::new(&format!("depay-convert{}", mlineidx), depay, convert);
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

                dbg!(&msg);

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
        if let Some(proxy) = shared.proxy.as_ref() {
            proxy
                .send_event(msg)
                .expect("Failed to send window message");
        }
    }

    fn send_app_message(&self, msg: AppMessage) -> Result<()> {
        self.signaller.unbounded_send(msg).map_err(|e| e.into())
    }

    fn connect(&self) {
        // Get the config from the views, and connect
        let cfg = self.view_control.get_config();
        log::info!("Connecting with {:?}", &cfg);

        let msg = AppMessage::Connect(cfg);
        self.send_app_message(msg)
            .expect("Failed to send connect message");
    }

    fn create_shared_context(
        ctx: ContextWrapper<NotCurrent, Window>,
    ) -> (
        ContextWrapper<NotCurrent, Window>,
        gst_gl::GLContext,
        gst_gl::GLDisplay,
    ) {
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

        (ctx, shared_context, gl_display)
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
        // Look for any upload{idx} element that might exist
        let n_views = self.view_control.get_n_views();
        let e = (0..n_views)
            .find_map(|i| self.pipeline.get_by_name(&format!("upload{}", i)))
            .expect("Failed to find upload element");

        // self.pipeline
        //     .get_by_name("upload")
        //     .expect("Failed to get upload element");

        e.get_property("context")
            .expect("No property 'context' found")
            .get::<gst_gl::GLContext>()
            .expect("Failed to cast to GLContext")
            .expect("Context is empty")
    }

    fn set_shared_context(
        &self,
        shared_context: gst_gl::GLContext,
        shared_display: gst_gl::GLDisplay,
    ) {
        // We set the context/display that should be wrapped by the gl-plugins.

        let display_context = gst::Context::new(*gst_gl::GL_DISPLAY_CONTEXT_TYPE, true);
        display_context.set_gl_display(&shared_display);
        self.pipeline.set_context(&display_context);

        let mut gl_context = gst::Context::new("gst.gl.app_context", true);
        {
            let context = gl_context.get_mut().unwrap();
            let s = context.get_mut_structure();
            s.set("context", &shared_context);
        }
        self.pipeline.set_context(&gl_context);
    }

    fn set_event_proxy(&self, proxy: EventLoopProxy<WindowMessage>) {
        let mut shared = self.shared.lock().unwrap();
        self.view_control.set_event_proxy(&proxy);
        shared.proxy = Some(proxy);
    }

    pub fn main_loop(self, config: AppConfig) -> Result<()> {
        log::debug!("Starting app main loop on current thread");

        // Arrange the views horizontally
        self.view_control.arrange_horizontal();
        let view_bounds = self.view_control.get_layout();

        dbg!(&view_bounds);

        let event_loop = EventLoop::<WindowMessage>::with_user_event();
        let size = config.viewport_size;
        let window_builder = WindowBuilder::new().with_inner_size(PhysicalSize {
            width: view_bounds.width,
            height: view_bounds.height,
        });

        let main_context = glutin::ContextBuilder::new()
            .with_gl(glutin::GlRequest::Specific(glutin::Api::OpenGl, (4, 5)))
            .with_gl_profile(glutin::GlProfile::Core)
            .with_vsync(false)
            .build_windowed(window_builder, &event_loop)
            .expect("Failed to build GL main context");

        // Create GStreamer context
        let (main_context, own_context, shared_display) = Self::create_shared_context(main_context);

        // Set the event loop proxy on App
        self.set_event_proxy(event_loop.create_proxy());
        self.set_shared_context(own_context.clone(), shared_display);

        // Start the pipeline
        self.pipeline
            .set_state(gst::State::Playing)
            .expect("Failed to set the pipeline to playing");

        // Connect to server
        self.connect();

        // We really need to ensure that connect() has been handled before we send another
        // ws-request, otherwise the server might error out.
        self.send_app_message(AppMessage::GetCases)
            .expect("Failed to send GetCases");

        // This is the context until we have the first sample, then we know
        // that context sharing is done and we can current the context.
        let mut tmp_ctx = Some(main_context);
        let mut main_context: Option<ContextWrapper<PossiblyCurrent, Window>> = None;
        let mut renderer: Option<GlRenderer> = None;
        let mut own_context: Option<gst_gl::GLContext> = Some(own_context);

        let mut request_response: Vec<Instant> = Vec::new();

        // Start a timer to get reliable callbacks on the event loop
        let interactions_per_second = 60;
        let request_timeout_ms = (1000_f32 / interactions_per_second as f32).floor() as u64;
        start_window_timer(
            event_loop.create_proxy(),
            Duration::from_millis(request_timeout_ms),
        );

        event_loop.run(move |event, _target, flow| {
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

                        let selected_case = if let Some(ref wanted) = config.case_key {
                            new_cases
                                .iter()
                                .find(|c| *c.key == *wanted)
                                .map(|c| c.clone())
                        } else {
                            new_cases.first().map(|c| c.clone())
                        };
                        if let Some(ref case) = selected_case {
                            println!("Selected case: {}", &case.key);
                            self.view_control
                                .set_case(case.key.clone(), case.number_of_images);
                        }
                    }
                    WindowMessage::Redraw(view_index) => {
                        println!("Got redraw idx {}", view_index);
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

                            // TODO: Remove
                            // gst::debug_bin_to_dot_file(
                            //     &self.pipeline,
                            //     gst::DebugGraphDetails::all(),
                            //     "wsclient",
                            // )
                            // END TODO
                        }

                        log::trace!("Main loop got a sample");

                        renderer.as_mut().map(|r| {
                            let views = self.view_control.get_active();
                            r.render_views(views);
                        });
                        main_context
                            .as_ref()
                            .map(|c| c.swap_buffers().expect("Failed to swap back-buffer"));

                        if request_response.len() > 0 {
                            log::trace!("Request len: {}", request_response.len());
                            let req = request_response.remove(0);
                            log::trace!("Gussing frame time is {:?}", req.elapsed());
                        }
                        let e = self
                            .pipeline
                            .get_by_name("rtpjitterbuffer0")
                            .expect("Failed to get jitterbuffer");
                        let stats = e.get_property("stats").expect("Failed to get stats");

                        let _s = stats
                            .get::<&StructureRef>()
                            .expect("Failed to get structure")
                            .expect("Structure is None");
                    }
                    WindowMessage::Timer(_) => {
                        self.view_control.push_state();
                    }
                    WindowMessage::PipelineError => {
                        log::error!("Got error from pipeline, exiting");
                        *flow = ControlFlow::Exit;
                    }
                },
                Event::WindowEvent { event, .. } => {
                    let handled = match event {
                        WindowEvent::CloseRequested => {
                            *flow = ControlFlow::Exit;
                            true
                        }
                        WindowEvent::KeyboardInput { input, .. }
                            if input.state == ElementState::Pressed =>
                        {
                            match input.virtual_keycode {
                                Some(VirtualKeyCode::S) => {
                                    println!("S is pressed, setting single view");
                                    self.view_control.set_active(&[0]);
                                    self.view_control.arrange_horizontal();
                                    true
                                }
                                Some(VirtualKeyCode::P) => {
                                    println!("P is pressed, setting two views.");
                                    self.view_control.set_active(&[0, 1]);
                                    self.view_control.arrange_horizontal();
                                    true
                                }
                                _ => false,
                            }
                        }
                        _ => false,
                    };

                    if !handled {
                        // Let the views handle the event
                        self.view_control.handle_window_event(&event);
                    }

                    // Check if we should hide the cursor.
                    if let Some(ref main_context) = main_context {
                        let window = main_context.window();
                        if self.view_control.hide_cursor() {
                            window.set_cursor_visible(false);
                        } else {
                            window.set_cursor_visible(true);
                        }
                    }
                    self.view_control.update();
                }
                Event::MainEventsCleared => {}
                Event::RedrawRequested(_) => {
                    // Nothing to do right now
                }
                _ => (),
            }
        });
    }
}
