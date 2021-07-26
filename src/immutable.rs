// MIT/Apache2 License

use super::{now_or_never, spawn_blocking};
use breadx::{
    display::{AsyncDisplay, Display, DisplayBase, PendingItem, PollOr, RequestInfo, StaticSetup},
    event::Event,
    XID,
};
use std::{
    future::Future,
    mem,
    num::NonZeroU32,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};

#[cfg(all(feature = "immutable", not(feature = "tokio-support")))]
use async_lock::Mutex;

#[cfg(all(feature = "immutable", feature = "tokio-support"))]
use tokio::sync::Mutex;

#[cfg(feature = "immutable")]
use concurrent_queue::ConcurrentQueue;

/// Similar to `BlockingDisplay`, but allows for usage via an immutable reference.
pub struct BlockingDisplayImmut<T> {
    inner: Arc<Inner<T>>,
}

struct Inner<T> {
    display: T,
    state: Mutex<State>,
    pending_task_wakers: ConcurrentQueue<Waker>,
}

enum State {
    Inactive,
    Wait(Pin<Box<dyn Future<Output = breadx::Result> + Send + Sync + 'static>>),
    SendRequest(Pin<Box<dyn Future<Output = breadx::Result<u16>> + Send + Sync + 'static>>),
}

impl<T> BlockingDisplayImmut<T> {
    /// Create a new `BlockingDisplayImmut<T>`.
    #[inline]
    pub fn new(display: T) -> BlockingDisplayImmut<T> {
        BlockingDisplayImmut {
            inner: Arc::new(Inner {
                display,
                state: Mutex::new(State::Inactive),
                pending_task_wakers: ConcurrentQueue::unbounded(),
            }),
        }
    }

    /// Get the inner display. This function will need to wait until it can cancel all of the ongoing wait or
    /// send request operations until it returns.
    #[inline]
    pub async fn into_inner(self) -> T {
        let mut state = self.state().lock().await;

        // put ourselves into an inactive state
        match mem::replace(&mut *state, State::Inactive) {
            State::Inactive => {}
            State::Wait(wait) => {
                let _ = wait.await;
            }
            State::SendRequest(send_request) => {
                let _ = send_request.await;
            }
        }

        mem::drop(state);

        let inner = Arc::try_unwrap(self.inner).unwrap_or_else(|_| panic!("Invalid state"));
        inner.display
    }

    /// Get a mutable reference to the inner display.
    #[inline]
    pub async fn get_mut(&mut self) -> &mut T {
        let mut state = self.state().lock().await;

        // put ourselves into an inactive state
        match mem::replace(&mut *state, State::Inactive) {
            State::Inactive => {}
            State::Wait(wait) => {
                let _ = wait.await;
            }
            State::SendRequest(send_request) => {
                let _ = send_request.await;
            }
        }

        mem::drop(state);

        &mut Arc::get_mut(&mut self.inner)
            .expect("Invalid state")
            .display
    }

    #[inline]
    fn state(&self) -> &Mutex<State> {
        &self.inner.state
    }

    #[inline]
    fn inner<'a>(&'a self) -> &'a T {
        &self.inner.display
    }

    #[inline]
    fn pending_waker(&self, waker: Waker) {
        self.inner.pending_task_wakers.push(waker).ok();
    }

    #[inline]
    fn release_wakers(&self) {
        while let Ok(waker) = self.inner.pending_task_wakers.pop() {
            waker.wake();
        }
    }
}

