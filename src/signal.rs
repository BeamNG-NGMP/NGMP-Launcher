use std::sync::{Arc, Weak};

#[derive(Clone)]
pub struct SignalReceiver {
    weak: Weak<()>,
}

impl SignalReceiver {
    pub fn alive(&self) -> bool {
        self.weak.upgrade().is_some()
    }
}

pub struct Signal {
    inner: Arc<()>,
}

impl Signal {
    pub fn new() -> (Self, SignalReceiver) {
        let inner = Arc::default();
        let weak = Arc::downgrade(&inner);
        (Self {
            inner,
        }, SignalReceiver {
            weak,
        })
    }

    pub fn trigger(self) {
        drop(self);
    }
}

impl Drop for Signal {
    fn drop(&mut self) {
        trace!("Signal dropped.");
    }
}
