//! Encodes mixed audio to Opus and writes it with [`WebmWriter`].

use std::path::Path;

use crate::error::Error;
use crate::webm::{Finished, WebmWriter};

/// 20 ms at 48 kHz: one Opus packet.
pub const FRAME: usize = 960;
const PACKET_MS: u64 = 20;
/// Plenty for speech on two independent channels; ~43 MB per hour, so the
/// server's 200 MB limit is about 4.5 hours.
pub const BITRATE_STEREO: i32 = 96_000;
pub const BITRATE_MONO: i32 = 48_000;

pub struct OpusFile {
    enc: Encoder,
    writer: WebmWriter,
    channels: usize,
    pending: Vec<f32>,
    packets: u64,
}

impl OpusFile {
    pub fn create(path: &Path, channels: u8, writing_app: &str) -> Result<Self, Error> {
        let mut enc = Encoder::new(channels)?;
        let pre_skip = enc.lookahead()?;
        let writer = WebmWriter::create(path, channels, pre_skip, PACKET_MS, writing_app)?;
        Ok(OpusFile { enc, writer, channels: channels as usize, pending: Vec::with_capacity(FRAME * 4), packets: 0 })
    }

    /// Add interleaved samples (any length).
    pub fn push(&mut self, interleaved: &[f32]) -> Result<(), Error> {
        self.pending.extend_from_slice(interleaved);
        let frame_len = FRAME * self.channels;
        let mut offset = 0;
        while self.pending.len() - offset >= frame_len {
            let packet = self.enc.encode(&self.pending[offset..offset + frame_len])?;
            self.writer.write_packet(self.packets * PACKET_MS, &packet);
            self.packets += 1;
            offset += frame_len;
        }
        self.pending.drain(..offset);
        Ok(())
    }

    pub fn disk_error(&self) -> Option<&str> {
        self.writer.disk_error()
    }

    pub fn duration_ms(&self) -> u64 {
        self.writer.duration_ms()
    }

    /// Encode what is left (padded with silence) and close the file.
    pub fn finish(mut self) -> Result<Finished, Error> {
        if !self.pending.is_empty() {
            let frame_len = FRAME * self.channels;
            let mut last = std::mem::take(&mut self.pending);
            last.resize(frame_len, 0.0);
            let packet = self.enc.encode(&last)?;
            self.writer.write_packet(self.packets * PACKET_MS, &packet);
        }
        Ok(self.writer.finish())
    }
}

#[cfg(feature = "opus")]
struct Encoder(opus::Encoder, Vec<u8>);

#[cfg(feature = "opus")]
impl Encoder {
    fn new(channels: u8) -> Result<Self, Error> {
        let e = |e: opus::Error| Error::Encoder(e.to_string());
        let ch = if channels == 1 { opus::Channels::Mono } else { opus::Channels::Stereo };
        let mut enc = opus::Encoder::new(48_000, ch, opus::Application::Audio).map_err(e)?;
        let bitrate = if channels == 1 { BITRATE_MONO } else { BITRATE_STEREO };
        enc.set_bitrate(opus::Bitrate::Bits(bitrate)).map_err(e)?;
        Ok(Encoder(enc, vec![0u8; 4000]))
    }

    fn lookahead(&mut self) -> Result<u16, Error> {
        let l = self.0.get_lookahead().map_err(|e| Error::Encoder(e.to_string()))?;
        Ok(l.clamp(0, u16::MAX as i32) as u16)
    }

    fn encode(&mut self, frame: &[f32]) -> Result<Vec<u8>, Error> {
        let n = self.0.encode_float(frame, &mut self.1).map_err(|e| Error::Encoder(e.to_string()))?;
        Ok(self.1[..n].to_vec())
    }
}

/// Without libopus (type-checking for another OS): recording is unavailable.
#[cfg(not(feature = "opus"))]
struct Encoder;

#[cfg(not(feature = "opus"))]
impl Encoder {
    fn new(_channels: u8) -> Result<Self, Error> {
        Err(Error::Unsupported("built without the Opus encoder"))
    }
    fn lookahead(&mut self) -> Result<u16, Error> {
        Ok(0)
    }
    fn encode(&mut self, _frame: &[f32]) -> Result<Vec<u8>, Error> {
        Ok(Vec::new())
    }
}

#[cfg(all(test, feature = "opus"))]
pub(crate) mod tests {
    use super::*;
    use crate::webm::{read_duration_ms, read_packets};

    /// Decode a file and return the RMS level (dBFS) of each channel.
    pub(crate) fn channel_levels(path: &Path) -> Vec<f64> {
        let (channels, pre_skip, packets) = read_packets(path).unwrap();
        let ch = if channels == 1 { opus::Channels::Mono } else { opus::Channels::Stereo };
        let mut dec = opus::Decoder::new(48_000, ch).unwrap();
        let n = channels as usize;
        let mut sums = vec![0f64; n];
        let mut count = 0usize;
        let mut out = vec![0f32; 5760 * n];
        let mut skip = pre_skip as usize;
        for (_, p) in packets {
            let frames = dec.decode_float(&p, &mut out, false).unwrap();
            for f in 0..frames {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                for c in 0..n {
                    let s = f64::from(out[f * n + c]);
                    sums[c] += s * s;
                }
                count += 1;
            }
        }
        sums.iter().map(|s| 10.0 * (s / count.max(1) as f64).max(1e-12).log10()).collect()
    }

    #[test]
    fn stereo_channels_stay_separate() {
        let path = crate::webm::tests::temp_path("opus.webm");
        let mut f = OpusFile::create(&path, 2, "test").unwrap();
        // 3 s: a 440 Hz tone on channel 0, silence on channel 1, pushed in odd sizes.
        let mut buf = Vec::new();
        for i in 0..(48_000 * 3) {
            let s = 0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            buf.push(s);
            buf.push(0.0);
            if buf.len() >= 2 * 777 {
                f.push(&buf).unwrap();
                buf.clear();
            }
        }
        f.push(&buf).unwrap();
        let done = f.finish().unwrap();
        assert!(done.unsaved.is_none());
        assert!((3_000..=3_020).contains(&done.duration_ms), "{}", done.duration_ms);
        assert!(read_duration_ms(&path).unwrap().is_some());
        let levels = channel_levels(&path);
        assert!(levels[0] > -15.0, "tone channel {levels:?}");
        assert!(levels[1] < -60.0, "silent channel {levels:?}");
    }
}
