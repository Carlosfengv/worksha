//! Bounded FIFO admission for the connection-private historical query index.
use std::{
    collections::BTreeSet,
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

const MAX_WAITERS: u64 = 32;
const BUSY: &str = "历史查询正在更新，请稍后刷新";
const UNAVAILABLE: &str = "历史查询索引暂不可用";

#[derive(Default)]
struct State {
    next: u64,
    serving: u64,
    cancelled: BTreeSet<u64>,
}
#[derive(Default)]
pub(super) struct Queue {
    state: Mutex<State>,
    changed: Condvar,
}
pub(super) struct Permit<'a> {
    queue: &'a Queue,
    deadline: Instant,
}

impl State {
    fn advance(&mut self) {
        self.serving += 1;
        while self.cancelled.remove(&self.serving) {
            self.serving += 1;
        }
    }
}
impl Queue {
    #[cfg(test)]
    pub(super) fn issued(&self) -> u64 {
        self.state.lock().unwrap().next
    }
    pub(super) fn enter(&self, budget: Duration) -> Result<Permit<'_>, String> {
        let deadline = Instant::now() + budget;
        let mut state = self.state.lock().map_err(|_| UNAVAILABLE)?;
        if state.next - state.serving > MAX_WAITERS {
            return Err(BUSY.into());
        }
        let ticket = state.next;
        state.next = state.next.checked_add(1).ok_or(BUSY)?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                if ticket == state.serving {
                    state.advance();
                } else {
                    state.cancelled.insert(ticket);
                }
                self.changed.notify_all();
                return Err(BUSY.into());
            }
            if ticket == state.serving {
                return Ok(Permit {
                    queue: self,
                    deadline,
                });
            }
            state = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| UNAVAILABLE)?
                .0;
        }
    }
}
impl Permit<'_> {
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }
}
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self.queue.state.lock().unwrap_or_else(|e| e.into_inner());
        state.advance();
        self.queue.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn admission_is_fifo_and_timed_out_tickets_do_not_block_followers() {
        let queue = Arc::new(Queue::default());
        let first = queue.enter(Duration::from_secs(2)).unwrap();
        let expired = std::thread::spawn({
            let queue = queue.clone();
            move || queue.enter(Duration::from_millis(10)).is_err()
        });
        assert!(expired.join().unwrap());
        let order = Arc::new(Mutex::new(vec![]));
        let mut workers = vec![];
        for n in 0..4 {
            workers.push(std::thread::spawn({
                let queue = queue.clone();
                let order = order.clone();
                move || {
                    let _permit = queue.enter(Duration::from_secs(2)).unwrap();
                    order.lock().unwrap().push(n);
                }
            }));
            let deadline = Instant::now() + Duration::from_secs(1);
            while queue.state.lock().unwrap().next < n + 3 {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        drop(first);
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(*order.lock().unwrap(), [0, 1, 2, 3]);
        assert!(queue.state.lock().unwrap().cancelled.is_empty());
    }
    #[test]
    fn overload_is_rejected_and_dropping_the_owner_releases_all_slots() {
        let queue = Arc::new(Queue::default());
        let first = queue.enter(Duration::from_secs(5)).unwrap();
        let mut workers = vec![];
        for _ in 0..MAX_WAITERS {
            workers.push(std::thread::spawn({
                let queue = queue.clone();
                move || {
                    let _permit = queue.enter(Duration::from_secs(3)).unwrap();
                }
            }));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while queue.state.lock().unwrap().next <= MAX_WAITERS {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(queue.enter(Duration::from_secs(1)).is_err());
        drop(first);
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(queue.enter(Duration::from_secs(1)).is_ok());
    }
}
