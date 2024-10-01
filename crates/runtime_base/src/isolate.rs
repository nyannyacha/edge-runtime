use deno_core::v8;
use tokio::sync::mpsc;

pub struct RequestInterruptPayload(Box<dyn FnOnce(&mut v8::Isolate) + Send>);

pub enum IsolateHandleOp {
    TerminateExecution,
    CancelTerminateExecution,
    RequestInterrupt(RequestInterruptPayload),
}

impl IsolateHandleOp {
    pub fn dispatch(self, isolate: &mut v8::Isolate) {
        match self {
            IsolateHandleOp::TerminateExecution => {
                isolate.terminate_execution();
            }
            IsolateHandleOp::CancelTerminateExecution => {
                isolate.cancel_terminate_execution();
            }
            IsolateHandleOp::RequestInterrupt(payload) => {
                extern "C" fn handle_interrupt(
                    isolate: &mut v8::Isolate,
                    cb: *mut std::ffi::c_void,
                ) {
                    unsafe { Box::from_raw(cb as *mut RequestInterruptPayload).0(isolate) };
                }

                let data_ptr_mut = Box::into_raw(Box::new(payload));

                if !isolate
                    .thread_safe_handle()
                    .request_interrupt(handle_interrupt, data_ptr_mut as *mut std::ffi::c_void)
                {
                    drop(unsafe { Box::from_raw(data_ptr_mut) })
                }
            }
        }
    }
}

pub struct IsolateHandleOpQueue {
    pub tx: mpsc::UnboundedSender<IsolateHandleOp>,
    pub rx: mpsc::UnboundedReceiver<IsolateHandleOp>,
}

impl Default for IsolateHandleOpQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl IsolateHandleOpQueue {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self { tx, rx }
    }

    pub fn sender(&self) -> IsolateHandleOpSender {
        IsolateHandleOpSender {
            tx: self.tx.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IsolateHandleOpSender {
    tx: mpsc::UnboundedSender<IsolateHandleOp>,
}

impl IsolateHandleOpSender {
    #[inline(always)]
    pub fn terminate_execution(&self) -> bool {
        self.tx.send(IsolateHandleOp::TerminateExecution).is_ok()
    }

    pub fn cancel_terminate_execution(&self) -> bool {
        self.tx
            .send(IsolateHandleOp::CancelTerminateExecution)
            .is_ok()
    }

    pub fn request_interrupt<F>(&self, func: F) -> bool
    where
        F: FnOnce(&mut v8::Isolate) + Send + 'static,
    {
        self.tx
            .send(IsolateHandleOp::RequestInterrupt(RequestInterruptPayload(
                Box::new(func),
            )))
            .is_ok()
    }
}
