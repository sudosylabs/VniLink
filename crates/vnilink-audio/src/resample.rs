use crate::{Error, Format};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// Band-limited streaming resampler. Fixed input chunks; callers retain only
/// a bounded remainder. Ratio correction is limited to the pacing's
/// [`crate::pace::MAX_PPM`] and ramped.
pub struct AudioResampler {
    inner: SincFixedIn<f32>,
    format: Format,
    chunk: usize,
}
impl AudioResampler {
    pub fn new(
        input_rate: u32,
        output_rate: u32,
        format: Format,
        chunk: usize,
    ) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&input_rate)
            || !(8_000..=192_000).contains(&output_rate)
            || chunk == 0
            || chunk > 3840
        {
            return Err(Error::InvalidFormat);
        }
        let inner = SincFixedIn::new(
            output_rate as f64 / input_rate as f64,
            1.0 + crate::pace::MAX_PPM / 1_000_000.0,
            SincInterpolationParameters {
                sinc_len: 128,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Cubic,
                oversampling_factor: 128,
                window: WindowFunction::BlackmanHarris2,
            },
            chunk,
            format.channels(),
        )
        .map_err(|_| Error::Resample)?;
        Ok(Self {
            inner,
            format,
            chunk,
        })
    }
    pub fn input_samples(&self) -> usize {
        self.chunk * self.format.channels()
    }
    pub fn delay_samples(&self) -> usize {
        self.inner.output_delay()
    }
    pub fn set_drift_ppm(&mut self, ppm: f64) -> Result<(), Error> {
        if !ppm.is_finite() || ppm.abs() > crate::pace::MAX_PPM {
            return Err(Error::InvalidFormat);
        }
        self.inner
            .set_resample_ratio_relative(1.0 + ppm / 1_000_000.0, true)
            .map_err(|_| Error::Resample)
    }
    pub fn process(&mut self, interleaved: &[f32]) -> Result<Vec<f32>, Error> {
        if interleaved.len() != self.input_samples() || interleaved.iter().any(|v| !v.is_finite()) {
            return Err(Error::InvalidFormat);
        }
        let channels: Vec<Vec<f32>> = (0..self.format.channels())
            .map(|c| {
                interleaved
                    .chunks_exact(self.format.channels())
                    .map(|frame| frame[c].clamp(-1.0, 1.0))
                    .collect()
            })
            .collect();
        let out = self
            .inner
            .process(&channels, None)
            .map_err(|_| Error::Resample)?;
        let mut samples = Vec::with_capacity(out[0].len() * self.format.channels());
        for i in 0..out[0].len() {
            for channel in &out {
                samples.push(channel[i].clamp(-1.0, 1.0));
            }
        }
        Ok(samples)
    }
}
