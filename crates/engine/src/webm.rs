//! A small WebM (Matroska) muxer for one Opus audio track, written live.
//!
//! Crash safety: clusters are written whole (about two seconds each) and synced
//! to disk, the Segment is opened with an "unknown" size and the duration is
//! left as a Void placeholder. A file cut off by a crash is therefore a valid
//! live WebM up to its last complete cluster — the same shape browsers'
//! MediaRecorder produces. [`WebmWriter::finish`] patches in the segment size
//! and the duration; [`repair`] does the same for a file left behind by a
//! crash, after trimming a half-written cluster.
//!
//! Every byte is also kept in memory, so a recording survives the disk
//! failing mid-meeting (see [`Finished::unsaved`]).

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// MIME type of the files this module writes.
pub const MIME: &str = "audio/webm";

/// Milliseconds of audio per cluster: the most a crash can lose.
const CLUSTER_MS: u64 = 2_000;

const EBML: u32 = 0x1A45_DFA3;
const EBML_VERSION: u32 = 0x4286;
const EBML_READ_VERSION: u32 = 0x42F7;
const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
const DOC_TYPE: u32 = 0x4282;
const DOC_TYPE_VERSION: u32 = 0x4287;
const DOC_TYPE_READ_VERSION: u32 = 0x4285;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const MUXING_APP: u32 = 0x4D80;
const WRITING_APP: u32 = 0x5741;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_UID: u32 = 0x73C5;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63A2;
const CODEC_DELAY: u32 = 0x56AA;
const SEEK_PRE_ROLL: u32 = 0x56BB;
const AUDIO: u32 = 0xE1;
const SAMPLING_FREQUENCY: u32 = 0xB5;
const CHANNELS: u32 = 0x9F;
const CLUSTER: u32 = 0x1F43_B675;
const TIMESTAMP: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const VOID: u32 = 0xEC;

/// Duration element (ID 2 bytes + size 1 byte + 8-byte float): the Void
/// placeholder reserved for it has exactly this size.
const DURATION_ELEMENT_LEN: usize = 11;
const UNKNOWN_SIZE: [u8; 8] = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// Result of [`WebmWriter::finish`].
#[derive(Debug)]
pub struct Finished {
    pub duration_ms: u64,
    pub size: u64,
    /// The whole file, when writing it to disk failed at some point. The caller
    /// must save it elsewhere; the file on disk is incomplete.
    pub unsaved: Option<Vec<u8>>,
    pub disk_error: Option<String>,
}

pub struct WebmWriter {
    file: Option<File>,
    disk_error: Option<String>,
    mirror: Vec<u8>,
    segment_size_pos: usize,
    segment_data_start: usize,
    duration_pos: usize,
    cluster: Vec<u8>,
    cluster_start_ms: Option<u64>,
    end_ms: u64,
    packet_ms: u64,
}

