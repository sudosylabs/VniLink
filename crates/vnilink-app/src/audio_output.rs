// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Playback endpoint ownership. Following the OS default never replaces the remote service.
use std::time::{Duration, Instant};
use vnilink::Error;
use vnilink_audio::Format;

pub(crate) const DEFAULT_OUTPUT: &str = "vnilink:default-output";
const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_millis(500);
const OUTPUT_RECOVERY_LIMIT: Duration = Duration::from_secs(1);

pub(crate) trait OutputDevice {
    fn push(&mut self, samples: &[f32]) -> Result<(), Error>;
    /// How the device is keeping up, where it can say.
    fn health(&self) -> Option<vnilink_audio::OutputHealth> {
        None
    }
}
pub(crate) trait OutputBackend {
    type Device: OutputDevice;
    fn default_id(&mut self) -> Result<String, Error>;
    fn open(&mut self, id: &str, format: Format) -> Result<Self::Device, Error>;
}
pub(crate) struct NativeOutputBackend;
impl OutputDevice for vnilink_audio::NativeOutput {
    fn push(&mut self, samples: &[f32]) -> Result<(), Error> {
        vnilink_audio::NativeOutput::push(self, samples)
            .map_err(|_| Error::Unsupported("native audio output ended"))
    }
    fn health(&self) -> Option<vnilink_audio::OutputHealth> {
        Some(vnilink_audio::NativeOutput::health(self))
    }
}
impl OutputBackend for NativeOutputBackend {
    type Device = vnilink_audio::NativeOutput;
    fn default_id(&mut self) -> Result<String, Error> {
        vnilink_audio::default_device_id(vnilink_audio::DeviceDirection::Output)
            .map_err(|_| Error::Unsupported("default audio output"))?
            .ok_or(Error::Unsupported("default audio output"))
    }
    fn open(&mut self, id: &str, format: Format) -> Result<Self::Device, Error> {
        vnilink_audio::NativeOutput::open(id, format)
            .map_err(|_| Error::Unsupported("native audio playback"))
    }
}

