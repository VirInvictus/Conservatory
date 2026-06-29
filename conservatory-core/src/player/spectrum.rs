//! Pure spectrum DSP (Phase 12d): turn a window of PCM samples into log-spaced
//! frequency-band magnitudes for the visualizer. libmpv exposes no PCM tap, so
//! the PipeWire capture that feeds this and the GTK widget that draws it live in
//! the binary; the FFT + binning live here so they are unit-testable headless
//! (the spec §16.13 "logic in core" rule).
//!
//! `SpectrumAnalyzer` holds the `realfft` plan and scratch buffers (re-planning
//! per audio frame would be wasteful), turning a mono window into `n_bands`
//! normalized 0..=1 levels on a log frequency scale. `SpectrumSmoother` gives the
//! bars a fast attack / slow decay so they rise sharply and fall smoothly.

use std::f32::consts::PI;
use std::sync::Arc;

use realfft::RealFftPlanner;
use realfft::RealToComplex;
use realfft::num_complex::Complex;

/// The analysis window length. ~85 ms at 48 kHz (≈12 Hz/bin): the finer bin
/// resolution lets the many thin spectrum lines resolve distinct frequencies
/// down low without collapsing onto each other.
pub const FFT_SIZE: usize = 4096;

/// The lowest band edge (Hz); below this is sub-bass rumble not worth a bar.
const F_MIN: f32 = 32.0;
/// The dB window mapped onto 0..=1: `FLOOR` reads as an empty bar, `CEIL` as full.
const DB_FLOOR: f32 = -70.0;
const DB_CEIL: f32 = -10.0;

/// Turns mono PCM windows into `n_bands` log-spaced, normalized band levels.
/// Holds the FFT plan + scratch so the hot path allocates nothing per frame.
pub struct SpectrumAnalyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    scratch_in: Vec<f32>,
    scratch_out: Vec<Complex<f32>>,
    /// The half-open FFT-bin range `[lo, hi)` feeding each output band.
    band_bins: Vec<(usize, usize)>,
    bands: Vec<f32>,
    sample_rate: u32,
}

impl SpectrumAnalyzer {
    /// Build an analyzer producing `n_bands` bars for audio at `sample_rate`.
    pub fn new(n_bands: usize, sample_rate: u32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let scratch_in = fft.make_input_vec();
        let scratch_out = fft.make_output_vec();
        let window = hann_window(FFT_SIZE);
        let band_bins = log_band_bins(n_bands, sample_rate, scratch_out.len());
        Self {
            fft,
            window,
            scratch_in,
            scratch_out,
            band_bins,
            bands: vec![0.0; n_bands],
            sample_rate,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn n_bands(&self) -> usize {
        self.bands.len()
    }

    /// Analyze the most recent window of mono `samples`, returning `n_bands`
    /// levels in 0..=1 (one per log-spaced frequency band). A window shorter than
    /// [`FFT_SIZE`] is zero-padded; only the last `FFT_SIZE` samples are used.
    pub fn analyze(&mut self, samples: &[f32]) -> &[f32] {
        // Copy the tail of the window in, applying the Hann taper; zero-pad the
        // front when we have fewer than FFT_SIZE samples.
        let take = samples.len().min(FFT_SIZE);
        let pad = FFT_SIZE - take;
        let src = &samples[samples.len() - take..];
        for v in self.scratch_in[..pad].iter_mut() {
            *v = 0.0;
        }
        for (i, &s) in src.iter().enumerate() {
            self.scratch_in[pad + i] = s * self.window[pad + i];
        }

        // `process` only fails on a wrong-length buffer, which we control.
        if self
            .fft
            .process(&mut self.scratch_in, &mut self.scratch_out)
            .is_err()
        {
            self.bands.iter_mut().for_each(|b| *b = 0.0);
            return &self.bands;
        }

        // Each band's level is the peak magnitude over its bins, normalized from a
        // dB window to 0..=1. Peak (not mean) keeps a single strong tone visible
        // even when its band spans many quiet bins.
        let norm = 2.0 / FFT_SIZE as f32;
        for (band, &(lo, hi)) in self.bands.iter_mut().zip(self.band_bins.iter()) {
            let mut peak = 0.0f32;
            for c in &self.scratch_out[lo..hi] {
                peak = peak.max(c.norm() * norm);
            }
            *band = normalize_db(peak);
        }
        &self.bands
    }
}

/// Per-band fast-attack / slow-decay smoothing so bars snap up and fall gently.
pub struct SpectrumSmoother {
    levels: Vec<f32>,
    attack: f32,
    decay: f32,
}

impl SpectrumSmoother {
    /// `attack` / `decay` are per-update blend factors in 0..=1 (1.0 = instant).
    /// A high attack and low decay give the classic "peak jumps, tail falls" look.
    pub fn new(n_bands: usize, attack: f32, decay: f32) -> Self {
        Self {
            levels: vec![0.0; n_bands],
            attack: attack.clamp(0.0, 1.0),
            decay: decay.clamp(0.0, 1.0),
        }
    }

    /// Blend `target` into the held levels (rising fast, falling slow) and return
    /// the smoothed result.
    pub fn update(&mut self, target: &[f32]) -> &[f32] {
        for (cur, &t) in self.levels.iter_mut().zip(target.iter()) {
            let rate = if t > *cur { self.attack } else { self.decay };
            *cur += (t - *cur) * rate;
        }
        &self.levels
    }

    pub fn levels(&self) -> &[f32] {
        &self.levels
    }
}

/// A periodic Hann window of length `n`, reducing spectral leakage before the FFT.
fn hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / n as f32).cos()))
        .collect()
}