impl WebmWriter {
    /// Create `path` and write the headers for an Opus track.
    /// `pre_skip` is the encoder's lookahead in 48 kHz samples.
    pub fn create(path: &Path, channels: u8, pre_skip: u16, packet_ms: u64, writing_app: &str) -> io::Result<Self> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let mut w = WebmWriter {
            file: Some(file),
            disk_error: None,
            mirror: Vec::with_capacity(1 << 20),
            segment_size_pos: 0,
            segment_data_start: 0,
            duration_pos: 0,
            cluster: Vec::with_capacity(64 * 1024),
            cluster_start_ms: None,
            end_ms: 0,
            packet_ms,
        };
        let header = w.headers(channels, pre_skip, writing_app);
        w.append(&header);
        if let Some(e) = w.disk_error.take() {
            // Nothing useful is on disk yet: fail loudly rather than record to memory only.
            return Err(io::Error::other(e));
        }
        Ok(w)
    }

    fn headers(&mut self, channels: u8, pre_skip: u16, writing_app: &str) -> Vec<u8> {
        let mut out = elem(
            EBML,
            &[
                elem_uint(EBML_VERSION, 1),
                elem_uint(EBML_READ_VERSION, 1),
                elem_uint(EBML_MAX_ID_LENGTH, 4),
                elem_uint(EBML_MAX_SIZE_LENGTH, 8),
                elem_str(DOC_TYPE, "webm"),
                elem_uint(DOC_TYPE_VERSION, 4),
                elem_uint(DOC_TYPE_READ_VERSION, 2),
            ]
            .concat(),
        );

        out.extend(id_bytes(SEGMENT));
        self.segment_size_pos = out.len();
        out.extend(UNKNOWN_SIZE);
        self.segment_data_start = out.len();

        let mut info = Vec::new();
        info.extend(elem_uint(TIMESTAMP_SCALE, 1_000_000)); // timestamps in ms
        info.extend(elem_str(MUXING_APP, "notizli-engine"));
        info.extend(elem_str(WRITING_APP, writing_app));
        let duration_in_info = info.len();
        info.extend(void(DURATION_ELEMENT_LEN));
        let info_elem = elem(INFO, &info);
        self.duration_pos = out.len() + (info_elem.len() - info.len()) + duration_in_info;
        out.extend(info_elem);

        let pre_skip_ns = u64::from(pre_skip) * 1_000_000_000 / 48_000;
        let track = [
            elem_uint(TRACK_NUMBER, 1),
            elem_uint(TRACK_UID, 1),
            elem_uint(TRACK_TYPE, 2), // audio
            elem_str(CODEC_ID, "A_OPUS"),
            elem(CODEC_PRIVATE, &opus_head(channels, pre_skip)),
            elem_uint(CODEC_DELAY, pre_skip_ns),
            elem_uint(SEEK_PRE_ROLL, 80_000_000),
            elem(AUDIO, &[elem_float(SAMPLING_FREQUENCY, 48_000.0), elem_uint(CHANNELS, u64::from(channels))].concat()),
        ]
        .concat();
        out.extend(elem(TRACKS, &elem(TRACK_ENTRY, &track)));
        out
    }

    /// Add one Opus packet starting at `ts_ms` (milliseconds from the start).
    pub fn write_packet(&mut self, ts_ms: u64, packet: &[u8]) {
        if let Some(start) = self.cluster_start_ms {
            if ts_ms.saturating_sub(start) >= CLUSTER_MS {
                self.flush_cluster();
            }
        }
        let start = *self.cluster_start_ms.get_or_insert(ts_ms);
        let rel = (ts_ms.saturating_sub(start)).min(i16::MAX as u64) as i16;
        let mut block = Vec::with_capacity(packet.len() + 4);
        block.push(0x81); // track 1
        block.extend(rel.to_be_bytes());
        block.push(0x80); // keyframe: every Opus packet decodes on its own
        block.extend(packet);
        self.cluster.extend(elem(SIMPLE_BLOCK, &block));
        self.end_ms = ts_ms + self.packet_ms;
    }

    /// Why the file on disk stopped being written, if it did.
    pub fn disk_error(&self) -> Option<&str> {
        self.disk_error.as_deref()
    }

    pub fn duration_ms(&self) -> u64 {
        self.end_ms
    }

    fn flush_cluster(&mut self) {
        let Some(start) = self.cluster_start_ms.take() else { return };
        let mut content = elem_uint(TIMESTAMP, start);
        content.append(&mut self.cluster);
        let cluster = elem(CLUSTER, &content);
        self.append(&cluster);
    }

    fn append(&mut self, bytes: &[u8]) {
        self.mirror.extend_from_slice(bytes);
        if let Some(f) = &mut self.file {
            if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_data()) {
                self.disk_error = Some(e.to_string());
                self.file = None;
            }
        }
    }

    fn patch(&mut self, pos: usize, bytes: &[u8]) {
        self.mirror[pos..pos + bytes.len()].copy_from_slice(bytes);
        if let Some(f) = &mut self.file {
            let r = f
                .seek(SeekFrom::Start(pos as u64))
                .and_then(|_| f.write_all(bytes))
                .and_then(|_| f.seek(SeekFrom::End(0)).map(|_| ()));
            if let Err(e) = r {
                self.disk_error = Some(e.to_string());
                self.file = None;
            }
        }
    }

    /// Write the last cluster, the segment size and the duration.
    pub fn finish(mut self) -> Finished {
        self.flush_cluster();
        let segment_size = (self.mirror.len() - self.segment_data_start) as u64;
        self.patch(self.segment_size_pos, &size_vint8(segment_size));
        let duration = elem_float(DURATION, self.end_ms as f64);
        self.patch(self.duration_pos, &duration);
        if let Some(f) = &mut self.file {
            if let Err(e) = f.sync_all() {
                self.disk_error = Some(e.to_string());
                self.file = None;
            }
        }
        let size = self.mirror.len() as u64;
        Finished {
            duration_ms: self.end_ms,
            size,
            unsaved: if self.file.is_none() { Some(self.mirror) } else { None },
            disk_error: self.disk_error,
        }
    }
}

