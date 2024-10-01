use super::{handle_interrupt, Arguments, CPUUsageMetrics, IsolateInterruptData};
use crate::worker::supervisor::{CPUUsage, Tokens};

use std::thread::ThreadId;
use std::{future::pending, sync::atomic::Ordering, time::Duration};

use log::error;
use runtime_base::event::ShutdownReason;
use sb_workers::context::{Timing, TimingStatus, UserWorkerMsgs};
use tokio::sync::mpsc;
use tracing::{debug_span, Instrument};

pub async fn supervise(args: Arguments) -> (ShutdownReason, i64) {
    let Arguments {
        key,
        runtime_opts,
        timing,
        mut memory_limit_rx,
        cpu_timer_param,
        cpu_usage_metrics_rx,
        pool_msg_tx,
        isolate_memory_usage_tx,
        isolate_handle_op_sender,
        tokens: Tokens {
            termination,
            supervise,
        },
        ..
    } = args;

    let Timing {
        status: TimingStatus { demand, is_retired },
        req: (_, mut req_end_rx),
    } = timing.unwrap_or_default();

    let (soft_limit_ms, hard_limit_ms) = cpu_timer_param.limits();

    let guard = scopeguard::guard(is_retired, |v| {
        v.raise();
    });

    let mut current_thread_id = Option::<ThreadId>::None;

    let mut is_worker_entered = false;
    let mut cpu_usage_metrics_rx = cpu_usage_metrics_rx.unwrap();
    let mut cpu_usage_ms = 0i64;
    let mut cpu_timer_rx = None::<mpsc::UnboundedReceiver<()>>;

    let mut cpu_time_soft_limit_reached = false;
    let mut wall_clock_alerts = 0;
    let mut req_ack_count = 0usize;

    let wall_clock_limit_ms = runtime_opts.worker_timeout_ms;
    let is_wall_clock_limit_disabled = wall_clock_limit_ms == 0;

    let wall_clock_duration = Duration::from_millis(if wall_clock_limit_ms < 2 {
        2
    } else {
        wall_clock_limit_ms
    });

    // Split wall clock duration into 2 intervals.
    // At the first interval, we will send a msg to retire the worker.
    let wall_clock_duration_alert = tokio::time::interval(
        wall_clock_duration
            .checked_div(2)
            .unwrap_or(Duration::from_millis(1)),
    );

    let early_retire_fn = || {
        // we should raise a retire signal because subsequent incoming requests are unlikely to get
        // enough wall clock time or cpu time
        guard.raise();
    };

    let terminate_fn = {
        move || {
            isolate_handle_op_sender.request_interrupt(move |isolate| {
                handle_interrupt(
                    isolate,
                    IsolateInterruptData {
                        should_terminate: true,
                        isolate_memory_usage_tx: Some(isolate_memory_usage_tx),
                    },
                );
            });
        }
    };

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
                terminate_fn();
                return (ShutdownReason::TerminationRequested, cpu_usage_ms);
            }

            Some(metrics) = cpu_usage_metrics_rx.recv() => {
                match metrics {
                    CPUUsageMetrics::Enter(thread_id, timer) => {
                        assert!(!is_worker_entered);
                        is_worker_entered = true;

                        super::log_if_thread_has_changed(current_thread_id, thread_id);

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
                    }

                    CPUUsageMetrics::Leave(CPUUsage { accumulated, .. }) => {
                        assert!(is_worker_entered);

                        is_worker_entered = false;
                        cpu_usage_ms = accumulated / 1_000_000;

                        if !cpu_timer_param.is_disabled() {
                            if cpu_usage_ms >= hard_limit_ms as i64 {
                                terminate_fn();
                                error!("CPU time hard limit reached: isolate: {:?}", key);
                                return (ShutdownReason::CPUTime, cpu_usage_ms);
                            } else if cpu_usage_ms >= soft_limit_ms as i64 && !cpu_time_soft_limit_reached {
                                early_retire_fn();
                                error!("CPU time soft limit reached: isolate: {:?}", key);
                                cpu_time_soft_limit_reached = true;

                                if req_ack_count == demand.load(Ordering::Acquire) {
                                    terminate_fn();
                                    error!("early termination due to the last request being completed: isolate: {:?}", key);
                                    return (ShutdownReason::EarlyDrop, cpu_usage_ms);
                                }
                            }
                        }
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
                if is_worker_entered {
                    if !cpu_time_soft_limit_reached {
                        early_retire_fn();
                        error!("CPU time soft limit reached: isolate: {:?}", key);
                        cpu_time_soft_limit_reached = true;

                        if req_ack_count == demand.load(Ordering::Acquire) {
                            terminate_fn();
                            error!("early termination due to the last request being completed: isolate: {:?}", key);
                            return (ShutdownReason::EarlyDrop, cpu_usage_ms);
                        }
                    } else {
                        terminate_fn();
                        error!("CPU time hard limit reached: isolate: {:?}", key);
                        return (ShutdownReason::CPUTime, cpu_usage_ms);
                    }
                }
            }

            Some(_) = req_end_rx.recv() => {
                req_ack_count += 1;

                if !cpu_time_soft_limit_reached {
                    if let Some(tx) = pool_msg_tx.clone() {
                        if tx.send(UserWorkerMsgs::Idle(key)).is_err() {
                            error!("failed to send idle msg to pool: {:?}", key);
                        }
                    }
                }

                if !cpu_time_soft_limit_reached || req_ack_count != demand.load(Ordering::Acquire) {
                    continue;
                }

                terminate_fn();
                error!("early termination due to the last request being completed: isolate: {:?}", key);
                return (ShutdownReason::EarlyDrop, cpu_usage_ms);
            }

            _ = wall_clock_duration_alert.tick(), if !is_wall_clock_limit_disabled => {
                if wall_clock_alerts == 0 {
                    // first tick completes immediately
                    wall_clock_alerts += 1;
                } else if wall_clock_alerts == 1 {
                    early_retire_fn();
                    error!("wall clock duration warning: isolate: {:?}", key);
                    wall_clock_alerts += 1;
                } else {
                    let is_in_flight_req_exists = req_ack_count != demand.load(Ordering::Acquire);

                    terminate_fn();

                    error!("wall clock duration reached: isolate: {:?} (in_flight_req_exists = {})", key, is_in_flight_req_exists);

                    return (ShutdownReason::WallClockTime, cpu_usage_ms);
                }
            }

            Some(_) = memory_limit_rx.recv() => {
                terminate_fn();
                error!("memory limit reached for the worker: isolate: {:?}", key);
                return (ShutdownReason::Memory, cpu_usage_ms);
            }
        }
    }
}
