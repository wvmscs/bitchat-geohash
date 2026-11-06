// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the monitor needs from a runtime: tasks, timers, a clock, chance.
//!
//! Native build: tokio, as before. Browser build (`wasm32`): one thread, so
//! tasks run on the page's event loop (`spawn_local`), timers are
//! `setTimeout`, blocking work runs in place, and the clocks are the
//! browser's (`std::time` panics there). The futures of the browser build
//! need not be `Send` ([`MaybeSend`]).

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

/// `Send` in the native build only: browser futures hold JavaScript values.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// The task ended without an output (panicked, or its runtime stopped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

/// A deadline passed before the future ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed;

/// Awaits the output of a task started with [`spawn`].
pub struct JoinHandle<T> {
    #[cfg(not(target_arch = "wasm32"))]
    inner: tokio::task::JoinHandle<T>,
    #[cfg(target_arch = "wasm32")]
    inner: tokio::sync::oneshot::Receiver<T>,
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, Stopped>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.inner)
            .poll(cx)
            .map(|r| r.map_err(|_| Stopped))
    }
}

/// Runs a future as its own task.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + MaybeSend + 'static,
    F::Output: MaybeSend + 'static,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        JoinHandle {
            inner: tokio::spawn(future),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(future.await);
        });
        JoinHandle { inner: rx }
    }
}

/// Runs blocking work (the database) off the async threads; in the browser,
/// in place.
pub async fn blocking<F, T>(work: F) -> Result<T, Stopped>
where
    F: FnOnce() -> T + MaybeSend + 'static,
    T: MaybeSend + 'static,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::task::spawn_blocking(work).await.map_err(|_| Stopped)
    }
    #[cfg(target_arch = "wasm32")]
    {
        Ok(work())
    }
}

#[cfg(target_arch = "wasm32")]
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// `globalThis.setTimeout`: a page and a worker both have it.
        #[wasm_bindgen(js_name = setTimeout)]
        pub fn set_timeout(handler: &js_sys::Function, ms: i32) -> JsValue;
        #[wasm_bindgen(js_namespace = performance, js_name = now)]
        pub fn performance_now() -> f64;
    }
}

pub async fn sleep(duration: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::sleep(duration).await
    }
    #[cfg(target_arch = "wasm32")]
    {
        let ms = duration.as_millis().min(i32::MAX as u128) as i32;
        let done = js_sys::Promise::new(&mut |resolve, _| {
            js::set_timeout(&resolve, ms);
        });
        let _ = wasm_bindgen_futures::JsFuture::from(done).await;
    }
}

/// `future`, unless `duration` passes first.
pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, Elapsed> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::timeout(duration, future)
            .await
            .map_err(|_| Elapsed)
    }
    #[cfg(target_arch = "wasm32")]
    {
        use futures_util::future::{Either, select};
        let (future, deadline) = (std::pin::pin!(future), std::pin::pin!(sleep(duration)));
        match select(future, deadline).await {
            Either::Left((output, _)) => Ok(output),
            Either::Right(_) => Err(Elapsed),
        }
    }
}

/// A monotonic instant.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Instant {
    #[cfg(not(target_arch = "wasm32"))]
    inner: std::time::Instant,
    /// `performance.now()`, milliseconds.
    #[cfg(target_arch = "wasm32")]
    ms: f64,
}

impl Instant {
    pub fn now() -> Instant {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Instant {
                inner: std::time::Instant::now(),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Instant {
                ms: js::performance_now(),
            }
        }
    }

    pub fn elapsed(&self) -> Duration {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.elapsed()
        }
        #[cfg(target_arch = "wasm32")]
        {
            Duration::from_secs_f64((js::performance_now() - self.ms).max(0.0) / 1000.0)
        }
    }
}

/// Unix time, in seconds.
pub fn unix_now() -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0) as i64
    }
}

/// A number in [0, 1), not for secrets (reconnection jitter).
pub fn random_unit() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::hash::{BuildHasher, RandomState};
        let r = RandomState::new().hash_one(std::time::Instant::now()) % 1000;
        r as f64 / 1000.0
    }
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Math::random()
    }
}

/// Ticks every `period`; a late tick delays the next ones (tokio's
/// `MissedTickBehavior::Delay`). [`Interval::tick`] is cancel-safe.
pub struct Interval {
    #[cfg(not(target_arch = "wasm32"))]
    inner: tokio::time::Interval,
    #[cfg(target_arch = "wasm32")]
    next: Instant,
    #[cfg(target_arch = "wasm32")]
    period: Duration,
}

/// The first tick at once, then every `period`.
pub fn interval(period: Duration) -> Interval {
    interval_after(Duration::ZERO, period)
}

/// The first tick after `first`, then every `period`.
pub fn interval_after(first: Duration, period: Duration) -> Interval {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut inner = tokio::time::interval_at(tokio::time::Instant::now() + first, period);
        inner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Interval { inner }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let now = Instant::now();
        Interval {
            next: Instant {
                ms: now.ms + first.as_secs_f64() * 1000.0,
            },
            period,
        }
    }
}

impl Interval {
    pub async fn tick(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.tick().await;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let wait = self.next.ms - Instant::now().ms;
            if wait > 0.0 {
                sleep(Duration::from_secs_f64(wait / 1000.0)).await;
            }
            // Only once the tick happened: a cancelled tick changes nothing.
            self.next = Instant {
                ms: Instant::now().ms + self.period.as_secs_f64() * 1000.0,
            };
        }
    }
}