fn opus_head(channels: u8, pre_skip: u16) -> Vec<u8> {
    let mut h = b"OpusHead".to_vec();
    h.push(1); // version
    h.push(channels);
    h.extend(pre_skip.to_le_bytes());
    h.extend(48_000u32.to_le_bytes()); // input sample rate
    h.extend(0i16.to_le_bytes()); // output gain
    h.push(0); // mapping family 0: mono or stereo
    h
}

fn id_bytes(id: u32) -> Vec<u8> {
    let b = id.to_be_bytes();
    let skip = b.iter().position(|&x| x != 0).unwrap_or(3);
    b[skip..].to_vec()
}

fn size_vint(n: u64) -> Vec<u8> {
    for len in 1..=8u32 {
        // All-ones is reserved for "unknown size".
        if n < (1u64 << (7 * len)) - 1 {
            let marked = n | (1u64 << (7 * len));
            return marked.to_be_bytes()[(8 - len as usize)..].to_vec();
        }
    }
    panic!("element too large");
}

fn size_vint8(n: u64) -> [u8; 8] {
    (n | (1u64 << 56)).to_be_bytes()
}

fn elem(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = id_bytes(id);
    out.extend(size_vint(payload.len() as u64));
    out.extend_from_slice(payload);
    out
}

fn elem_uint(id: u32, v: u64) -> Vec<u8> {
    let b = v.to_be_bytes();
    let skip = b.iter().position(|&x| x != 0).unwrap_or(7);
    elem(id, &b[skip..])
}

fn elem_float(id: u32, v: f64) -> Vec<u8> {
    elem(id, &v.to_be_bytes())
}

fn elem_str(id: u32, v: &str) -> Vec<u8> {
    elem(id, v.as_bytes())
}

/// A Void element of exactly `total` bytes (total >= 2 and < 129).
fn void(total: usize) -> Vec<u8> {
    let mut v = vec![0u8; total];
    v[0] = VOID as u8;
    v[1] = 0x80 | (total - 2) as u8;
    v
}

// ---- reading (repair and tests) --------------------------------------------

/// What [`repair`] found and fixed.
#[derive(Debug, PartialEq, Eq)]
pub struct Repaired {
    pub duration_ms: u64,
    /// Bytes of a half-written cluster cut off the end.
    pub trimmed_bytes: u64,
    /// The file was already finished; nothing was changed.
    pub already_finished: bool,
}

