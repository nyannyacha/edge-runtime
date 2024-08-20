pub mod strategy_per_request;
pub mod strategy_per_worker;

use std::sync::Arc;

use anyhow::Error;
use cpu_timer::CPUTimer;
use deno_core::v8::IsolateHandle;
use enum_as_inner::EnumAsInner;
use futures_util::task::AtomicWaker;
use log::error;
use sb_workers::context::{Timing, UserWorkerMsgs, UserWorkerRuntimeOpts};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{worker_ctx::TerminationToken, worker_pool::SupervisorPolicy};

#[repr(C)]
pub struct IsolateInterruptData {
    pub should_terminate: bool,
    pub isolate_memory_usage_tx: Option<oneshot::Sender<IsolateMemoryStats>>,
}

pub extern "C" fn handle_interrupt(
    isolate: &mut deno_core::v8::Isolate,
    data: *mut std::ffi::c_void,
) {
    let mut boxed_data: Box<IsolateInterruptData>;

    unsafe {
        boxed_data = Box::from_raw(data as *mut IsolateInterruptData);
    }

    // log memory usage
    let mut heap_stats = deno_core::v8::HeapStatistics::default();

    isolate.get_heap_statistics(&mut heap_stats);

    let usage = IsolateMemoryStats {
        used_heap_size: heap_stats.used_heap_size(),
        external_memory: heap_stats.external_memory(),
    };

    if let Some(usage_tx) = boxed_data.isolate_memory_usage_tx.take() {
        if usage_tx.send(usage).is_err() {
            error!("failed to send isolate memory usage - receiver may have been dropped");
        }
    }

    if boxed_data.should_terminate {
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
    pub thread_safe_handle: IsolateHandle,
    pub waker: Arc<AtomicWaker>,
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
