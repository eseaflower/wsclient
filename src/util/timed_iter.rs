use std::{
    iter::Repeat,
    time::{Duration, Instant},
};

pub struct TimedIter<I> {
    time_since_last: Instant,
    timeout: Duration,
    inner: I,
}

impl<I> Iterator for TimedIter<I>
where
    I: Iterator,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        let loop_time = self.time_since_last.elapsed();
        let sleep_time = (self.timeout.as_secs_f32() - loop_time.as_secs_f32()).max(0.0);
        let sleep_duration = Duration::from_secs_f32(sleep_time);
        spin_sleep::sleep(sleep_duration);
        self.time_since_last = Instant::now();
        self.inner.next()
    }
}
pub trait TimedIterExt {
    type IterType;
    fn timed(self, timeout: Duration) -> TimedIter<Self::IterType>;
}

impl<I> TimedIterExt for I
where
    I: Iterator,
{
    type IterType = I;
    fn timed(self, timeout: Duration) -> TimedIter<Self::IterType> {
        TimedIter {
            time_since_last: Instant::now(),
            inner: self,
            timeout,
        }
    }
}

pub fn timer(timeout: Duration) -> TimedIter<Repeat<()>> {
    std::iter::repeat(()).timed(timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_wrap() {
        let range = vec![1, 2, 3];
        let timed = TimedIter {
            time_since_last: Instant::now(),
            inner: range.iter(),
            timeout: Duration::from_millis(1),
        };

        let res: Vec<_> = timed.collect();
        assert!(res.len() == 3);
        assert!(*res[0] == range[0]);
        assert!(*res[1] == range[1]);
        assert!(*res[2] == range[2]);
    }

    #[test]
    fn test_can_consume() {
        let range = vec![1, 2, 3];
        let timed = range.iter().timed(Duration::from_millis(1));
        let res: Vec<_> = timed.collect();
        assert!(res.len() == 3);
        assert!(*res[0] == range[0]);
        assert!(*res[1] == range[1]);
        assert!(*res[2] == range[2]);
    }
    #[test]
    fn test_infinite() {
        let _r = std::iter::repeat(())
            .take(3)
            .timed(Duration::from_millis(1));
    }
    #[test]
    fn test_timeout() {
        let r = timer(Duration::from_millis(50)).take(3);
        let mut timer = Instant::now();
        for _ in r {
            log::trace!("Elapsed: {}", timer.elapsed().as_secs_f32());
            timer = Instant::now();
        }
    }
}
