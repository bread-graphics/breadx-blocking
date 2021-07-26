// MIT/Apache2 License

use super::spawn_blocking;
use breadx::{
    display::{AsyncDisplay, Display, DisplayBase, PendingItem, PollOr, RequestInfo, StaticSetup},
    event::Event,
    XID,
};
use std::{
    future::Future,
    num::NonZeroU32,
    pin::Pin,
    task::{Context, Poll, Waker},
};

/// An `AsyncDisplay` that sends operations for the `Display` onto a blocking thread-pool.
pub struct BlockingDisplay<T> {
    display: Option<T>,

    // cached futures for wait() and send_request()
    wait: Option<Pin<Box<dyn Future<Output = (breadx::Result, T)> + Send + Sync + 'static>>>,
    send_request:
        Option<Pin<Box<dyn Future<Output = (breadx::Result<u16>, T)> + Send + Sync + 'static>>>,

    // wakers waiting on begin_send_request_info()
    send_pending_request_wakers: Vec<Waker>,
}

impl<T> BlockingDisplay<T> {
    /// Create a new `BlockingDisplay`.
    #[inline]
    pub fn new(display: T) -> BlockingDisplay<T> {
        BlockingDisplay {
            display: Some(display),
            wait: None,
            send_request: None,
            send_pending_request_wakers: Vec::new(),
        }
    }

    /// Get the inner display. This function will need to wait until it can cancel all of the ongoing wait or
    /// send request operations until it returns.
    #[inline]
    pub async fn into_inner(mut self) -> T {
        if let Some(display) = self.display.take() {
            display
        } else if let Some(wait) = self.wait.take() {
            let (_, display) = wait.await;
            display
        } else {
            let send_request = self
                .send_request
                .take()
                .unwrap_or_else(|| panic!("Invalid state"));
            let (_, display) = send_request.await;
            display
        }
    }

    /// Get a mutable reference to the inner display. This function will need to wait until it can cancel all of
    /// the ongoing wait or send request operations until it returns.
    #[inline]
    pub async fn get_mut(&mut self) -> &mut T {
        if let Some(ref mut display) = self.display {
            return display;
        }

        if let Some(wait) = self.wait.take() {
            let (_, display) = wait.await;
            self.display.insert(display)
        } else {
            let send_request = self
                .send_request
                .take()
                .unwrap_or_else(|| panic!("Invalid state"));
            let (_, display) = send_request.await;
            self.display.insert(display)
        }
    }

    #[inline]
    fn inner(&self) -> &T {
        self.display.as_ref().expect("Invalid state")
    }

    #[inline]
    fn inner_mut(&mut self) -> &mut T {
        self.display.as_mut().expect("Invalid state")
    }
}

// Note: functions in DisplayBase should be non-blocking, so we can just forward to the inner impl.
impl<T: DisplayBase> DisplayBase for BlockingDisplay<T> {
    #[inline]
    fn setup(&self) -> &StaticSetup {
        self.inner().setup()
    }

    #[inline]
    fn default_screen_index(&self) -> usize {
        self.inner().default_screen_index()
    }

    #[inline]
    fn next_request_number(&mut self) -> u64 {
        self.inner_mut().next_request_number()
    }

    #[inline]
    fn generate_xid(&mut self) -> Option<XID> {
        self.inner_mut().generate_xid()
    }

    #[inline]
    fn add_pending_item(&mut self, req_id: u16, item: PendingItem) {
        self.inner_mut().add_pending_item(req_id, item)
    }

    #[inline]
    fn get_pending_item(&mut self, req_id: u16) -> Option<PendingItem> {
        self.inner_mut().get_pending_item(req_id)
    }

    #[inline]
    fn take_pending_item(&mut self, req_id: u16) -> Option<PendingItem> {
        self.inner_mut().take_pending_item(req_id)
    }

