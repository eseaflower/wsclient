use std::{
    convert::TryInto,
    sync::{Arc, Weak},
};

use anyhow::Result;
use app::{App, AppInner};
use async_std::task::JoinHandle;
use async_tungstenite::{async_std::connect_async, tungstenite::Message};
use futures::{
    channel::mpsc::{unbounded, UnboundedReceiver},
    future, Sink, SinkExt, Stream, StreamExt, TryStreamExt,
};
use message::AppMessage;

mod app;
mod message;
mod view_state;
mod window_message;
mod interaction;
mod bindings;
mod glvideo;
mod vertex;

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
            dbg!(response);
            let (outgoing, incomming) = ws.split();

            let send_handle = start_sender(outgoing.sink_map_err(|e| e.into()), rcv);
            let receive_handle = start_receiver(incomming.map_err(|e| e.into()), weak_app);

            // Let this task run until either the server closses the connection or the signal sender (snd) is dropped.
            // The signal sender will be dropped when the App is dropped, which means that the sender task will complete.
            let (_, _, to_cancel) = future::select_all(vec![send_handle, receive_handle]).await;
            // Make sure all remaining tasks are canceled
            future::join_all(to_cancel.into_iter().map(|x| x.cancel())).await;

            println!("Main task is complete");
        });
    })
}

pub fn run(ws_url: String) -> Result<()> {
    // Init GStreamer
    gstreamer::init().expect("Failed to initialize GStreamer");

    let (snd, rcv) = unbounded::<AppMessage>();
    let app = App::new(snd);

    let signal_thread = run_signalling(ws_url, Arc::downgrade(&app.0), rcv);

    app.main_loop();
    // Wait for the signal thread to complete (it exits when the app is dropped)
    let _ = signal_thread.join();
    println!("All done");

    Ok(())
}
