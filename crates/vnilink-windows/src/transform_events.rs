//! Events of an asynchronous Media Foundation transform, the kind hardware
//! encoders are. Such a transform takes input only after it has sent
//! `METransformNeedInput` and has output only after `METransformHaveOutput`.
//! Events arrive through `BeginGetEvent` on a Media Foundation work queue
//! and are counted here, so the encoding thread waits for one with a
//! deadline instead of blocking in `GetEvent` on a transform that holds a
//! picture back.
//!
//! The pending callback holds the event generator. Shutting the transform
//! down ([`crate::transform::CreatedTransform`] does when dropped) completes
//! it with an error, and the chain ends.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Instant;

use windows::core::{implement, IUnknownImpl, Interface, Ref};
use windows::Win32::Foundation::E_NOTIMPL;
use windows::Win32::Media::MediaFoundation::{
    IMFAsyncCallback, IMFAsyncCallback_Impl, IMFAsyncResult, IMFMediaEventGenerator, IMFTransform,
    METransformHaveOutput, METransformNeedInput,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    NeedInput,
    HaveOutput,
    /// The transform failed or was shut down; no event follows.
    Ended,
}

pub(crate) struct TransformEvents {
    events: Receiver<Event>,
    /// `METransformNeedInput` events not yet answered with an input.
    inputs: u32,
    /// `METransformHaveOutput` events not yet answered with `ProcessOutput`.
    outputs: u32,
    ended: bool,
}

/// What a wait ended with.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Waited {
    Ready,
    TimedOut,
    Ended,
}

impl TransformEvents {
    /// Starts listening to `transform`'s events. Call before the transform
    /// is told to start streaming, so no event is missed.
    pub(crate) unsafe fn listen(transform: &IMFTransform) -> windows::core::Result<Self> {
        let generator: IMFMediaEventGenerator = transform.cast()?;
        let (sender, events) = mpsc::channel();
        let callback: IMFAsyncCallback = Callback {
            generator: generator.clone(),
            sender,
        }
        .into();
        generator.BeginGetEvent(&callback, None)?;
        Ok(Self {
            events,
            inputs: 0,
            outputs: 0,
            ended: false,
        })
    }

    /// Waits until the transform takes an input, and counts it as taken.
    pub(crate) fn take_input(&mut self, deadline: Instant) -> Waited {
        let waited = self.wait(deadline, |events| events.inputs > 0);
        if waited == Waited::Ready {
            self.inputs -= 1;
        }
        waited
    }

    /// Waits until the transform has an output, and counts it as drained.
    pub(crate) fn take_output(&mut self, deadline: Instant) -> Waited {
        let waited = self.wait(deadline, |events| events.outputs > 0);
        if waited == Waited::Ready {
            self.outputs -= 1;
        }
        waited
    }

    fn wait(&mut self, deadline: Instant, ready: impl Fn(&Self) -> bool) -> Waited {
        loop {
            while let Ok(event) = self.events.try_recv() {
                self.count(event);
            }
            if ready(self) {
                return Waited::Ready;
            }
            if self.ended {
                return Waited::Ended;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) => self.count(event),
                Err(RecvTimeoutError::Timeout) => return Waited::TimedOut,
                Err(RecvTimeoutError::Disconnected) => self.ended = true,
            }
        }
    }

    fn count(&mut self, event: Event) {
        match event {
            Event::NeedInput => self.inputs += 1,
            Event::HaveOutput => self.outputs += 1,
            Event::Ended => self.ended = true,
        }
    }
}

#[implement(IMFAsyncCallback)]
struct Callback {
    generator: IMFMediaEventGenerator,
    sender: Sender<Event>,
}

impl IMFAsyncCallback_Impl for Callback_Impl {
    fn GetParameters(&self, _: *mut u32, _: *mut u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Invoke(&self, result: Ref<'_, IMFAsyncResult>) -> windows::core::Result<()> {
        // SAFETY: `result` is the one this generator's `BeginGetEvent`
        // completed with.
        let event = unsafe { self.generator.EndGetEvent(result.as_ref()) };
        let kind = match &event {
            Ok(event) => match unsafe { (event.GetType(), event.GetStatus()) } {
                (_, Ok(status)) if status.is_err() => Event::Ended,
                (Ok(kind), _) if kind == METransformNeedInput.0 as u32 => Event::NeedInput,
                (Ok(kind), _) if kind == METransformHaveOutput.0 as u32 => Event::HaveOutput,
                // Other events (drain and marker completions) are not used.
                (Ok(_), _) => {
                    return self.listen_again();
                }
                (Err(_), _) => Event::Ended,
            },
            Err(_) => Event::Ended,
        };
        if self.sender.send(kind).is_err() || kind == Event::Ended {
            return Ok(());
        }
        self.listen_again()
    }
}

impl Callback_Impl {
    fn listen_again(&self) -> windows::core::Result<()> {
        let callback: IMFAsyncCallback = self.to_interface();
        // SAFETY: the generator is live while this callback is referenced.
        if unsafe { self.generator.BeginGetEvent(&callback, None) }.is_err() {
            let _ = self.sender.send(Event::Ended);
        }
        Ok(())
    }
}
