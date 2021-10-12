use std::{
    convert::TryInto,
    sync::{Arc, Weak},
};

use anyhow::Result;
use app::{App, AppInner, Decoder};
use async_std::task::JoinHandle;
use async_tungstenite::{async_std::connect_async, tungstenite::Message};
use futures::{
    channel::mpsc::{unbounded, UnboundedReceiver},
    future, Sink, SinkExt, Stream, StreamExt, TryStreamExt,
};
use glutin::{
    dpi::PhysicalSize,
    event::{ElementState, Event, MouseButton, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use message::AppMessage;
use util::bitrate::Schedule;
use view::ViewControl;

use crate::window_message::WindowMessage;

mod app;
mod bindings;
mod glvideo;
mod interaction;
mod message;
mod text_renderer;
mod util;
mod vertex;
mod view;
mod view_state;
mod window_message;

#[derive(Debug)]
pub struct AppConfig {
    viewport_size: (u32, u32),
    case_key: Option<String>,
    protocol_key: Option<String>,
    ws_url: String,
    bitrate_scale: f32,
    gpu: bool,
    preset: String,
    lossless: bool,
    video_scaling: f32,
    narrow: bool,
    tcp: bool,
    decoder: Decoder,
    jitter: u32,
    n_views: usize,
    schedule: Schedule,
}
impl AppConfig {
    pub fn new(
        ws_url: String,
        viewport_size: (u32, u32),
        case_key: Option<String>,
        protocol_key: Option<String>,
        bitrate_scale: f32,
        gpu: bool,
        preset: String,
        lossless: bool,
        video_scaling: f32,
        narrow: bool,
        tcp: bool,
        client_hw: bool,
        fast_sw_decode: bool,
        jitter: u32,
        n_views: usize,
        scedule_string: String,
    ) -> Self {
        let decoder = if fast_sw_decode {
            Decoder::FastSoftware
        } else if client_hw {
            Decoder::Hardware
        } else {
            Decoder::Software
        };
        let schedule = match &scedule_string[..] {
            "performance" => Schedule::Performance,
            "quality" => Schedule::Quality,
            _ => Schedule::Default,
        };
        Self {
            ws_url,
            viewport_size,
            case_key,
            protocol_key,
            bitrate_scale,
            gpu,
            preset,
            lossless,
            video_scaling,
            narrow,
            tcp,
            decoder,
            jitter,
            n_views,
            schedule,
        }
    }
}

fn start_sender<S>(sink: S, rcv: UnboundedReceiver<AppMessage>) -> JoinHandle<()>
where
    S: Sink<Message, Error = anyhow::Error> + Send + 'static,
{
    let handle = async_std::task::spawn(async move {
        let _ = rcv.map(|m| m.try_into()).forward(sink).await;
        log::info!("Exiting sender task");
    });
    handle
}

fn start_receiver<S>(stream: S, weak_app: Weak<AppInner>) -> JoinHandle<()>
where
    S: Stream<Item = Result<Message>> + Send + 'static,
{
    let handle = async_std::task::spawn(async move {
        let _ = stream
            .try_for_each(|msg| async {
                if let Ok(msg) = msg.try_into() {
                    if let Some(app) = weak_app.upgrade().map(App) {
                        if let Err(e) = app.handle_app_message(msg) {
                            log::error!("Failed to handle app message: {:?}", e);
                        }
                    } else {
                        log::error!("Failed to upgrade weak reference");
                    }
                } else {
                    log::error!("Failed to deserialize AppMessage");
                }
                Ok(())
            })
            .await;
        log::info!("Exiting receiver task");
    });
    handle
}

fn run_signalling(
    url: String,
    weak_app: Weak<AppInner>,
    rcv: UnboundedReceiver<AppMessage>,
) -> std::thread::JoinHandle<()> {
    // Start a new thread that runs the async tasks used for web socket communication.

    std::thread::spawn(|| {
        async_std::task::block_on(async move {
            let (ws, response) = connect_async(url)
                .await
                .expect("Failed to connect to server");

            log::debug!("Got respose from websocker server: {:?}", response);
            let (outgoing, incomming) = ws.split();

            let send_handle = start_sender(outgoing.sink_map_err(|e| e.into()), rcv);
            let receive_handle = start_receiver(incomming.map_err(|e| e.into()), weak_app);

            // Let this task run until either the server closses the connection or the signal sender (snd) is dropped.
            // The signal sender will be dropped when the App is dropped, which means that the sender task will complete.
            let (_, _, to_cancel) = future::select_all(vec![send_handle, receive_handle]).await;
            // Make sure all remaining tasks are canceled
            future::join_all(to_cancel.into_iter().map(|x| x.cancel())).await;

            log::debug!("Main task is complete");
        });
    })
}

pub fn run(config: AppConfig) -> Result<()> {
    // Init GStreamer
    gstreamer::init().expect("Failed to initialize GStreamer");

    // Create the views that we want connected.
    let (snd, rcv) = unbounded::<AppMessage>();
    let app = App::new(snd, config.tcp, config.decoder, config.jitter);

    let signal_thread = run_signalling(config.ws_url.clone(), Arc::downgrade(&app.0), rcv);

    // Build the window and gl-context
    let event_loop = EventLoop::<WindowMessage>::with_user_event();
    let window_builder = WindowBuilder::new().with_inner_size(PhysicalSize {
        width: config.viewport_size.0,
        height: config.viewport_size.1,
    });
    let main_context = glutin::ContextBuilder::new()
        .with_gl(glutin::GlRequest::Specific(glutin::Api::OpenGl, (4, 5)))
        .with_gl_profile(glutin::GlProfile::Core)
        .with_vsync(false)
        .build_windowed(window_builder, &event_loop)
        .expect("Failed to build GL main context");
    let (main_context, window) = unsafe { main_context.split() };
    let (snd, message_thread) = app.start(config, main_context);

    event_loop.run(move |event, _target, flow| {
        *flow = ControlFlow::Wait;
        match &event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::MouseInput { state, button, .. } if *button == MouseButton::Left => {
                    match *state {
                        ElementState::Pressed => window.set_cursor_visible(false),
                        ElementState::Released => window.set_cursor_visible(true),
                    }
                }
                WindowEvent::CloseRequested => {
                    *flow = ControlFlow::Exit;
                }
                _ => {}
            },
            _ => {}
        };
        // Convert messages with static lifetimes, ignore the one (ScaleFactorChanged) that cant be converted.
        event.to_static().map(|event| snd.send(event));
    });

    // Wait for the signal thread to complete (it exits when the app is dropped)
    let _ = signal_thread.join();
    let _ = message_thread.join();
    log::debug!("All done");

    Ok(())
}