pub(crate) struct PlaybackOutput<B: OutputBackend = NativeOutputBackend> {
    backend: B,
    output: Option<B::Device>,
    selected_id: String,
    follows_default: bool,
    format: Format,
    checked: Instant,
}
impl<B: OutputBackend> PlaybackOutput<B> {
    pub(crate) fn open(mut backend: B, id: &str, format: Format) -> Result<Self, Error> {
        let follows_default = id == DEFAULT_OUTPUT;
        let selected_id = if follows_default {
            backend.default_id()?
        } else {
            id.into()
        };
        let output = backend.open(&selected_id, format)?;
        Ok(Self {
            backend,
            output: Some(output),
            selected_id,
            follows_default,
            format,
            checked: Instant::now(),
        })
    }
    /// How the open output is keeping up, where it can say.
    pub(crate) fn health(&self) -> Option<vnilink_audio::OutputHealth> {
        self.output.as_ref().and_then(OutputDevice::health)
    }
    pub(crate) fn push(
        &mut self,
        samples: &[f32],
        cancelled: impl Fn() -> bool,
    ) -> Result<(), Error> {
        if cancelled() {
            return Err(Error::NoSession);
        }
        let result = (|| {
            if self.follows_default && self.checked.elapsed() >= DEFAULT_CHECK_INTERVAL {
                let next_id = self.backend.default_id()?;
                if next_id != self.selected_id {
                    let output = self.backend.open(&next_id, self.format)?;
                    self.output = Some(output);
                    self.selected_id = next_id;
                }
                self.checked = Instant::now();
            }
            self.output.as_mut().ok_or(Error::NoSession)?.push(samples)
        })();
        match result {
            Err(error) if self.follows_default => self.recover(samples, error, cancelled),
            other => other,
        }
    }
    fn recover(
        &mut self,
        samples: &[f32],
        mut error: Error,
        cancelled: impl Fn() -> bool,
    ) -> Result<(), Error> {
        // A removed endpoint can fail before the OS publishes its replacement.
        // Bound this wait and keep cancellation responsive; explicit endpoints never retry.
        self.output.take();
        let until = Instant::now() + OUTPUT_RECOVERY_LIMIT;
        loop {
            if cancelled() {
                return Err(Error::NoSession);
            }
            if let Ok(next_id) = self.backend.default_id() {
                if next_id != self.selected_id {
                    match self.backend.open(&next_id, self.format) {
                        Ok(mut output) => {
                            if cancelled() {
                                return Err(Error::NoSession);
                            }
                            output.push(samples)?;
                            self.output = Some(output);
                            self.selected_id = next_id;
                            self.checked = Instant::now();
                            return Ok(());
                        }
                        Err(next_error) => error = next_error,
                    }
                }
            }
            if Instant::now() >= until {
                return Err(error);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct State {
        default: String,
        failed: bool,
        opened: Vec<String>,
        played: Vec<String>,
    }
    struct Backend(Arc<Mutex<State>>);
    struct Device {
        state: Arc<Mutex<State>>,
        id: String,
    }
    impl OutputDevice for Device {
        fn push(&mut self, _: &[f32]) -> Result<(), Error> {
            let mut state = self.state.lock().unwrap();
            if self.id == "headset" && state.failed {
                return Err(Error::Unsupported("unplugged"));
            }
            state.played.push(self.id.clone());
            Ok(())
        }
    }
    impl OutputBackend for Backend {
        type Device = Device;
        fn default_id(&mut self) -> Result<String, Error> {
            Ok(self.0.lock().unwrap().default.clone())
        }
        fn open(&mut self, id: &str, _: Format) -> Result<Device, Error> {
            self.0.lock().unwrap().opened.push(id.into());
            Ok(Device {
                state: self.0.clone(),
                id: id.into(),
            })
        }
    }
    fn state() -> Arc<Mutex<State>> {
        Arc::new(Mutex::new(State {
            default: "headset".into(),
            ..Default::default()
        }))
    }
    #[test]
    fn unplug_follows_replacement_before_periodic_check_without_reopening_remote_audio() {
        let state = state();
        let mut output =
            PlaybackOutput::open(Backend(state.clone()), DEFAULT_OUTPUT, Format::Stereo).unwrap();
        output.push(&[0.0; 1920], || false).unwrap();
        {
            let mut state = state.lock().unwrap();
            state.failed = true;
            state.default = "speakers".into();
        }
        output.push(&[0.0; 1920], || false).unwrap();
        let state = state.lock().unwrap();
        assert_eq!(state.opened, ["headset", "speakers"]);
        assert_eq!(state.played, ["headset", "speakers"]);
    }
    #[test]
    fn explicit_output_failure_never_uses_the_new_default() {
        let state = state();
        let mut output =
            PlaybackOutput::open(Backend(state.clone()), "headset", Format::Stereo).unwrap();
        {
            let mut state = state.lock().unwrap();
            state.failed = true;
            state.default = "speakers".into();
        }
        assert!(output.push(&[], || false).is_err());
        assert_eq!(state.lock().unwrap().opened, ["headset"]);
    }
    #[test]
    fn permission_revocation_interrupts_output_recovery_before_reopening_a_device() {
        let state = state();
        let mut output =
            PlaybackOutput::open(Backend(state.clone()), DEFAULT_OUTPUT, Format::Stereo).unwrap();
        state.lock().unwrap().failed = true;
        let checks = std::cell::Cell::new(0);
        assert!(matches!(
            output.push(&[], || {
                checks.set(checks.get() + 1);
                checks.get() > 1
            }),
            Err(Error::NoSession)
        ));
        assert_eq!(state.lock().unwrap().opened, ["headset"]);
    }
}
