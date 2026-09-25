//! Streaming sample-rate conversion to 48 kHz for mono speech.
//!
//! Cubic (Catmull-Rom) interpolation, with a moving-average pre-filter when
//! downsampling by more than 1.5x. Plenty for transcription; most devices
//! already deliver 48 kHz (and Windows converts for us), so this mostly serves
//! Bluetooth headset microphones (16/24 kHz) and 44.1 kHz devices.

pub struct Resampler {
    step: f64,
    /// Pending input; `pos` is relative to `buf[1]`.
    buf: Vec<f32>,
    pos: f64,
    box_len: usize,
    box_hist: Vec<f32>,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        let ratio = f64::from(from) / f64::from(to);
        let box_len = if ratio > 1.5 { ratio.round() as usize } else { 1 };
        Resampler { step: ratio, buf: vec![0.0], pos: 0.0, box_len, box_hist: Vec::new() }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.box_len > 1 {
            for &x in input {
                self.box_hist.push(x);
                if self.box_hist.len() > self.box_len {
                    self.box_hist.remove(0);
                }
                let avg = self.box_hist.iter().sum::<f32>() / self.box_hist.len() as f32;
                self.buf.push(avg);
            }
        } else {
            self.buf.extend_from_slice(input);
        }
        loop {
            let i = self.pos.floor() as usize;
            if i + 3 > self.buf.len() {
                break;
            }
            let f = (self.pos - i as f64) as f32;
            let y0 = self.buf[i];
            let y1 = self.buf[i + 1];
            let y2 = self.buf[i + 2];
            let y3 = if i + 3 < self.buf.len() { self.buf[i + 3] } else { y2 };
            out.push(catmull_rom(y0, y1, y2, y3, f));
            self.pos += self.step;
        }
        let consumed = (self.pos.floor() as usize).min(self.buf.len().saturating_sub(1));
        self.buf.drain(..consumed);
        self.pos -= consumed as f64;
    }
}

fn catmull_rom(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * t + b) * t + c) * t + y1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, hz: f32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize).map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin()).collect()
    }

    fn zero_crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
    }

    #[test]
    fn keeps_length_and_pitch() {
        for from in [16_000u32, 24_000, 44_100, 96_000] {
            let input = tone(from, 300.0, 2.0);
            let mut r = Resampler::new(from, 48_000);
            let mut out = Vec::new();
            for chunk in input.chunks(333) {
                r.process(chunk, &mut out);
            }
            let expected = 96_000.0;
            assert!((out.len() as f64 - expected).abs() < 10.0, "{from}: {} samples", out.len());
            // 300 Hz for 2 s: ~1200 zero crossings.
            let zc = zero_crossings(&out) as i64;
            assert!((zc - 1200).abs() <= 4, "{from}: {zc} crossings");
            let peak = out[1000..].iter().fold(0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.9 && peak < 1.1, "{from}: peak {peak}");
        }
    }
}
