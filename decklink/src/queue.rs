use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use futures_core::Stream;

use crate::error::{Error, ErrorKind};

/// What to do when a hardware callback finds a full queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverflowPolicy {
    /// Live preview default: keep the newest sample.
    DropOldest,
    /// Keep already queued samples and discard the incoming one.
    DropNewest,
    /// Close the stream so the session can stop cleanly.
    ErrorAndStop,
}

/// Snapshot of overflow activity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverflowInfo {
    pub dropped: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub high_water_mark: usize,
}

#[derive(Debug)]
struct Inner<T> {
    items: VecDeque<T>,
    overflow: OverflowInfo,
    next_sequence: u64,
}

/// Bounded event queue shared by DeckLink callbacks and async consumers.
#[derive(Debug)]
pub struct EventQueue<T> {
    inner: Mutex<Inner<T>>,
    capacity: usize,
    policy: OverflowPolicy,
    waker: Mutex<Option<Waker>>,
    closed: AtomicBool,
}

impl<T> EventQueue<T> {
    pub fn new(capacity: usize, policy: OverflowPolicy) -> Arc<Self> {
        assert!(capacity > 0, "queue capacity must be at least 1");
        Arc::new(Self {
            inner: Mutex::new(Inner {
                items: VecDeque::with_capacity(capacity),
                overflow: OverflowInfo::default(),
                next_sequence: 0,
            }),
            capacity,
            policy,
            waker: Mutex::new(None),
            closed: AtomicBool::new(false),
        })
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.wake();
    }

    fn wake(&self) {
        if let Some(waker) = self.waker.lock().expect("waker mutex").take() {
            waker.wake();
        }
    }

    fn register(&self, waker: &Waker) {
        *self.waker.lock().expect("waker mutex") = Some(waker.clone());
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn overflow_info(&self) -> OverflowInfo {
        self.inner.lock().expect("queue mutex").overflow
    }

    pub fn try_push(&self, item: T) -> PushResult<T> {
        if self.is_closed() {
            return PushResult::Closed(item);
        }
        let mut inner = self.inner.lock().expect("queue mutex");
        let sequence = inner.next_sequence;
        inner.next_sequence += 1;
        if inner.items.len() < self.capacity {
            inner.items.push_back(item);
            inner.overflow.high_water_mark = inner.overflow.high_water_mark.max(inner.items.len());
            drop(inner);
            self.wake();
            return PushResult::Accepted { sequence };
        }

        match self.policy {
            OverflowPolicy::DropOldest => {
                let _ = inner.items.pop_front();
                inner.items.push_back(item);
                record_overflow(&mut inner.overflow, sequence);
                inner.overflow.high_water_mark = inner.overflow.high_water_mark.max(inner.items.len());
                drop(inner);
                self.wake();
                PushResult::DroppedOldest { sequence }
            }
            OverflowPolicy::DropNewest => {
                record_overflow(&mut inner.overflow, sequence);
                PushResult::DroppedNewest { sequence }
            }
            OverflowPolicy::ErrorAndStop => {
                record_overflow(&mut inner.overflow, sequence);
                drop(inner);
                self.close();
                PushResult::Stopped { sequence }
            }
        }
    }

    fn poll_next_inner(&self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        {
            let mut inner = self.inner.lock().expect("queue mutex");
            if let Some(item) = inner.items.pop_front() {
                return Poll::Ready(Some(item));
            }
        }
        if self.is_closed() {
            return Poll::Ready(None);
        }
        self.register(cx.waker());
        let mut inner = self.inner.lock().expect("queue mutex");
        if let Some(item) = inner.items.pop_front() {
            return Poll::Ready(Some(item));
        }
        if self.is_closed() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

fn record_overflow(info: &mut OverflowInfo, sequence: u64) {
    if info.dropped == 0 {
        info.first_sequence = sequence;
    }
    info.last_sequence = sequence;
    info.dropped += 1;
}

/// Result of a non-blocking enqueue.
#[derive(Debug)]
pub enum PushResult<T> {
    Accepted { sequence: u64 },
    DroppedOldest { sequence: u64 },
    DroppedNewest { sequence: u64 },
    Stopped { sequence: u64 },
    Closed(T),
}

impl<T> PushResult<T> {
    pub fn overflowed(&self) -> bool {
        matches!(
            self,
            Self::DroppedOldest { .. } | Self::DroppedNewest { .. } | Self::Stopped { .. }
        )
    }
}

/// Stream wrapper around [`EventQueue`].
pub struct EventStream<T> {
    queue: Arc<EventQueue<T>>,
}

impl<T> EventStream<T> {
    pub fn new(queue: Arc<EventQueue<T>>) -> Self {
        Self { queue }
    }

    pub fn queue(&self) -> &Arc<EventQueue<T>> {
        &self.queue
    }
}

impl<T> Stream for EventStream<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.queue.poll_next_inner(cx)
    }
}

impl From<OverflowInfo> for Error {
    fn from(info: OverflowInfo) -> Self {
        Error::new(
            ErrorKind::QueueOverflow,
            "queue",
            format!(
                "dropped {} samples (seq {}-{})",
                info.dropped, info.first_sequence, info.last_sequence
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_oldest_keeps_latest() {
        let queue = EventQueue::new(2, OverflowPolicy::DropOldest);
        assert!(matches!(queue.try_push(1), PushResult::Accepted { .. }));
        assert!(matches!(queue.try_push(2), PushResult::Accepted { .. }));
        assert!(matches!(queue.try_push(3), PushResult::DroppedOldest { .. }));
        let mut inner = queue.inner.lock().unwrap();
        assert_eq!(Vec::from_iter(inner.items.drain(..)), vec![2, 3]);
        assert_eq!(inner.overflow.dropped, 1);
    }

    #[test]
    fn error_and_stop_closes_queue() {
        let queue = EventQueue::new(1, OverflowPolicy::ErrorAndStop);
        assert!(matches!(queue.try_push(1), PushResult::Accepted { .. }));
        assert!(matches!(queue.try_push(2), PushResult::Stopped { .. }));
        assert!(queue.is_closed());
    }
}
