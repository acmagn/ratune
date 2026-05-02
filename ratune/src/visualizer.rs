use std::collections::VecDeque;

use rustfft::{num_complex::Complex, FftPlanner};

const DEFAULT_FFT_SIZE: usize = 2048;

/// Compute `num_bands` frequency bands from the latest samples in the ring buffer.
///
/// - Applies a Hann window to the most recent FFT_SIZE samples.
/// - Uses logarithmic bin grouping so bass bands are wider than treble bands.
/// - Converts magnitudes to dB, normalises to 0.0–1.0, then applies temporal smoothing.
///
/// Returns a `Vec<f32>` of length `num_bands`.  If the buffer has fewer than 16
/// samples the function returns all-zeros (silence / startup).
pub fn compute_bands(
    samples: &VecDeque<f32>,
    planner: &mut FftPlanner<f32>,
    prev_bands: &[f32],
    num_bands: usize,
    fft_size: Option<usize>,
    gain_db: f32,
) -> Vec<f32> {
    if num_bands == 0 {
        return Vec::new();
    }

    let n = fft_size.unwrap_or(DEFAULT_FFT_SIZE).clamp(256, 4096);
    let available = samples.len().min(n);

    if available < 16 {
        // Not enough samples yet — return silence.
        return vec![0.0; num_bands];
    }

    let start = samples.len() - available;

    // Build Hann-windowed complex input, zero-padded to FFT_SIZE.
    let mut buffer: Vec<Complex<f32>> = (0..n)
        .map(|i| {
            if i < available {
                let s = samples[start + i];
                let window =
                    0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos());
                Complex {
                    re: s * window,
                    im: 0.0,
                }
            } else {
                Complex { re: 0.0, im: 0.0 }
            }
        })
        .collect();

    let fft = planner.plan_fft_forward(n);
    fft.process(&mut buffer);

    // Magnitudes of positive-frequency bins (skip DC bin 0).
    let half = n / 2;
    let magnitudes: Vec<f32> = buffer[1..half]
        .iter()
        .map(|c| (c.re * c.re + c.im * c.im).sqrt())
        .collect();

    let num_mag = magnitudes.len(); // = half - 1 = 1023

    // Logarithmic band grouping: maps from bin 1..num_mag to num_bands bands.
    let log_min = 1.0f32.ln();
    let log_max = (num_mag as f32).ln();

    let raw_bands: Vec<f32> = (0..num_bands)
        .map(|b| {
            let t0 = b as f32 / num_bands as f32;
            let t1 = (b + 1) as f32 / num_bands as f32;
            let bin_start = ((log_min + t0 * (log_max - log_min)).exp() as usize).min(num_mag - 1);
            let bin_end = ((log_min + t1 * (log_max - log_min)).exp() as usize)
                .max(bin_start + 1)
                .min(num_mag);

            let count = (bin_end - bin_start) as f32;
            let sum: f32 = magnitudes[bin_start..bin_end].iter().sum();
            sum / count
        })
        .collect();

    // Normalisation factor: scale magnitude so a full-scale sine gives ~0 dB.
    let scale = 2.0 / n as f32;

    // Convert to dB, apply gain, normalise to 0.0–1.0, smooth.
    raw_bands
        .iter()
        .zip(prev_bands.iter().chain(std::iter::repeat(&0.0f32)))
        .map(|(&m, &prev)| {
            let m_scaled = m * scale;
            let db = if m_scaled > 1e-10 {
                (20.0 * m_scaled.log10()).max(-60.0)
            } else {
                -60.0
            };
            let db = (db + gain_db).max(-60.0);
            let raw = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            prev * 0.7 + raw * 0.3
        })
        .collect()
}
