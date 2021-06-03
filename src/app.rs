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
    collections::HashMap,
    ops::Deref,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use window_message::{ViewSample, WindowMessage};

use crate::message::{AppMessage, ClientConfig, LayoutRect};
use crate::window_message;
use crate::{
    glvideo::GlRenderer,
    util::{element_timer::ElementTimer, window_timer::WindowTimer},
    view::ViewControl,
    AppConfig,
};

// use super::view::ViewCollection;

#[derive(Debug)]
struct SharedState {
    proxy: Option<EventLoopProxy<WindowMessage>>,
    timers: Vec<ElementTimer>,
    samples: HashMap<usize, Option<ViewSample>>,
}

#[derive(Debug, Clone)]
pub struct App(pub Arc<AppInner>);
#[derive(Debug)]
pub struct AppInner {
    signaller: UnboundedSender<AppMessage>,
    webrtcbin: gst::Element,
    pipeline: gst::Pipeline,
    shared: Mutex<SharedState>,
    tcp: bool,
    decoder: Decoder,
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

#[derive(Debug, Copy, Clone)]
pub enum Decoder {
    Software,
    Hardware,
    FastSoftware,
}

impl App {
    pub fn new(
        signaller: UnboundedSender<AppMessage>,
        tcp: bool,
        decoder: Decoder,
        jitter: u32,
    ) -> Self {
        let pipeline = gst::Pipeline::new(None);
        let webrtcbin = gst::ElementFactory::make("webrtcbin", Some("webrtcbin"))
            .expect("Failed to create webrtcbin");
        pipeline
            .add(&webrtcbin)
            .expect("Failed to add element to pipeline");
        webrtcbin.set_property_from_str("stun-server", "stun://stun.l.google.com:19302");
        webrtcbin.set_property_from_str("bundle-policy", "max-bundle");

        if tcp {
            log::debug!("Disabling UDP, using TCP");

            let agent = webrtcbin
                .get_property("ice-agent")
                .expect("Failed to get ice agent")
                .get::<gst::Object>()
                .expect("Failed to get object")
                .expect("Object is empty");
            agent
                .set_property("ice-udp", &false)
                .expect("Failed to disable UDP");

        // let rtpbin = pipeline
        //     .get_by_name("rtpbin")
        //     .expect("Failed to get rtpbin");

        // // rtpbin
        // //     .set_property("latency", &500_u32)
        // //     .expect("Failed to set latency");

        // println!("Setting 'synced' rtpjitterbuffer mode");
        // rtpbin.set_property_from_str("buffer-mode", "synced");
        } else {
            // For UDP transfer we need some mechanisms to handle missing packets
            log::debug!("Allowing UDP, adding NACKs");
            webrtcbin
                .connect("on-new-transceiver", false, move |vals| {
                    log::debug!("Got transceiver callback, setting nack");
                    let t = vals[1]
                        .get::<gst_webrtc::WebRTCRTPTransceiver>()
                        .expect("Not the type")
                        .expect("Trans is None");
                    t.set_property("do-nack", &true)
                        .expect("Failed to set nack");
                    None
                })
                .expect("Failed to attach handler");

            // let rtpbin = pipeline
            //     .get_by_name("rtpbin")
            //     .expect("Failed to get rtpbin");

            // TODO: Handle configuring the latency in the buffer
            // this helps A LOT for the problem of corrrupt frames!
            // A too small buffer leads to that frames might be
            // dropped while waiting for retransmission etc.
            // this is likely what causes the large artifacts.

            // rtpbin
            //     .set_property("latency", &50_u32)
            //     .expect("Failed to set latency");

            // rtpbin
            //     .set_property("do-retransmission", &true)
            //     .expect("Failed to set retransmission");
            // rtpbin
            //     .set_property("drop-on-latency", &true)
            //     .expect("Failed to set drop on latency");
            // rtpbin
            //     .set_property("do-lost", &true)
            //     .expect("Failed to set do-lost");
        }

        let rtpbin = pipeline
            .get_by_name("rtpbin")
            .expect("Failed to get rtpbin");

        log::debug!("Setting 'synced' rtpjitterbuffer mode");
        rtpbin.set_property_from_str("buffer-mode", "synced");
        log::debug!("Setting jitter buffer latency to {}", jitter);
        rtpbin.set_property("property_name", &jitter);

        // END TODO

        let inner = AppInner {
            signaller,
            pipeline,
            webrtcbin,
            shared: Mutex::new(SharedState {
                proxy: None,
                timers: Vec::default(),
                samples: HashMap::default(),
            }),
            tcp,
            decoder,
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
                    let shared = app.shared.lock().unwrap();
                    shared.proxy.as_ref().map(|proxy| {
                        proxy
                            .send_event(WindowMessage::Datachannel(datachannel))
                            .expect("Failed to send datachannel")
                    });
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
            log::debug!("Pad removed...");
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

        log::debug!("Linking new stream with id {}", mlineidx);
        // let pipeline_description = "identity name=ident ! application/x-rtp, media=(string)video, clock-rate=(int)90000, encoding-name=(string)H264, payload=(int)96
        //  ! rtph264depay name=depay ! h264parse ! avdec_h264 ! videoconvert ! videoscale ! d3d11upload ! d3d11videosink";

        // let pipeline_description = "rtph264depay name=depay ! h264parse ! avdec_h264 ! d3d11upload ! d3d11convert ! d3d11videosink sync=false";
        let decoder_template = match self.decoder {
            Decoder::Software => "openh264dec",
            Decoder::Hardware => "nvh264dec",
            Decoder::FastSoftware => "avdec_h264",
        };

        let pipeline_template =
            "rtph264depay name=depay{idx} ! h264parse name=parse{idx} config-interval=-1 ! {decoder_tpl} name=decoder{idx} qos=true ! queue ! glupload name=upload{idx} ! glcolorconvert name=convert{idx} ! appsink name=appsink{idx}";
        // Get the selected decoder
        let pipeline_template = pipeline_template.replace("{decoder_tpl}", decoder_template);
        let pipeline_description = pipeline_template.replace("{idx}", &mlineidx.to_string());
        println!("Using decoder bin: {}", &pipeline_template);

        let decodebin = gst::parse_bin_from_description(&pipeline_description, true)
            .expect("Failed to parse decodebin");

        let appsink_name = format!("appsink{}", mlineidx);
        let appsink = decodebin
            .get_by_name(&appsink_name)
            .expect("Failed to get appsink");
        let appsink = appsink
            .downcast::<gst_app::AppSink>()
            .expect("Failed to cast to appsink");

        let weak_app = Arc::downgrade(&self.0);
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    if let Some(app) = weak_app.upgrade().map(App) {
                        let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let mut shared = app.shared.lock().unwrap();
                        // Set the sample in the slot for the mlineidx.
                        shared.samples.insert(
                            mlineidx as usize,
                            Some(ViewSample {
                                sample,
                                id: mlineidx as _,
                                timer: std::time::Instant::now(),
                            }),
                        );

                        shared.proxy.as_ref().map(|proxy| {
                            proxy
                                .send_event(WindowMessage::Sample(mlineidx as usize))
                                .expect("Failed to send sample")
                        });
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
            .get_by_name(&format!("decoder{}", mlineidx))
            .expect("Failed to get decoder");
        let convert = decodebin
            .get_by_name(&format!("convert{}", mlineidx))
            .expect("Failed to get appsink");

        if log::log_enabled!(log::Level::Trace) {
            let timer = ElementTimer::new(&format!("decoder-convert{}", mlineidx), depay, convert);
            {
                let mut shared = self.shared.lock().unwrap();
                shared.timers.push(timer);
            }
        }
        Ok(())
    }

    pub fn get_sample(&self, index: usize) -> Option<ViewSample> {
        let mut shared = self.shared.lock().unwrap();
        if let Some(sample) = shared.samples.get_mut(&index) {
            // Take the last sample and move it out
            sample.take()
        } else {
            None
        }
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

                // dbg!(&msg);

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

    fn connect(&self, cfg: Vec<ClientConfig>) {
        // Get the config from the views, and connect
        // let cfg = self.view_control.get_config();
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

    fn get_pipe_context(&self, idx: usize) -> gst_gl::GLContext {
        let e = self
            .pipeline
            .get_by_name(&format!("upload{}", idx))
            .expect("Failed to get upload element");

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
        shared.proxy = Some(proxy);
    }

    fn finalize_contexts(
        ctx: ContextWrapper<NotCurrent, Window>,
        own_context: gst_gl::GLContext,
        pipe_context: gst_gl::GLContext,
    ) -> (ContextWrapper<PossiblyCurrent, Window>, GlRenderer) {
        // Current the context
        let main_context = unsafe { ctx.make_current().expect("Failed to current context") };
        log::debug!(
            "Using window with settings: {:?}",
            main_context.get_pixel_format()
        );

        log::debug!("Main context has been currented");
        let mut renderer = GlRenderer::new(
            |name| main_context.get_proc_address(name),
            own_context,
            pipe_context,
        );
        // Get the size of the window
        let inner_size = main_context.window().inner_size();
        renderer.set_window_size((inner_size.width, inner_size.height));
        (main_context, renderer)
    }

    pub fn main_loop(self, config: AppConfig) -> Result<()> {
        log::debug!("Starting app main loop on current thread");

        let mut view_control = ViewControl::new(1, &config);
        view_control.partition(1, 1);

        let window_size = (config.viewport_size.0, config.viewport_size.1);
        let event_loop = EventLoop::<WindowMessage>::with_user_event();
        let window_builder = WindowBuilder::new().with_inner_size(PhysicalSize {
            width: window_size.0,
            height: window_size.1,
        });

        // Set the size of the view control to match the window.
        view_control.set_layout(LayoutRect {
            x: 0,
            y: 0,
            width: window_size.0,
            height: window_size.1,
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
        self.connect(view_control.get_config());

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

        // Start a timer to get reliable callbacks on the event loop
        let interactions_per_second = 61;
        let request_timeout_ms = (1000_f32 / interactions_per_second as f32).floor() as u64;
        let timer = WindowTimer::new(
            event_loop.create_proxy(),
            Duration::from_millis(request_timeout_ms),
        );

        // Start a repeat timer that fires with the request timeout
        let duration = Duration::from_millis(1);
        timer.repeat(WindowMessage::Timer(duration), duration);
        // Start a timer that traces JitterBuffer statistics
        timer.repeat(WindowMessage::JitterStats, Duration::from_millis(1000));

        let mut layout_pending = false;

        event_loop.run(move |event, _target, flow| {
            // The actual rendering seems not dependant on this loop.
            // So we can wait for new events.
            *flow = ControlFlow::Wait;

            // Try to get the video overlay and move it into a normal reference
            match event {
                Event::UserEvent(wm) => match wm {
                    WindowMessage::Cases((protocols, cases)) => {
                        view_control.set_case_meta(protocols, cases);

                        println!("Known cases:\n{}", view_control.get_case_string());
                        println!("Known protocols:\n{}", view_control.get_protocol_string());

                        view_control.select_default_display();
                    }
                    WindowMessage::Datachannel(datachannel) => {
                        view_control.set_datachannel(datachannel);
                    }
                    WindowMessage::Sample(index) => {
                        // When we get the first sample we can current our context
                        // and build the renderer, since now context-sharing should
                        // be set up.
                        if let Some(ctx) = tmp_ctx.take() {
                            // Move the tmp_ctx into the main context after setting it current
                            let (context, gl_rend) = Self::finalize_contexts(
                                ctx,
                                own_context.take().expect("Context is empty"),
                                self.get_pipe_context(index),
                            );
                            // Assign the instances that we will use through out.
                            main_context = Some(context);
                            renderer = Some(gl_rend);
                        }

                        log::trace!("Main loop got a sample");

                        // Get the latest sample for view 'index'
                        self.get_sample(index)
                            .map(|sample| view_control.push_sample(sample));

                        // Request a redraw
                        main_context.as_ref().map(|c| {
                            c.window().request_redraw();
                        });
                    }
                    WindowMessage::Timer(_) => {
                        // Let the control react to timer events.
                        view_control.handle_timer_event();

                        view_control.push_state();
                    }
                    WindowMessage::UpdateLayout => {
                        layout_pending = false;
                        main_context.as_ref().map(|c| {
                            let size = c.window().inner_size();
                            // Update the layout to fill the entire window.
                            view_control.set_layout(LayoutRect {
                                x: 0,
                                y: 0,
                                width: size.width,
                                height: size.height,
                            });
                        });
                    }
                    WindowMessage::PipelineError => {
                        log::error!("Got error from pipeline, exiting");
                        *flow = ControlFlow::Exit;
                    }
                    WindowMessage::JitterStats => {
                        self.pipeline.get_by_name("rtpjitterbuffer0").map(|e| {
                            let gst_stats = e.get_property("stats").expect("Failed to get stats");
                            let stats = gst_stats
                                .get::<&gst::StructureRef>()
                                .expect("Failed to cast to StructureRef")
                                .expect("StructureRef is empty");

                            let jitter_stats = to_jitter_stats(stats);
                            log::trace!("{:?}", jitter_stats);
                        });
                    }
                },
                Event::WindowEvent { event, .. } => {
                    let handled = match event {
                        WindowEvent::CloseRequested => {
                            *flow = ControlFlow::Exit;
                            true
                        }
                        WindowEvent::Resized(size) => {
                            // Also update the renderer with the new window size
                            renderer
                                .as_mut()
                                .map(|r| r.set_window_size((size.width, size.height)));

                            view_control.set_window_size((size.width, size.height));
                            if !layout_pending {
                                layout_pending = true;
                                timer.once(WindowMessage::UpdateLayout, Duration::from_millis(500));
                            }

                            // Make sure the GL-surface is resized
                            main_context.as_ref().map(|c| {
                                c.resize(size);
                                c.window().request_redraw();
                            });
                            true
                        }
                        _ => false,
                    };

                    if !handled {
                        // Let the views handle the event
                        view_control.handle_window_event(&event);
                    }

                    // Check if we should hide the cursor.
                    if let Some(ref main_context) = main_context {
                        let window = main_context.window();
                        if view_control.hide_cursor() {
                            window.set_cursor_visible(false);
                        } else {
                            window.set_cursor_visible(true);
                        }
                    }
                    view_control.update_focused();
                }
                Event::MainEventsCleared => {}
                Event::RedrawRequested(_) => {
                    // Render the views
                    renderer.as_mut().map(|r| {
                        r.render_views(&view_control);
                    });
                    // Swap back buffer
                    main_context
                        .as_ref()
                        .map(|c| c.swap_buffers().expect("Failed to swap back-buffer"));
                }
                _ => (),
            }
        });
    }
}
#[derive(Debug)]
struct JitterStats {
    num_pushed: u64,
    num_lost: u64,
    num_late: u64,
    num_duplicates: u64,
    avg_jitter: u64,
    rtx_count: u64,
    rtx_success_count: u64,
    rtx_per_packet: f64,
    rtx_rtt: u64,
}

fn to_jitter_stats(stats: &gst::StructureRef) -> JitterStats {
    JitterStats {
        num_pushed: stats.get::<u64>("num-pushed").unwrap().unwrap(),
        num_lost: stats.get::<u64>("num-lost").unwrap().unwrap(),
        num_late: stats.get::<u64>("num-late").unwrap().unwrap(),
        num_duplicates: stats.get::<u64>("num-duplicates").unwrap().unwrap(),
        avg_jitter: stats.get::<u64>("avg-jitter").unwrap().unwrap(),
        rtx_success_count: stats.get::<u64>("rtx-success-count").unwrap().unwrap(),
        rtx_count: stats.get::<u64>("rtx-count").unwrap().unwrap(),
        rtx_per_packet: stats.get::<f64>("rtx-per-packet").unwrap().unwrap(),
        rtx_rtt: stats.get::<u64>("rtx-rtt").unwrap().unwrap(),
    }
}