    #[inline]
    fn has_pending_event(&self) -> bool {
        self.inner().has_pending_event()
    }

    #[inline]
    fn push_event(&mut self, event: Event) {
        self.inner_mut().push_event(event)
    }

    #[inline]
    fn pop_event(&mut self) -> Option<Event> {
        self.inner_mut().pop_event()
    }

    #[inline]
    fn create_special_event_queue(&mut self, xid: XID) {
        self.inner_mut().create_special_event_queue(xid)
    }

    #[inline]
    fn push_special_event(&mut self, xid: XID, event: Event) -> Result<(), Event> {
        self.inner_mut().push_special_event(xid, event)
    }

    #[inline]
    fn pop_special_event(&mut self, xid: XID) -> Option<Event> {
        self.inner_mut().pop_special_event(xid)
    }

    #[inline]
    fn delete_special_event_queue(&mut self, xid: XID) {
        self.inner_mut().delete_special_event_queue(xid)
    }

    #[inline]
    fn checked(&self) -> bool {
        self.inner().checked()
    }

    #[inline]
    fn set_checked(&mut self, checked: bool) {
        self.inner_mut().set_checked(checked)
    }

    #[inline]
    fn bigreq_enabled(&self) -> bool {
        self.inner().bigreq_enabled()
    }

    #[inline]
    fn max_request_len(&self) -> usize {
        self.inner().max_request_len()
    }

    #[inline]
    fn get_extension_opcode(&mut self, key: &[u8; 24]) -> Option<u8> {
        self.inner_mut().get_extension_opcode(key)
    }

    #[inline]
    fn set_extension_opcode(&mut self, key: [u8; 24], opcode: u8) {
        self.inner_mut().set_extension_opcode(key, opcode)
    }

    #[inline]
    fn wm_protocols_atom(&self) -> Option<NonZeroU32> {
        self.inner().wm_protocols_atom()
    }

    #[inline]
    fn set_wm_protocols_atom(&mut self, a: NonZeroU32) {
        self.inner_mut().set_wm_protocols_atom(a)
    }
}

impl<T: Display + Send + Sync + 'static> AsyncDisplay for BlockingDisplay<T> {
    #[inline]
    fn poll_wait(&mut self, cx: &mut Context<'_>) -> Poll<breadx::Result> {
        // start polling for wait if we haven't already
        let wait = match &mut self.wait {
            Some(wait) => wait,
            None => {
                let mut display = self.display.take().expect("Invalid state");
                let wait = spawn_blocking(move || {
                    let res = display.wait();
                    (res, display)
                });

                self.wait.insert(Box::pin(wait))
            }
        };

        match wait.as_mut().poll(cx) {
            Poll::Ready((res, display)) => {
                self.display = Some(display);
                self.wait = None;
                Poll::Ready(res)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    #[inline]
    fn begin_send_request_raw(
        &mut self,
        req: RequestInfo,
        cx: &mut Context<'_>,
    ) -> PollOr<(), RequestInfo> {
        if self.send_request.is_none() {
            let mut display = self.display.take().expect("Invalid state");
            let send_request = spawn_blocking(move || {
                let res = display.send_request_raw(req);
                (res, display)
            });

            self.send_request = Some(Box::pin(send_request));
            PollOr::Ready(())
        } else {
            self.send_pending_request_wakers.push(cx.waker().clone());
            PollOr::Pending(req)
        }
    }

    #[inline]
    fn poll_send_request_raw(&mut self, cx: &mut Context<'_>) -> Poll<breadx::Result<u16>> {
        let send_request = self.send_request.as_mut().expect("Invalid state");

        match send_request.as_mut().poll(cx) {
            Poll::Ready((res, display)) => {
                self.display = Some(display);
                self.send_request = None;
                // wake any waker that was waiting on this completing
                self.send_pending_request_wakers
                    .drain(..)
                    .for_each(|waker| waker.wake());
                Poll::Ready(res)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