impl<T: 'static + Send + Sync> BlockingDisplayImmut<T>
where
    &'static T: Display,
{
    #[inline]
    fn poll_wait_internal(&self, cx: &mut Context<'_>) -> Poll<breadx::Result> {
        let mut state = match now_or_never(self.state().lock()) {
            Some(state) => state,
            None => {
                self.pending_waker(cx.waker().clone());
                return Poll::Pending;
            }
        };

        let mut wait = match &mut *state {
            State::Inactive => {
                let cloned = self.inner.clone();
                let wait = spawn_blocking(move || (&cloned.display).wait());
                *state = State::Wait(Box::pin(wait));

                match &mut *state {
                    State::Wait(wait) => wait,
                    _ => unreachable!(),
                }
            }
            State::Wait(wait) => wait,
            State::SendRequest(_) => {
                self.pending_waker(cx.waker().clone());
                return Poll::Pending;
            }
        };

        match wait.as_mut().poll(cx) {
            Poll::Ready(res) => {
                // release the pending wakers and reset the state
                *state = State::Inactive;
                mem::drop(state);

                self.release_wakers();
                Poll::Ready(res)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    #[inline]
    fn begin_send_request_internal(
        &self,
        ri: RequestInfo,
        cx: &mut Context<'_>,
    ) -> PollOr<(), RequestInfo> {
        let mut state = match now_or_never(self.state().lock()) {
            Some(state) => state,
            None => {
                self.pending_waker(cx.waker().clone());
                return PollOr::Pending(ri);
            }
        };

        match &mut *state {
            State::Inactive => {
                let cloned = self.inner.clone();
                let send_request = spawn_blocking(move || (&cloned.display).send_request_raw(ri));
                *state = State::SendRequest(Box::pin(send_request));

                PollOr::Ready(())
            }
            _ => {
                self.pending_waker(cx.waker().clone());
                PollOr::Pending(ri)
            }
        }
    }

    #[inline]
    fn poll_send_request_internal(&self, cx: &mut Context<'_>) -> Poll<breadx::Result<u16>> {
        let mut state = match now_or_never(self.state().lock()) {
            Some(state) => state,
            None => {
                self.pending_waker(cx.waker().clone());
                return Poll::Pending;
            }
        };

        let mut send_request = match &mut *state {
            State::SendRequest(send_request) => send_request,
            _ => panic!("Invalid state"),
        };

        match send_request.as_mut().poll(cx) {
            Poll::Ready(res) => {
                *state = State::Inactive;
                mem::drop(state);

                self.release_wakers();
                Poll::Ready(res)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/*
impl<T: 'static> DisplayBase for BlockingDisplayImmut<T> where &T: DisplayBase {
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
        self.inner().next_request_number()
    }

    #[inline]
    fn generate_xid(&mut self) -> Option<XID> {
        self.inner().generate_xid()
    }

    #[inline]
    fn add_pending_item(&mut self, req_id: u16, item: PendingItem) {
        self.inner().add_pending_item(req_id, item)
    }

    #[inline]
    fn get_pending_item(&mut self, req_id: u16) -> Option<PendingItem> {
        self.inner().get_pending_item(req_id)
    }

    #[inline]
    fn take_pending_item(&mut self, req_id: u16) -> Option<PendingItem> {
        self.inner().take_pending_item(req_id)
    }

    #[inline]
    fn has_pending_event(&self) -> bool {
        self.inner().has_pending_event()
    }

    #[inline]
    fn push_event(&mut self, event: Event) {
        self.inner().push_event(event)
    }

    #[inline]
    fn pop_event(&mut self) -> Option<Event> {
        self.inner().pop_event()
    }

    #[inline]
    fn create_special_event_queue(&mut self, xid: XID) {
        self.inner().create_special_event_queue(xid)
    }

    #[inline]
    fn push_special_event(&mut self, xid: XID, event: Event) -> Result<(), Event> {
        self.inner().push_special_event(xid, event)
    }

    #[inline]
    fn pop_special_event(&mut self, xid: XID) -> Option<Event> {
        self.inner().pop_special_event(xid)
    }

    #[inline]
    fn delete_special_event_queue(&mut self, xid: XID) {
        self.inner().delete_special_event_queue(xid)
    }

    #[inline]
    fn checked(&self) -> bool {
        self.inner().checked()
    }

    #[inline]
    fn set_checked(&mut self, checked: bool) {
        self.inner().set_checked(checked)
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
        self.inner().get_extension_opcode(key)
    }

    #[inline]
    fn set_extension_opcode(&mut self, key: [u8; 24], opcode: u8) {
        self.inner().set_extension_opcode(key, opcode)
    }

    #[inline]
    fn wm_protocols_atom(&self) -> Option<NonZeroU32> {
        self.inner().wm_protocols_atom()
    }

    #[inline]
    fn set_wm_protocols_atom(&mut self, a: NonZeroU32) {
        self.inner().set_wm_protocols_atom(a)
    }
}
*/

impl<'a, T: 'static> DisplayBase for &'a BlockingDisplayImmut<T>
where
    &'a T: DisplayBase,
{
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
        self.inner().next_request_number()
    }

    #[inline]
    fn generate_xid(&mut self) -> Option<XID> {
        self.inner().generate_xid()
    }

    #[inline]
    fn add_pending_item(&mut self, req_id: u16, item: PendingItem) {
        self.inner().add_pending_item(req_id, item)
    }

    #[inline]
    fn get_pending_item(&mut self, req_id: u16) -> Option<PendingItem> {
        self.inner().get_pending_item(req_id)
    }

    #[inline]
    fn take_pending_item(&mut self, req_id: u16) -> Option<PendingItem> {
        self.inner().take_pending_item(req_id)
    }

    #[inline]
    fn has_pending_event(&self) -> bool {
        self.inner().has_pending_event()
    }

    #[inline]
    fn push_event(&mut self, event: Event) {
        self.inner().push_event(event)
    }

    #[inline]
    fn pop_event(&mut self) -> Option<Event> {
        self.inner().pop_event()
    }

    #[inline]
    fn create_special_event_queue(&mut self, xid: XID) {
        self.inner().create_special_event_queue(xid)
    }

    #[inline]
    fn push_special_event(&mut self, xid: XID, event: Event) -> Result<(), Event> {
        self.inner().push_special_event(xid, event)
    }

    #[inline]
    fn pop_special_event(&mut self, xid: XID) -> Option<Event> {
        self.inner().pop_special_event(xid)
    }

    #[inline]
    fn delete_special_event_queue(&mut self, xid: XID) {
        self.inner().delete_special_event_queue(xid)
    }

    #[inline]
    fn checked(&self) -> bool {
        self.inner().checked()
    }

    #[inline]
    fn set_checked(&mut self, checked: bool) {
        self.inner().set_checked(checked)
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
        self.inner().get_extension_opcode(key)
    }

    #[inline]
    fn set_extension_opcode(&mut self, key: [u8; 24], opcode: u8) {
        self.inner().set_extension_opcode(key, opcode)
    }

    #[inline]
    fn wm_protocols_atom(&self) -> Option<NonZeroU32> {
        self.inner().wm_protocols_atom()
    }

    #[inline]
    fn set_wm_protocols_atom(&mut self, a: NonZeroU32) {
        self.inner().set_wm_protocols_atom(a)
    }
}

/*
impl<T> AsyncDisplay for BlockingDisplayImmut<T> where &T: Display {
    #[inline]
    fn poll_wait(&mut self, ctx: &mut Context<'_>) -> Poll<breadx::Result> {
        self.poll_wait_internal(ctx)
    }

    #[inline]
    fn begin_send_request_raw(&mut self, ri: RequestInfo, ctx: &mut Context<'_>) -> PollOr<(), RequestInfo> {
        self.begin_send_request_internal(ri, ctx)
    }

    #[inline]
    fn poll_send_request_raw(&mut self, ctx: &mut Context<'_>) -> Poll<breadx::Result<u16>> {
        self.poll_send_request_internal(ctx)
    }
}
*/

impl<'a, T: Send + Sync + 'static> AsyncDisplay for &'a BlockingDisplayImmut<T>
where
    &'a T: Display,
{
    #[inline]
    fn poll_wait(&mut self, ctx: &mut Context<'_>) -> Poll<breadx::Result> {
        self.poll_wait_internal(ctx)
    }

    #[inline]
    fn begin_send_request_raw(
        &mut self,
        ri: RequestInfo,
        ctx: &mut Context<'_>,
    ) -> PollOr<(), RequestInfo> {
        self.begin_send_request_internal(ri, ctx)
    }

    #[inline]
    fn poll_send_request_raw(&mut self, ctx: &mut Context<'_>) -> Poll<breadx::Result<u16>> {
        self.poll_send_request_internal(ctx)
    }
}
