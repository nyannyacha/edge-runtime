use std::thread::ThreadId;
use std::{future::pending, sync::atomic::Ordering, time::Duration};

use cpu_timer::CPUTimer;
use event_worker::events::ShutdownReason;
use log::error;
use sb_workers::context::{Timing, TimingStatus, UserWorkerMsgs};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{debug_span, instrument, Instrument};

use crate::rt_worker::supervisor::{
    handle_interrupt, CPUUsage, CPUUsageMetrics, IsolateInterruptData, Tokens,
};

use super::Arguments;

#[instrument(level = "debug", skip(args), fields(key = %args.key))]
pub async fn supervise(args: Arguments, oneshot: bool) -> (ShutdownReason, i64) {
    let Arguments {
        key,
        runtime_opts,
        timing,
        cpu_timer_param,
        cpu_usage_metrics_rx,
        mut memory_limit_rx,
        pool_msg_tx,
        isolate_memory_usage_tx,
        thread_safe_handle,
        tokens: Tokens {
            termination,
            supervise,
        },
        ..
    } = args;

    let Timing {
        status: TimingStatus { demand, is_retired },
        req: (mut req_start_rx, mut req_end_rx),
        ..
    } = timing.unwrap_or_default();

    let (_, hard_limit_ms) = cpu_timer_param.limits();

    let _guard = scopeguard::guard(is_retired, |v| {
        v.raise();
    });

    let mut current_thread_id = Option::<ThreadId>::None;
    let mut cpu_timer = Option::<CPUTimer>::None;

    let mut is_worker_entered = false;
    let mut cpu_usage_metrics_rx = cpu_usage_metrics_rx.unwrap();
    let mut cpu_usage_ms = 0i64;
    let mut cpu_usage_accumulated_ms = 0i64;
    let mut cpu_timer_rx = None::<mpsc::UnboundedReceiver<()>>;

    let mut complete_reason = None::<ShutdownReason>;
    let mut req_ack_count = 0usize;
    let mut req_start_ack = false;

    let wall_clock_limit_ms = runtime_opts.worker_timeout_ms;
    let is_wall_clock_limit_disabled = wall_clock_limit_ms == 0;

    let wall_clock_duration = Duration::from_millis(if wall_clock_limit_ms < 1 {
        1
    } else {
        wall_clock_limit_ms
    });

    let wall_clock_duration_alert = tokio::time::sleep(wall_clock_duration);

    tokio::pin!(wall_clock_duration_alert);

    loop {
        tokio::select! {
            _ = supervise.cancelled() => {
                return (ShutdownReason::TerminationRequested, cpu_usage_ms);
            }

            _ = async {
                match termination.as_ref() {
                    Some(token) => token.inbound.cancelled().await,
                    None => pending().await,
                }
            } => {
                complete_reason = Some(ShutdownReason::TerminationRequested);
            }

            Some(metrics) = cpu_usage_metrics_rx.recv() => {
                match metrics {
                    CPUUsageMetrics::Enter(thread_id, timer) => {
                        assert!(!is_worker_entered);
                        is_worker_entered = true;

                        let span = debug_span!(
                            "enter",
                            thread_id_was = ?current_thread_id,
                            thread_id_now = ?thread_id,
                        );

                        let _enter = span.enter();

                        current_thread_id = Some(thread_id);

                        if !cpu_timer_param.is_disabled() {
                            cpu_timer_rx = Some(timer.set_channel().in_current_span().await);

                            if let Err(err) = cpu_timer_param.reset(&timer) {
                                error!("can't reset cpu timer: {}", err);
                            }
                        }

                        cpu_timer = Some(timer);
                    }

                    CPUUsageMetrics::Leave(CPUUsage { accumulated, diff }) => {
                        assert!(is_worker_entered);

                        is_worker_entered = false;
                        cpu_usage_ms += diff / 1_000_000;
                        cpu_usage_accumulated_ms = accumulated / 1_000_000;

                        if !cpu_timer_param.is_disabled() {
                            if cpu_usage_ms >= hard_limit_ms as i64 {
                                error!("CPU time limit reached: isolate: {:?}", key);
                                complete_reason = Some(ShutdownReason::CPUTime);
                            }

                            if let Some(Err(err)) = cpu_timer.as_ref().map(|it| cpu_timer_param.reset(it)) {
                                error!("can't reset cpu timer: {}", err);
                            }
                        }

                        cpu_timer = None;
                    }
                }
            }

            _ = async {
                if let Some(timer) = cpu_timer_rx.as_mut() {
                    let _ = timer.recv().await;
                } else {
                    pending::<()>().await
                }
            } => {
                if is_worker_entered && req_start_ack {
                    error!("CPU time limit reached: isolate: {:?}", key);
                    complete_reason = Some(ShutdownReason::CPUTime);
                }
            }

            Some(notify) = req_start_rx.recv() => {
                // INVARIANT: This branch MUST not be satisfied more than once
                // during the same request cycle.
                assert!(!req_start_ack, "supervisor has seen request start signal twice");

                notify.notify_one();

                if let Some(Err(err)) = cpu_timer.as_ref().map(|it| cpu_timer_param.reset(it)) {
                    error!("cannot reset the cpu timer: {}", err);
                }

                cpu_usage_ms = 0;
                req_start_ack = true;
                complete_reason = None;
            }

            Some(_) = req_end_rx.recv() => {
                // INVARIANT: This branch MUST be satisfied only once after the
                // request start signal has arrived during the same request
                // cycle.
                assert!(req_start_ack, "supervisor observed the request end signal but did not see request start signal");

                req_ack_count += 1;
                complete_reason = Some(ShutdownReason::EarlyDrop);
            }

            _ = &mut wall_clock_duration_alert, if !is_wall_clock_limit_disabled => {
                if !oneshot && req_ack_count != demand.load(Ordering::Acquire) {
                    wall_clock_duration_alert
                        .as_mut()
                        .reset(Instant::now() + wall_clock_duration);

                    continue;
                } else {
                    error!("wall clock duraiton reached: isolate: {:?}", key);
                    complete_reason = Some(ShutdownReason::WallClockTime);
                }
            }

            Some(_) = memory_limit_rx.recv() => {
                error!("memory limit reached for the worker: isolate: {:?}", key);
                complete_reason = Some(ShutdownReason::Memory);
            }
        }

        match complete_reason.take() {
            Some(ShutdownReason::EarlyDrop) if !oneshot => {
                req_start_ack = false;
                wall_clock_duration_alert
                    .as_mut()
                    .reset(Instant::now() + wall_clock_duration);

                if let Some(tx) = pool_msg_tx.clone() {
                    if tx.send(UserWorkerMsgs::Idle(key)).is_err() {
                        error!("failed to send idle msg to pool: {:?}", key);
                    }
                }

                continue;
            }

            Some(reason) => {
                let data_ptr_mut = Box::into_raw(Box::new(IsolateInterruptData {
                    should_terminate: true,
                    isolate_memory_usage_tx: Some(isolate_memory_usage_tx),
                }));

                if !thread_safe_handle
                    .request_interrupt(handle_interrupt, data_ptr_mut as *mut std::ffi::c_void)
                {
                    drop(unsafe { Box::from_raw(data_ptr_mut) });
                }

                return (reason, cpu_usage_accumulated_ms);
            }

            None => continue,
        }
    }
}
