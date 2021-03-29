use glutin::event_loop::EventLoopProxy;

use crate::window_message::WindowMessage;

use super::timed_iter::timer;

pub fn start_window_timer(proxy: EventLoopProxy<WindowMessage>, interval: std::time::Duration) {
    std::thread::spawn(move || {
        for _ in timer(interval) {
            proxy
                .send_event(WindowMessage::Timer(interval))
                .expect("Failed to send window timer message");
        }
    });
}