/// Make a recording left behind by a crash complete: cut a half-written final
/// cluster, then write the segment size and the duration.
pub fn repair(path: &Path, packet_ms: u64) -> io::Result<Repaired> {
    let mut f = OpenOptions::new().read(true).write(true).open(path)?;
    let len = f.metadata()?.len();

    let (id, header_size) = read_element_header(&mut f)?;
    if id != EBML {
        return Err(invalid("not a WebM file"));
    }
    f.seek(SeekFrom::Current(header_size.ok_or_else(|| invalid("bad EBML header"))? as i64))?;
    let (id, _) = read_element_header(&mut f)?;
    if id != SEGMENT {
        return Err(invalid("no Segment"));
    }
    let segment_size_pos = f.stream_position()? - 8;
    let segment_data_start = f.stream_position()?;
    let mut size_field = [0u8; 8];
    f.seek(SeekFrom::Start(segment_size_pos))?;
    f.read_exact(&mut size_field)?;
    f.seek(SeekFrom::Start(segment_data_start))?;

    let mut duration_pos = None;
    let mut end_ms = 0u64;
    let mut good_end = segment_data_start;
    loop {
        let start = f.stream_position()?;
        if start >= len {
            break;
        }
        let Ok((id, size)) = read_element_header(&mut f) else { break };
        let Some(size) = size else { break }; // unknown-size child: stop here
        let data_start = f.stream_position()?;
        if data_start + size > len {
            break; // half-written
        }
        match id {
            INFO => {
                let mut info = vec![0u8; size as usize];
                f.read_exact(&mut info)?;
                duration_pos = find_void_placeholder(&info).map(|off| data_start + off as u64);
            }
            CLUSTER => {
                let mut c = vec![0u8; size as usize];
                f.read_exact(&mut c)?;
                if let Some(last) = cluster_last_block_ms(&c) {
                    end_ms = end_ms.max(last + packet_ms);
                }
            }
            _ => {
                f.seek(SeekFrom::Start(data_start + size))?;
            }
        }
        good_end = data_start + size;
    }

    if size_field != UNKNOWN_SIZE && good_end == len {
        return Ok(Repaired { duration_ms: end_ms, trimmed_bytes: 0, already_finished: true });
    }
    f.set_len(good_end)?;
    f.seek(SeekFrom::Start(segment_size_pos))?;
    f.write_all(&size_vint8(good_end - segment_data_start))?;
    if let Some(pos) = duration_pos {
        f.seek(SeekFrom::Start(pos))?;
        f.write_all(&elem_float(DURATION, end_ms as f64))?;
    }
    f.sync_all()?;
    Ok(Repaired { duration_ms: end_ms, trimmed_bytes: len - good_end, already_finished: false })
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Element ID (with its marker bits, as IDs are written) and payload size
/// (None = unknown size).
fn read_element_header(r: &mut impl Read) -> io::Result<(u32, Option<u64>)> {
    let (id, _) = read_vint(r, false)?;
    let (size, unknown) = read_vint(r, true)?;
    Ok((id as u32, if unknown { None } else { Some(size) }))
}

/// Read a variable-length integer. With `strip`, the length marker is removed
/// (sizes); otherwise it is kept (IDs). Returns (value, all-ones).
fn read_vint(r: &mut impl Read, strip: bool) -> io::Result<(u64, bool)> {
    let mut first = [0u8; 1];
    r.read_exact(&mut first)?;
    let len = first[0].leading_zeros() as usize + 1;
    if len > 8 {
        return Err(invalid("bad vint"));
    }
    let mut rest = [0u8; 8];
    r.read_exact(&mut rest[..len - 1])?;
    let mut v = if strip { u64::from(first[0]) & ((1u64 << (8 - len)) - 1) } else { u64::from(first[0]) };
    for b in &rest[..len - 1] {
        v = (v << 8) | u64::from(*b);
    }
    let all_ones = strip && v == (1u64 << (7 * len)) - 1;
    Ok((v, all_ones))
}

/// Offset of the Duration placeholder inside an Info payload, if still a Void.
fn find_void_placeholder(info: &[u8]) -> Option<usize> {
    let mut pos = 0;
    let mut cur = io::Cursor::new(info);
    while (pos as usize) < info.len() {
        let (id, size) = read_element_header(&mut cur).ok()?;
        let size = size?;
        let data = cur.position();
        if id == VOID && (data - pos) + size == DURATION_ELEMENT_LEN as u64 {
            return Some(pos as usize);
        }
        pos = data + size;
        cur.set_position(pos);
    }
    None
}

/// Timestamp (ms) of the last SimpleBlock in a cluster payload.
fn cluster_last_block_ms(cluster: &[u8]) -> Option<u64> {
    let mut cur = io::Cursor::new(cluster);
    let mut base = 0u64;
    let mut last = None;
    while (cur.position() as usize) < cluster.len() {
        let (id, size) = read_element_header(&mut cur).ok()?;
        let size = size? as usize;
        let data = cur.position() as usize;
        let payload = cluster.get(data..data + size)?;
        match id {
            TIMESTAMP => base = payload.iter().fold(0u64, |a, b| (a << 8) | u64::from(*b)),
            SIMPLE_BLOCK if payload.len() >= 4 => {
                let rel = i16::from_be_bytes([payload[1], payload[2]]);
                last = Some(base.saturating_add_signed(i64::from(rel)));
            }
            _ => {}
        }
        cur.set_position((data + size) as u64);
    }
    last
}

/// (timestamp ms, Opus packet) pairs.
pub type Packets = Vec<(u64, Vec<u8>)>;

/// Every Opus packet in a WebM file, for tests and diagnostics:
/// (channels, pre-skip, [(timestamp ms, packet)]).
pub fn read_packets(path: &Path) -> io::Result<(u8, u16, Packets)> {
    let data = std::fs::read(path)?;
    let mut cur = io::Cursor::new(&data[..]);
    let (id, size) = read_element_header(&mut cur)?;
    if id != EBML {
        return Err(invalid("not a WebM file"));
    }
    cur.set_position(cur.position() + size.unwrap_or(0));
    let (id, seg_size) = read_element_header(&mut cur)?;
    if id != SEGMENT {
        return Err(invalid("no Segment"));
    }
    let seg_end = seg_size.map_or(data.len() as u64, |s| cur.position() + s).min(data.len() as u64);
    let (mut channels, mut pre_skip, mut packets) = (0u8, 0u16, Vec::new());
    while cur.position() < seg_end {
        let (id, size) = read_element_header(&mut cur)?;
        let size = size.ok_or_else(|| invalid("unknown-size element"))?;
        let start = cur.position() as usize;
        let payload = data.get(start..start + size as usize).ok_or_else(|| invalid("truncated"))?;
        match id {
            TRACKS => {
                if let Some(head) = find_nested(payload, &[TRACK_ENTRY, CODEC_PRIVATE]) {
                    channels = head[9];
                    pre_skip = u16::from_le_bytes([head[10], head[11]]);
                }
            }
            CLUSTER => {
                let mut c = io::Cursor::new(payload);
                let mut base = 0u64;
                while (c.position() as usize) < payload.len() {
                    let (cid, csize) = read_element_header(&mut c)?;
                    let csize = csize.ok_or_else(|| invalid("unknown size"))? as usize;
                    let d = c.position() as usize;
                    let p = &payload[d..d + csize];
                    match cid {
                        TIMESTAMP => base = p.iter().fold(0u64, |a, b| (a << 8) | u64::from(*b)),
                        SIMPLE_BLOCK => {
                            let rel = i16::from_be_bytes([p[1], p[2]]);
                            packets.push((base.saturating_add_signed(i64::from(rel)), p[4..].to_vec()));
                        }
                        _ => {}
                    }
                    c.set_position((d + csize) as u64);
                }
            }
            _ => {}
        }
        cur.set_position(start as u64 + size);
    }
    Ok((channels, pre_skip, packets))
}

fn find_nested<'a>(payload: &'a [u8], path: &[u32]) -> Option<&'a [u8]> {
    let mut cur = io::Cursor::new(payload);
    while (cur.position() as usize) < payload.len() {
        let (id, size) = read_element_header(&mut cur).ok()?;
        let size = size? as usize;
        let d = cur.position() as usize;
        let p = payload.get(d..d + size)?;
        if id == path[0] {
            return if path.len() == 1 { Some(p) } else { find_nested(p, &path[1..]) };
        }
        cur.set_position((d + size) as u64);
    }
    None
}

