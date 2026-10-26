//! Relative motion and high-resolution scroll. Absolute positions are not scaled.

use super::{MAX_PRECISE_WHEEL, WHEEL_DETENT};

/// Adds one precise-scroll sample to a remainder and returns whole detents.
///
/// 120 precise units are one detent. A remainder smaller than a detent is kept
/// so later samples are not dropped. The remainder keeps the sign of the sum.
pub(crate) fn accumulate_detents(pending: &mut i32, delta: i32) -> i32 {
    let delta = delta.clamp(-MAX_PRECISE_WHEEL, MAX_PRECISE_WHEEL);
    *pending = pending.saturating_add(delta);
    let detents = *pending / WHEEL_DETENT;
    *pending -= detents * WHEEL_DETENT;
    detents
}

/// Controller-local pointer policy. Each Desktop owner has its own copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerSettings {
    pub sensitivity_percent: u16,
    pub reverse_wheel: bool,
    pub swap_buttons: bool,
}
impl Default for PointerSettings {
    fn default() -> Self {
        Self {
            sensitivity_percent: super::DEFAULT_POINTER_SENSITIVITY,
            reverse_wheel: false,
            swap_buttons: false,
        }
    }
}
impl PointerSettings {
    pub(crate) fn validate(self) -> Result<(), crate::Error> {
        if !(super::MIN_POINTER_SENSITIVITY..=super::MAX_POINTER_SENSITIVITY)
            .contains(&self.sensitivity_percent)
        {
            return Err(crate::Error::InvalidSetting(
                "pointer sensitivity must be between 1 and 400 percent",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(crate) struct PointerPlan {
    pub(crate) settings: PointerSettings,
    remainder: (i32, i32),
}
impl PointerPlan {
    pub(crate) fn with_settings(settings: PointerSettings) -> Self {
        Self {
            settings,
            remainder: (0, 0),
        }
    }
    pub(crate) fn reset(&mut self) {
        self.remainder = (0, 0);
    }
    pub(crate) fn set(
        &mut self,
        settings: PointerSettings,
        buttons_held: bool,
    ) -> Result<(), crate::Error> {
        settings.validate()?;
        if buttons_held && settings.swap_buttons != self.settings.swap_buttons {
            return Err(crate::Error::InvalidSetting(
                "release held buttons before changing the pointer swap",
            ));
        }
        self.settings = settings;
        self.reset();
        Ok(())
    }
    pub(crate) fn apply(
        &mut self,
        event: super::InputEvent,
    ) -> Result<Option<super::InputEvent>, crate::Error> {
        use super::InputEvent;
        event.validate()?;
        let event = if self.settings.swap_buttons {
            super::swap_pointer_button(event)
        } else {
            event
        };
        Ok(Some(match event {
            InputEvent::Motion { dx, dy } => {
                // Retain sub-counts at low sensitivity. Saturate only the
                // wire-sized result, without retaining overflow as backlog.
                let scale = |delta: i16, remainder: &mut i32| {
                    let total = i32::from(delta) * i32::from(self.settings.sensitivity_percent)
                        + *remainder;
                    *remainder = total % 100;
                    (total / 100).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
                };
                let dx = scale(dx, &mut self.remainder.0);
                let dy = scale(dy, &mut self.remainder.1);
                if dx == 0 && dy == 0 {
                    return Ok(None);
                }
                InputEvent::motion(dx, dy)
            }
            InputEvent::Wheel { dx, dy } if self.settings.reverse_wheel => {
                InputEvent::wheel(dx.saturating_neg(), dy.saturating_neg())
            }
            InputEvent::PreciseWheel { dx, dy } if self.settings.reverse_wheel => {
                InputEvent::precise_wheel(-dx, -dy)
            }
            other => other,
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::input::InputEvent;

    #[test]
    fn pointer_motion_keeps_sub_counts_and_never_scales_absolute_positions() {
        use crate::input::{PointerPlan, PointerSettings};
        let mut plan = PointerPlan::default();
        plan.set(
            PointerSettings {
                sensitivity_percent: 1,
                ..PointerSettings::default()
            },
            false,
        )
        .unwrap();
        for _ in 0..99 {
            assert_eq!(plan.apply(InputEvent::motion(1, -1)).unwrap(), None);
        }
        assert_eq!(
            plan.apply(InputEvent::motion(1, -1)).unwrap(),
            Some(InputEvent::motion(1, -1))
        );
        assert_eq!(
            plan.apply(InputEvent::pointer(-20, 30, 1, 1)).unwrap(),
            Some(InputEvent::pointer(-20, 30, 1, 1))
        );
        assert!(plan
            .set(
                PointerSettings {
                    sensitivity_percent: 0,
                    ..PointerSettings::default()
                },
                false
            )
            .is_err());
        plan.set(
            PointerSettings {
                sensitivity_percent: 400,
                ..PointerSettings::default()
            },
            false,
        )
        .unwrap();
        assert_eq!(
            plan.apply(InputEvent::motion(i16::MAX, i16::MIN)).unwrap(),
            Some(InputEvent::motion(i16::MAX, i16::MIN))
        );
        let mut pending = 0;
        for _ in 0..7 {
            assert_eq!(crate::input::accumulate_detents(&mut pending, 15), 0);
        }
        assert_eq!(crate::input::accumulate_detents(&mut pending, 15), 1);
        assert_eq!(pending, 0);
        assert_eq!(crate::input::accumulate_detents(&mut pending, -119), 0);
        assert_eq!(crate::input::accumulate_detents(&mut pending, -1), -1);
    }
}
