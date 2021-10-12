use super::timed_iter::timer;

pub struct TimerMessage<T> {
    message: T,
    repeat: bool,
    duration: std::time::Duration,
    ticks: usize,
}

pub struct WindowTimer<T> {
    sender: std::sync::mpsc::Sender<TimerMessage<T>>,
    _handle: std::thread::JoinHandle<()>,
    poll_interval: std::time::Duration,
}

impl<T: Clone + Send + 'static> WindowTimer<T> {
    pub fn new<F: FnMut(T) + Send + 'static>(
        mut dispatch: F,
        poll_interval: std::time::Duration,
    ) -> Self {
        let (sender, recevier) = std::sync::mpsc::channel::<TimerMessage<T>>();
        let _handle = std::thread::spawn(move || {
            let mut active_timers = Vec::new();

            for _ in timer(poll_interval) {
                // Get all new timers
                recevier
                    .try_iter()
                    .for_each(|timer| active_timers.push(timer));

                // Reduce the `ticks` for each active timer.
                for timer in &mut active_timers {
                    timer.ticks -= 1;

                    if timer.ticks == 0 {
                        // Expired timer
                        // send message
                        if timer.repeat {
                            // Reset the tick count
                            timer.ticks = Self::duration_to_polls(timer.duration, poll_interval);
                        }
                        // Send the message
                        dispatch(timer.message.clone());
                    }
                }
                // remove all expired timers.
                active_timers = active_timers
                    .into_iter()
                    .filter(|timer| timer.ticks > 0)
                    .collect();
            }
        });

        Self {
            sender,
            _handle,
            poll_interval,
        }
    }

    fn duration_to_polls(
        duration: std::time::Duration,
        poll_interval: std::time::Duration,
    ) -> usize {
        (duration.as_secs_f64() / poll_interval.as_secs_f64()).ceil() as usize
    }

    pub fn once(&self, message: T, duration: std::time::Duration) {
        let timer = TimerMessage {
            message,
            duration,
            ticks: Self::duration_to_polls(duration, self.poll_interval),
            repeat: false,
        };
        self.sender
            .send(timer)
            .expect("Failed to send new timer message");
    }

    pub fn repeat(&self, message: T, duration: std::time::Duration) {
        let timer = TimerMessage {
            message,
            duration,
            ticks: Self::duration_to_polls(duration, self.poll_interval),
            repeat: true,
        };
        self.sender
            .send(timer)
            .expect("Failed to send new timer message");
    }
}