/// Duration (ms) stored in a finished file's Info, if any.
pub fn read_duration_ms(path: &Path) -> io::Result<Option<f64>> {
    let data = std::fs::read(path)?;
    let mut cur = io::Cursor::new(&data[..]);
    let (_, size) = read_element_header(&mut cur)?;
    cur.set_position(cur.position() + size.unwrap_or(0));
    let (_, _) = read_element_header(&mut cur)?;
    let (id, size) = read_element_header(&mut cur)?;
    if id != INFO {
        return Ok(None);
    }
    let start = cur.position() as usize;
    let info = &data[start..start + size.unwrap_or(0) as usize];
    Ok(find_nested(info, &[DURATION]).map(|b| f64::from_be_bytes(b.try_into().unwrap_or([0; 8]))))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("notizli-engine-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn vints_round_trip() {
        for n in [0u64, 1, 126, 127, 128, 16_382, 16_383, 1 << 20, (1 << 35) + 7] {
            let bytes = size_vint(n);
            let (v, unknown) = read_vint(&mut io::Cursor::new(&bytes), true).unwrap();
            assert_eq!((v, unknown), (n, false), "n={n}");
        }
        let (_, unknown) = read_vint(&mut io::Cursor::new(&UNKNOWN_SIZE), true).unwrap();
        assert!(unknown);
        let (v, _) = read_vint(&mut io::Cursor::new(&size_vint8(12345)), true).unwrap();
        assert_eq!(v, 12345);
    }

    fn fake_packets(w: &mut WebmWriter, n: u64) {
        for i in 0..n {
            w.write_packet(i * 20, &[0xFC, i as u8, 1, 2, 3]);
        }
    }

    #[test]
    fn finished_file_has_size_duration_and_all_packets() {
        let p = temp_path("finished.webm");
        let mut w = WebmWriter::create(&p, 2, 312, 20, "test").unwrap();
        fake_packets(&mut w, 500); // 10 s
        let done = w.finish();
        assert_eq!(done.duration_ms, 10_000);
        assert!(done.unsaved.is_none());
        let (channels, pre_skip, packets) = read_packets(&p).unwrap();
        assert_eq!((channels, pre_skip, packets.len()), (2, 312, 500));
        assert_eq!(packets[499].0, 9_980);
        assert_eq!(read_duration_ms(&p).unwrap(), Some(10_000.0));
        let r = repair(&p, 20).unwrap();
        assert!(r.already_finished);
    }

    #[test]
    fn crashed_file_is_repaired() {
        let p = temp_path("crashed.webm");
        let mut w = WebmWriter::create(&p, 2, 312, 20, "test").unwrap();
        fake_packets(&mut w, 330); // 6.6 s: three whole clusters on disk, the rest in memory
        std::mem::forget(w); // crash: nothing else is written
        // A half-written cluster at the end, as a crash mid-write would leave.
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(&[0x1F, 0x43, 0xB6, 0x75, 0x40, 0x50, 0xE7, 0x81]).unwrap();
        drop(f);

        let r = repair(&p, 20).unwrap();
        assert_eq!(r.trimmed_bytes, 8);
        assert_eq!(r.duration_ms, 6_000);
        let (_, _, packets) = read_packets(&p).unwrap();
        assert_eq!(packets.len(), 300);
        assert_eq!(read_duration_ms(&p).unwrap(), Some(6_000.0));
        assert!(repair(&p, 20).unwrap().already_finished);
    }
}