/// The half-open FFT-bin range for each of `n_bands` log-spaced bands spanning
/// `F_MIN`..Nyquist. `n_bins` is the real-FFT output length (`FFT_SIZE/2 + 1`).
/// Every band gets at least one bin (low bands otherwise collapse to empty).
fn log_band_bins(n_bands: usize, sample_rate: u32, n_bins: usize) -> Vec<(usize, usize)> {
    let nyquist = sample_rate as f32 / 2.0;
    let f_max = nyquist.clamp(F_MIN * 2.0, 20_000.0);
    let bin_hz = nyquist / (n_bins - 1) as f32;
    let ratio = (f_max / F_MIN).powf(1.0 / n_bands as f32);

    let mut edges = Vec::with_capacity(n_bands + 1);
    let mut f = F_MIN;
    for _ in 0..=n_bands {
        let bin = (f / bin_hz).round() as usize;
        edges.push(bin.min(n_bins - 1));
        f *= ratio;
    }

    (0..n_bands)
        .map(|i| {
            let lo = edges[i];
            // Ensure a non-empty, in-range span even where the log edges collide.
            let hi = edges[i + 1].clamp(lo + 1, n_bins);
            (lo.min(n_bins - 1), hi)
        })
        .collect()
}

/// Map a linear magnitude to 0..=1 over the `DB_FLOOR`..`DB_CEIL` dB window.
fn normalize_db(mag: f32) -> f32 {
    if mag <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * mag.log10();
    ((db - DB_FLOOR) / (DB_CEIL - DB_FLOOR)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono sine of `freq` Hz, `n` samples at `sample_rate`, amplitude 0.8.
    fn sine(freq: f32, n: usize, sample_rate: u32) -> Vec<f32> {
        (0..n)
            .map(|i| 0.8 * (2.0 * PI * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    fn argmax(levels: &[f32]) -> usize {
        levels
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap()
    }

    #[test]
    fn sine_concentrates_in_its_band() {
        let sr = 48_000;
        let n_bands = 24;
        let mut a = SpectrumAnalyzer::new(n_bands, sr);
        // A 1 kHz tone should light up the band whose range contains 1 kHz and
        // dominate the others.
        let bands = a.analyze(&sine(1000.0, FFT_SIZE, sr)).to_vec();
        let peak = argmax(&bands);
        let (lo, hi) = a.band_bins[peak];
        let bin_hz = (sr as f32 / 2.0) / (a.scratch_out.len() - 1) as f32;
        let lo_hz = lo as f32 * bin_hz;
        let hi_hz = hi as f32 * bin_hz;
        assert!(
            (lo_hz..=hi_hz).contains(&1000.0) || (lo_hz <= 1100.0 && hi_hz >= 900.0),
            "peak band {peak} spans {lo_hz:.0}..{hi_hz:.0} Hz, expected to contain 1 kHz"
        );
        assert!(bands[peak] > 0.5, "peak band weak: {}", bands[peak]);
        // A distant low band should be far quieter than the peak.
        assert!(bands[0] < bands[peak] * 0.5, "low band not suppressed");
    }

    #[test]
    fn silence_is_all_zero() {
        let mut a = SpectrumAnalyzer::new(16, 48_000);
        let bands = a.analyze(&vec![0.0; FFT_SIZE]);
        assert!(bands.iter().all(|&b| b == 0.0), "silence should be flat");
    }

    #[test]
    fn short_window_is_padded_not_panicked() {
        let mut a = SpectrumAnalyzer::new(16, 48_000);
        // Far fewer than FFT_SIZE samples must not panic.
        let bands = a.analyze(&sine(2000.0, 256, 48_000));
        assert_eq!(bands.len(), 16);
    }

    #[test]
    fn smoother_rises_fast_and_decays_slow() {
        let mut s = SpectrumSmoother::new(1, 0.8, 0.2);
        // One loud frame, then silence.
        let after_rise = s.update(&[1.0])[0];
        assert!(after_rise > 0.5, "attack too slow: {after_rise}");
        let mut prev = after_rise;
        // Feeding zeros, the level falls monotonically toward 0 but not instantly.
        for _ in 0..5 {
            let v = s.update(&[0.0])[0];
            assert!(v < prev, "decay not monotonic: {v} !< {prev}");
            assert!(v > 0.0, "decay overshot to {v}");
            prev = v;
        }
    }

    #[test]
    fn every_band_has_a_nonempty_bin_range() {
        // Even at many bands the low end must not collapse to empty spans.
        let bins = log_band_bins(48, 48_000, FFT_SIZE / 2 + 1);
        assert_eq!(bins.len(), 48);
        assert!(bins.iter().all(|&(lo, hi)| hi > lo), "empty band span");
    }
}
