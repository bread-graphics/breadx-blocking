// MIT/Apache2 License

//! This crate provides the [`BlockingDisplay`] and [`BlockingDisplayImmut`] objects, which allow the user to
//! convert a `breadx::Display` into a `breadx::AsyncDisplay`.
//!
//! Occasionally, you have an object that implements `breadx::Display` that you need to implement
//! `breadx::AsyncDisplay`. Although the `*Display` objects in `breadx` can be easily changed to implement
//! `breadx::AsyncDisplay` by changing the `Connection` to an `AsyncConnection`, `Display` implementations
//! outside of `breadx` may not share this guarantee.
//!
//! `BlockingDisplay<T>` implements `AsyncDisplay` when `T` implements `Display`. `BlockingDisplayImmut<T>`
//! implements `AsyncDisplay` and `&AsyncDisplay` when `T` implements `&Display`.
//!
//! This is implemented on the [`blocking`] thread-pool when the `tokio` feature is not enabled, and is
//! implemented via [`spawn_blocking`] when it is.
//!
//! [`blocking`]: https://crates.io/crates/blocking
//! [`spawn_blocking`]: https://docs.rs/tokio/1.9.0/tokio/task/fn.spawn_blocking.html

#![forbid(unsafe_code)]

//#[cfg(any(feature = "immutable", not(feature = "tokio")))]
#[cfg(not(feature = "tokio-support"))]
use std::future::Future;

//#[cfg(feature = "immutable")]
//use std::task::{Context, Poll};

mod blocking_display;
pub use blocking_display::BlockingDisplay;

//#[cfg(feature = "immutable")]
//mod immutable;
//#[cfg(feature = "immutable")]
//pub use immutable::BlockingDisplayImmut;

#[cfg(not(feature = "tokio-support"))]
pub(crate) fn spawn_blocking<R: Send + 'static>(
    f: impl FnOnce() -> R + Send + 'static,
) -> impl Future<Output = R> {
    blocking::unblock(f)
}

#[cfg(feature = "tokio-support")]
pub(crate) async fn spawn_blocking<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    tokio::task::spawn_blocking(f)
        .await
        .expect("Task error occurred")
}

/*#[cfg(feature = "immutable")]
pub(crate) fn now_or_never<R>(f: impl Future<Output = R>) -> Option<R> {
    // pin future on stack
    pin_utils::pin_mut!(f);

    // create context
    let waker = noop_waker::noop_waker();
    let mut ctx = Context::from_waker(&waker);

    // poll future
    match f.poll(&mut ctx) {
        Poll::Ready(out) => Some(out),
        Poll::Pending => None,
    }
}*/
