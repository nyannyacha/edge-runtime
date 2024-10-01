use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    future::Future,
    marker::PhantomData,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::Poll,
    time::Duration,
};

use crate::event::WorkerEvents;
use deno_core::{error::AnyError, JsRuntime};
use deno_unsync::MaskFutureAsSend;
use futures_util::{
    future::{poll_fn, Fuse, LocalBoxFuture, Shared},
    FutureExt,
};
use nohash_hasher::IntMap;
use once_cell::sync::OnceCell;
use scopeguard::guard;
use tokio::{
    pin,
    runtime::{Handle, RuntimeFlavor},
    task::spawn_blocking,
};
use tokio_util::sync::CancellationToken;

// Following static variables are initialized in the cli crate.
pub static ENABLED: OnceCell<bool> = OnceCell::new();
pub static SAMPLE_SIZE: OnceCell<u32> = OnceCell::new();
pub static WORKER_THREAD_THRESHOLD: OnceCell<Duration> = OnceCell::new();
pub static BLOCKING_THREAD_THRESHOLD: OnceCell<Duration> = OnceCell::new();
pub static BLOCKING_THREAD_RECHECK_INTERVAL: OnceCell<Duration> = OnceCell::new();

type SharedRuntimeFuture =
    Shared<Fuse<LocalBoxFuture<'static, Result<WorkerEvents, Arc<AnyError>>>>>;

thread_local! {
    static IN_BLOCKING_THREAD: Cell<bool> = Cell::default();
    static COUNTER: RefCell<AtomicU64> = RefCell::default();
    static MARKED_RUNTIME_FUT: RefCell<IntMap<u64, SharedRuntimeFuture>> = RefCell::default();
}

pub fn mark_rt_fut<Fut>(future: Fut) -> impl Future<Output = Fut::Output>
where
    Fut: Future<Output = Result<WorkerEvents, Arc<AnyError>>>,
    Fut: 'static,
{
    let boxed = future.boxed_local();
    let fut = boxed.fuse().shared();
    let key = COUNTER.with_borrow(|it| it.fetch_add(1, Ordering::Relaxed));
    let _ = MARKED_RUNTIME_FUT.with_borrow_mut(|it| it.insert(key, fut.clone()));

    async move {
        let fut = unsafe { MaskFutureAsSend::new(fut) };
        let _guard = scopeguard::guard((), move |_| {
            MARKED_RUNTIME_FUT.with_borrow_mut(|it| {
                let old_value = it.remove(&key);
                debug_assert!(old_value.is_some());
            });
        });

        fut.await.into_inner()
    }
}

pub struct MovingAvg<T, W> {
    history: RefCell<VecDeque<T>>,
    window_size: (W, usize),
    _phantom_unit: PhantomData<W>,
}

impl<T, W, S> MovingAvg<T, W>
where
    T: std::ops::Div<W, Output = T> + std::iter::Sum<T> + Copy,
    T: Sample<Start = S, End = T>,
    W: TryInto<usize> + Copy,
    S: Copy,
{
    fn new(window_size: W) -> Self {
        let window_size = (window_size, window_size.try_into().unwrap_or_default());

        Self {
            history: RefCell::new(VecDeque::with_capacity(window_size.1)),
            window_size,
            _phantom_unit: PhantomData,
        }
    }

    pub fn update(&self, sample: T) {
        let mut history = self.history.borrow_mut();
        if history.len() == self.window_size.1 {
            history.pop_front();
        }
        history.push_back(sample);
    }

    pub fn update_guard(&self) -> MovingAvgUpdateGuard<'_, T, W, S> {
        MovingAvgUpdateGuard {
            scope: self,
            sample: <T as Sample>::start(),
        }
    }

    pub fn in_scope<F, R>(&self, func: F) -> R
    where
        F: FnOnce() -> R,
    {
        let _guard = self.update_guard();
        func()
    }

    pub fn avg(&self) -> T {
        self.history.borrow().iter().copied().sum::<T>() / self.window_size.0
    }
}

pub trait Sample {
    type Start;
    type End;

    fn start() -> Self::Start;
    fn end(start: Self::Start) -> Self::End;
}

impl Sample for Duration {
    type Start = std::time::Instant;
    type End = Duration;

    fn start() -> Self::Start {
        std::time::Instant::now()
    }

    fn end(start: Self::Start) -> Self::End {
        start.elapsed()
    }
}

pub struct MovingAvgUpdateGuard<'l, T, W, S>
where
    T: std::ops::Div<W, Output = T> + std::iter::Sum<T> + Copy,
    T: Sample<Start = S, End = T>,
    W: TryInto<usize> + Copy,
    S: Copy,
{
    scope: &'l MovingAvg<T, W>,
    sample: S,
}

impl<'l, T, W, S> std::ops::Drop for MovingAvgUpdateGuard<'l, T, W, S>
where
    T: std::ops::Div<W, Output = T> + std::iter::Sum<T> + Copy,
    T: Sample<Start = S, End = T>,
    W: TryInto<usize> + Copy,
    S: Copy,
{
    fn drop(&mut self) {
        self.scope.update(<T as Sample>::end(self.sample))
    }
}

pub struct SchedMetric {
    pub poll_event_loop: MovingAvg<Duration, u32>,
    pub blocking_call: MovingAvg<Duration, u32>,
}

impl SchedMetric {
    fn get(js_runtime: &mut JsRuntime) -> Option<Rc<SchedMetric>> {
        if !ENABLED.get().copied().unwrap_or_default() {
            return None;
        }

        let sample_size = SAMPLE_SIZE.get().copied().unwrap_or_default();
        debug_assert_ne!(sample_size, 0);

        let state = js_runtime.op_state();
        let mut state = state.borrow_mut();
        if !state.has::<Self>() {
            state.put(Rc::new(Self {
                poll_event_loop: MovingAvg::new(sample_size),
                blocking_call: MovingAvg::new(sample_size),
            }));
        }

        Some(state.borrow::<Rc<Self>>().clone())
    }
}

pub fn poll_marked_rt_while<F, R>(js_runtime: &mut JsRuntime, func: F) -> R
where
    F: FnOnce() -> R + 'static,
    R: 'static,
{
    let metric = SchedMetric::get(js_runtime);

    if IN_BLOCKING_THREAD.get() {
        let Some(metric) = metric else { return func() };
        return metric.blocking_call.in_scope(func);
    }

    let handle = Handle::current();
    let token = CancellationToken::new();
    let _guard = token.clone().drop_guard();

    debug_assert!(handle.runtime_flavor() == RuntimeFlavor::CurrentThread);

    let rt_fut_list = MARKED_RUNTIME_FUT.with_borrow(|it| {
        it.values()
            .cloned()
            .map(|it| unsafe { MaskFutureAsSend::new(it) })
            .collect::<Vec<_>>()
    });

    for rt_fut in rt_fut_list {
        spawn_blocking({
            // there is no io driver or timer driver here
            let handle = handle.clone();
            let cancel_fut = token.clone().cancelled_owned();
            move || {
                let _guard = {
                    IN_BLOCKING_THREAD.set(true);
                    guard((), |_| {
                        let _ = IN_BLOCKING_THREAD.take();
                    })
                };

                pin!(cancel_fut);
                pin!(rt_fut);

                handle.block_on(poll_fn(move |cx| {
                    if cancel_fut.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(());
                    }

                    if rt_fut.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(());
                    }

                    Poll::Pending
                }));
            }
        });
    }

    let Some(metric) = metric else {
        return func();
    };

    metric.blocking_call.in_scope(func)
}
