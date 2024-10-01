pub mod strategy_per_request;
pub mod strategy_per_worker;

use std::thread::ThreadId;

use anyhow::Error;
use cpu_timer::CPUTimer;
use enum_as_inner::EnumAsInner;
use log::error;
use runtime_base::isolate::IsolateHandleOpSender;
use sb_workers::context::{Timing, UserWorkerMsgs, UserWorkerRuntimeOpts};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use super::{ctx::TerminationToken, pool::SupervisorPolicy};

#[repr(C)]
pub struct IsolateInterruptData {
    pub should_terminate: bool,
    pub isolate_memory_usage_tx: Option<oneshot::Sender<IsolateMemoryStats>>,
}

pub fn handle_interrupt(isolate: &mut deno_core::v8::Isolate, mut data: IsolateInterruptData) {
    // log memory usage
    let mut heap_stats = deno_core::v8::HeapStatistics::default();

    isolate.get_heap_statistics(&mut heap_stats);

    let usage = IsolateMemoryStats {
        used_heap_size: heap_stats.used_heap_size(),
        external_memory: heap_stats.external_memory(),
    };

    if let Some(usage_tx) = data.isolate_memory_usage_tx.take() {
        if usage_tx.send(usage).is_err() {
            error!("failed to send isolate memory usage - receiver may have been dropped");
        }
    }

    if data.should_terminate {
        isolate.terminate_execution();
    }
}

#[repr(C)]
pub struct IsolateMemoryStats {
    pub used_heap_size: usize,
    pub external_memory: usize,
}

#[derive(Clone, Copy)]
pub struct CPUTimerParam {
    soft_limit_ms: u64,
    hard_limit_ms: u64,
}

impl CPUTimerParam {
    pub fn new(soft_limit_ms: u64, hard_limit_ms: u64) -> Self {
        Self {
            soft_limit_ms,
            hard_limit_ms,
        }
    }

    pub fn limits(&self) -> (u64, u64) {
        (self.soft_limit_ms, self.hard_limit_ms)
    }

    pub fn is_disabled(&self) -> bool {
        self.soft_limit_ms == 0 && self.hard_limit_ms == 0
    }

    pub fn reset(&self, cpu_timer: &CPUTimer) -> Result<(), Error> {
        cpu_timer.reset(self.soft_limit_ms, self.hard_limit_ms)
    }
}

pub struct Tokens {
    pub termination: Option<TerminationToken>,
    pub supervise: CancellationToken,
}

pub struct Arguments {
    pub key: Uuid,
    pub runtime_opts: UserWorkerRuntimeOpts,
    pub cpu_usage_metrics_rx: Option<mpsc::UnboundedReceiver<CPUUsageMetrics>>,
    pub cpu_timer_param: CPUTimerParam,
    pub supervisor_policy: SupervisorPolicy,
    pub timing: Option<Timing>,
    pub memory_limit_rx: mpsc::UnboundedReceiver<()>,
    pub pool_msg_tx: Option<mpsc::UnboundedSender<UserWorkerMsgs>>,
    pub isolate_memory_usage_tx: oneshot::Sender<IsolateMemoryStats>,
    pub isolate_handle_op_sender: IsolateHandleOpSender,
    pub tokens: Tokens,
}

pub struct CPUUsage {
    pub accumulated: i64,
    pub diff: i64,
}

#[derive(EnumAsInner)]
pub enum CPUUsageMetrics {
    Enter(std::thread::ThreadId, CPUTimer),
    Leave(CPUUsage),
}

fn log_if_thread_has_changed(maybe_thread_id_was: Option<ThreadId>, thread_id_now: ThreadId) {
    if let Some(id) = maybe_thread_id_was {
        if id != thread_id_now {
            info!(
                thread_id_was = ?id,
                ?thread_id_now,
                "switching thread"
            );
        }
    }
}
