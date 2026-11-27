use crate::h264::{MfH264Decoder, MfH264Encoder};
use crate::win::WindowsCapturer;
use crate::{decode_frame_to_bgra, CaptureError, ScreenCapturer};
use agentdesk_protocol::{ControlMessage, DataPacket, FrameEncoding, VideoFrame};
use jpeg_encoder::{ColorType as JpegColorType, Encoder as JpegEncoder, SamplingFactor};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

const HYBRID_MAGIC: u8 = 0xA1;
const FLAG_KEYFRAME: u8 = 0x01;
const CODEC_JPEG_BGRA: u8 = 0;
const CODEC_LZ4_BGRA: u8 = 1;
const TILE_SIZE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamConfig {
    pub max_width: u16,
    pub jpeg_quality: u8,
    pub max_fps: u8,
    pub auto_adapt: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            // 0 = native resolution. Downscaling desktop text is what made sessions look blurry.
            max_width: 0,
            jpeg_quality: 72,
            max_fps: 60,
            auto_adapt: true,
        }
    }
}

/// Highest quality the adaptive controller may climb to unless the viewer asks for more.
const ABR_QUALITY_CAP: f32 = 88.0;
const ABR_QUALITY_FLOOR: f32 = 38.0;
const ABR_MIN_FPS: u8 = 20;

/// Delay-based rate controller.
///
/// Instead of reacting to absolute RTT (which punished long-but-healthy internet paths and
/// changed the resolution, forcing a new encoder + IDR every time), it reacts to *queuing
/// delay*: smoothed RTT minus the lowest RTT seen recently. Rising queuing delay means we
/// are sending faster than the path drains, so quality (and therefore bitrate) backs off
/// multiplicatively; an empty queue lets it climb additively (AIMD, like GCC/LEDBAT).
/// Resolution is never changed automatically.
#[derive(Debug, Clone)]
pub struct AbrController {
    pub config: StreamConfig,
    sent_seq: u64,
    acked_seq: u64,
    last_send: Instant,
    ewma_rtt_ms: Option<f32>,
    min_rtt_ms: Option<f32>,
    last_decode_ms: u16,
    sent_times: VecDeque<(u64, Instant)>,
    quality: f32,
    quality_cap: f32,
    fps_cap: u8,
    last_adapt: Instant,
}

impl Default for AbrController {
    fn default() -> Self {
        Self::new()
    }
}

impl AbrController {
    pub fn new() -> Self {
        let config = StreamConfig::default();
        Self {
            config,
            sent_seq: 0,
            acked_seq: 0,
            last_send: Instant::now() - Duration::from_secs(5),
            ewma_rtt_ms: None,
            min_rtt_ms: None,
            last_decode_ms: 0,
            sent_times: VecDeque::with_capacity(32),
            quality: config.jpeg_quality as f32,
            quality_cap: ABR_QUALITY_CAP,
            fps_cap: config.max_fps,
            last_adapt: Instant::now(),
        }
    }

    pub fn in_flight(&self) -> u64 {
        self.sent_seq.saturating_sub(self.acked_seq)
    }

    /// Frames allowed in flight: one RTT worth of frames plus headroom, so the frame rate no
    /// longer collapses on high-RTT paths (2 fixed frames capped 100 ms RTT at 20 fps), while
    /// still bounding the queue to about one RTT.
    pub fn window(&self) -> u64 {
        let rtt = self.ewma_rtt_ms.unwrap_or(30.0).max(1.0);
        let frame_ms = 1000.0 / self.config.max_fps.max(1) as f32;
        ((rtt / frame_ms).ceil() as u64 + 2).clamp(2, 8)
    }

    /// Anti-bufferbloat gate with a 400 ms watchdog so lost ACKs never stall capture for long.
    pub fn can_send_now(&self) -> bool {
        self.in_flight() < self.window() || self.last_send.elapsed() > Duration::from_millis(400)
    }

    pub fn on_frame_sent(&mut self, seq: u64) {
        self.sent_seq = seq;
        let now = Instant::now();
        self.last_send = now;
        self.sent_times.push_back((seq, now));
        while self.sent_times.len() > 32 {
            self.sent_times.pop_front();
        }
    }

    pub fn on_ack_measured(&mut self, seq: u64, decode_ms: u16) {
        let measured_rtt = self
            .sent_times
            .iter()
            .find(|(s, _)| *s == seq)
            .map(|(_, t)| (t.elapsed().as_millis() as u64).saturating_sub(decode_ms as u64))
            .unwrap_or(50);
        self.on_ack(seq, measured_rtt, decode_ms);
    }

    pub fn set_policy(&mut self, max_width: u16, jpeg_quality: u8, max_fps: u8, auto_adapt: bool) {
        let q = jpeg_quality.clamp(25, 95);
        let fps = max_fps.clamp(1, 60);
        self.config = StreamConfig {
            max_width,
            jpeg_quality: q,
            max_fps: fps,
            auto_adapt,
        };
        self.quality = q as f32;
        self.quality_cap = if auto_adapt { ABR_QUALITY_CAP.max(q as f32) } else { q as f32 };
        self.fps_cap = fps;
    }

    pub fn set_legacy_quality(&mut self, jpeg_quality: u8, max_fps: u8) {
        self.config.auto_adapt = false;
        self.config.jpeg_quality = jpeg_quality.clamp(25, 95);
        self.config.max_fps = max_fps.clamp(1, 60);
        self.quality = self.config.jpeg_quality as f32;
        self.quality_cap = self.quality;
        self.fps_cap = self.config.max_fps;
    }

    /// Queuing delay estimate in ms (smoothed RTT above the path's base RTT).
    pub fn queue_delay_ms(&self) -> f32 {
        match (self.ewma_rtt_ms, self.min_rtt_ms) {
            (Some(s), Some(m)) => (s - m).max(0.0),
            _ => 0.0,
        }
    }

    pub fn on_ack(&mut self, seq: u64, rtt_ms: u64, decode_ms: u16) {
        self.acked_seq = self.acked_seq.max(seq);
        self.last_decode_ms = decode_ms;
        let sample = rtt_ms.min(2000) as f32;
        let smoothed = match self.ewma_rtt_ms {
            Some(prev) => prev * 0.8 + sample * 0.2,
            None => sample,
        };
        self.ewma_rtt_ms = Some(smoothed);
        // Base RTT: follows drops instantly, drifts up slowly (~1 ms per 25 acks) so a route
        // change is eventually learned.
        self.min_rtt_ms = Some(match self.min_rtt_ms {
            Some(m) => (m + 0.04).min(sample),
            None => sample,
        });

        if !self.config.auto_adapt {
            return;
        }
        if self.last_adapt.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.last_adapt = Instant::now();

        let qd = self.queue_delay_ms();
        let mut fps = self.config.max_fps;
        if qd > 60.0 {
            self.quality *= 0.85;
            if self.quality <= ABR_QUALITY_FLOOR + 2.0 {
                fps = fps.saturating_sub(10).max(ABR_MIN_FPS);
            }
        } else if qd > 30.0 {
            self.quality -= 3.0;
        } else if qd < 12.0 {
            self.quality += 1.5;
            if fps < self.fps_cap {
                fps = (fps + 5).min(self.fps_cap);
            }
        }
        self.quality = self.quality.clamp(ABR_QUALITY_FLOOR, self.quality_cap);
        if decode_ms > 30 {
            fps = fps.min(30);
        }
        self.config.jpeg_quality = self.quality.round() as u8;
        self.config.max_fps = fps.max(1);
    }

    pub fn smoothed_rtt_ms(&self) -> Option<f32> {
        self.ewma_rtt_ms
    }
}

/// H.264 bitrate from resolution, frame rate and the adaptive quality (25..95).
/// q 88 at 2560x1440@60 ~= 26 Mbps, q 38 ~= 8 Mbps; desktop content rarely needs the peak
/// because unchanged screens produce no frames at all.
pub fn h264_bitrate_for(width: u32, height: u32, fps: u32, quality: u8) -> u32 {
    let q = (quality.clamp(25, 95) as f32 - 25.0) / 70.0;
    let bits_per_pixel = 0.02 + q * 0.10;
    let bps = width as f32 * height as f32 * fps.clamp(10, 60) as f32 * bits_per_pixel;
    (bps as u32).clamp(1_500_000, 40_000_000)
}

pub struct HostStreamEncoder {
    width: u32,
    height: u32,
    prev_bgra: Vec<u8>,
    scaled_scratch: Vec<u8>,
    rect_scratch: Vec<u8>,
    force_keyframe: bool,
}

impl Default for HostStreamEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl HostStreamEncoder {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            prev_bgra: Vec::new(),
            scaled_scratch: Vec::new(),
            rect_scratch: Vec::new(),
            force_keyframe: true,
        }
    }

    pub fn force_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    /// Encodes `src_bgra` into `FrameEncoding::HybridTiles` payload.
    /// Returns `None` if the frame is identical to the previous frame and no keyframe was requested.
    pub fn encode_bgra(
        &mut self,
        src_w: u32,
        src_h: u32,
        src_bgra: &[u8],
        max_width: u16,
        jpeg_quality: u8,
    ) -> Option<(u32, u32, Vec<u8>)> {
        if src_w < 2 || src_h < 2 || src_bgra.len() < (src_w as usize * src_h as usize * 4) {
            return None;
        }

        let (w, h, use_scaled) = if max_width > 0 && src_w > max_width as u32 {
            let mut dw = (max_width as u32).max(320);
            let mut dh = ((src_h as u64 * dw as u64) / src_w as u64).max(2) as u32;
            dw &= !1;
            dh &= !1;
            scale_bgra_bilinear(src_w, src_h, src_bgra, dw, dh, &mut self.scaled_scratch);
            (dw, dh, true)
        } else {
            (src_w, src_h, false)
        };

        let cur: &[u8] = if use_scaled {
            &self.scaled_scratch
        } else {
            &src_bgra[..(w as usize * h as usize * 4)]
        };

        let expected = w as usize * h as usize * 4;
        let is_keyframe =
            self.force_keyframe || self.width != w || self.height != h || self.prev_bgra.len() != expected;

        let mut rects: Vec<(u16, u16, u16, u16)> = Vec::new();
        if is_keyframe {
            self.width = w;
            self.height = h;
            self.prev_bgra.resize(expected, 0);
            self.prev_bgra.copy_from_slice(cur);
            self.force_keyframe = false;
            rects.push((0, 0, w as u16, h as u16));
        } else {
            let cols = (w as usize).div_ceil(TILE_SIZE);
            let rows = (h as usize).div_ceil(TILE_SIZE);
            let total_tiles = cols * rows;
            let mut dirty_tiles = 0usize;
            let stride = w as usize * 4;

            for ty in 0..rows {
                let y0 = ty * TILE_SIZE;
                let y1 = (y0 + TILE_SIZE).min(h as usize);
                let mut run_start: Option<usize> = None;

                for tx in 0..cols {
                    let x0 = tx * TILE_SIZE;
                    let x1 = (x0 + TILE_SIZE).min(w as usize);
                    let row_bytes = (x1 - x0) * 4;
                    let mut tile_dirty = false;
                    for y in y0..y1 {
                        let off = y * stride + x0 * 4;
                        if cur[off..off + row_bytes] != self.prev_bgra[off..off + row_bytes] {
                            tile_dirty = true;
                            break;
                        }
                    }

                    if tile_dirty {
                        dirty_tiles += 1;
                        if run_start.is_none() {
                            run_start = Some(tx);
                        }
                    } else if let Some(start_tx) = run_start.take() {
                        let rx = (start_tx * TILE_SIZE) as u16;
                        let rx1 = (tx * TILE_SIZE).min(w as usize) as u16;
                        rects.push((rx, y0 as u16, rx1 - rx, (y1 - y0) as u16));
                    }
                }
                if let Some(start_tx) = run_start.take() {
                    let rx = (start_tx * TILE_SIZE) as u16;
                    let rx1 = w as u16;
                    rects.push((rx, y0 as u16, rx1 - rx, (y1 - y0) as u16));
                }
            }

            if dirty_tiles == 0 {
                return None;
            }

            if dirty_tiles * 100 > total_tiles * 55 {
                rects.clear();
                rects.push((0, 0, w as u16, h as u16));
                self.prev_bgra.copy_from_slice(cur);
            } else {
                for &(rx, ry, rw, rh) in &rects {
                    let x0 = rx as usize;
                    let y0 = ry as usize;
                    let row_bytes = rw as usize * 4;
                    for y in y0..(y0 + rh as usize) {
                        let off = y * stride + x0 * 4;
                        self.prev_bgra[off..off + row_bytes]
                            .copy_from_slice(&cur[off..off + row_bytes]);
                    }
                }
            }
        }

        let mut out = Vec::with_capacity(if is_keyframe { 64 * 1024 } else { 16 * 1024 });
        out.push(HYBRID_MAGIC);
        out.push(if is_keyframe { FLAG_KEYFRAME } else { 0 });
        let rect_count = rects.len().min(u16::MAX as usize) as u16;
        out.extend_from_slice(&rect_count.to_le_bytes());

        let q = jpeg_quality.clamp(25, 95);
        let stride = w as usize * 4;

        for &(rx, ry, rw, rh) in rects.iter().take(rect_count as usize) {
            let need = rw as usize * rh as usize * 4;
            self.rect_scratch.resize(need, 0);
            let row_bytes = rw as usize * 4;
            for r in 0..rh as usize {
                let src_off = (ry as usize + r) * stride + rx as usize * 4;
                let dst_off = r * row_bytes;
                self.rect_scratch[dst_off..dst_off + row_bytes]
                    .copy_from_slice(&cur[src_off..src_off + row_bytes]);
            }

            let mut encoded_codec = CODEC_JPEG_BGRA;
            let mut encoded_bytes: Vec<u8> = Vec::new();

            if !is_keyframe
                && (rw as usize * rh as usize) <= (TILE_SIZE * TILE_SIZE * 8)
                && looks_like_flat_ui(&self.rect_scratch)
            {
                let lz4 = lz4_flex::compress_prepend_size(&self.rect_scratch);
                if lz4.len() * 3 < self.rect_scratch.len() {
                    encoded_codec = CODEC_LZ4_BGRA;
                    encoded_bytes = lz4;
                }
            }

            if encoded_codec == CODEC_JPEG_BGRA {
                encoded_bytes.reserve(need / 10);
                let mut enc = JpegEncoder::new(&mut encoded_bytes, q);
                // 4:4:4: chroma subsampling smears coloured text edges (ClearType, syntax colours).
                enc.set_sampling_factor(SamplingFactor::F_1_1);
                if enc
                    .encode(&self.rect_scratch, rw, rh, JpegColorType::Bgra)
                    .is_err()
                {
                    return None;
                }
            }

            out.push(encoded_codec);
            out.extend_from_slice(&rx.to_le_bytes());
            out.extend_from_slice(&ry.to_le_bytes());
            out.extend_from_slice(&rw.to_le_bytes());
            out.extend_from_slice(&rh.to_le_bytes());
            out.extend_from_slice(&(encoded_bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&encoded_bytes);
        }

        Some((w, h, out))
    }
}

fn looks_like_flat_ui(bgra: &[u8]) -> bool {
    let total_px = bgra.len() / 4;
    if total_px == 0 {
        return false;
    }
    let step = (total_px / 64).max(1);
    let mut distinct: HashSet<u16> = HashSet::with_capacity(20);
    let mut i = 0usize;
    while i < total_px {
        let p = &bgra[i * 4..i * 4 + 4];
        let key = (((p[2] >> 4) as u16) << 8) | (((p[1] >> 4) as u16) << 4) | ((p[0] >> 4) as u16);
        distinct.insert(key);
        if distinct.len() > 16 {
            return false;
        }
        i += step;
    }
    true
}

/// Bilinear BGRA resize (fixed point). Replaces nearest-neighbour, which dropped whole pixel
/// columns/rows and made downscaled text unreadable.
fn scale_bgra_bilinear(
    src_w: u32,
    src_h: u32,
    src: &[u8],
    dst_w: u32,
    dst_h: u32,
    out: &mut Vec<u8>,
) {
    let (sw, sh, dw, dh) = (src_w as usize, src_h as usize, dst_w as usize, dst_h as usize);
    out.resize(dw * dh * 4, 0);
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return;
    }
    let src_stride = sw * 4;
    // 16.16 source coordinates of each destination pixel centre.
    let x_map: Vec<(usize, u32)> = (0..dw)
        .map(|dx| {
            let fx = (((dx as u64 * 2 + 1) * sw as u64 * 65536) / (dw as u64 * 2)).saturating_sub(32768);
            let x0 = ((fx >> 16) as usize).min(sw - 1);
            (x0, ((fx & 0xFFFF) >> 8) as u32)
        })
        .collect();
    for dy in 0..dh {
        let fy = (((dy as u64 * 2 + 1) * sh as u64 * 65536) / (dh as u64 * 2)).saturating_sub(32768);
        let y0 = ((fy >> 16) as usize).min(sh - 1);
        let y1 = (y0 + 1).min(sh - 1);
        let wy = ((fy & 0xFFFF) >> 8) as u32;
        let r0 = &src[y0 * src_stride..(y0 + 1) * src_stride];
        let r1 = &src[y1 * src_stride..(y1 + 1) * src_stride];
        let dst_row = &mut out[dy * dw * 4..(dy + 1) * dw * 4];
        for (dx, &(x0, wx)) in x_map.iter().enumerate() {
            let x1 = (x0 + 1).min(sw - 1);
            let (a0, a1, b0, b1) = (x0 * 4, x1 * 4, x0 * 4, x1 * 4);
            for c in 0..4 {
                let top = r0[a0 + c] as u32 * (256 - wx) + r0[a1 + c] as u32 * wx;
                let bot = r1[b0 + c] as u32 * (256 - wx) + r1[b1 + c] as u32 * wx;
                dst_row[dx * 4 + c] = ((top * (256 - wy) + bot * wy + 32768) >> 16) as u8;
            }
        }
    }
}

/// Crops to the top-left `dst_w x dst_h` region (used to make odd dimensions even without
/// resampling the whole frame).
fn crop_bgra(src_w: u32, src: &[u8], dst_w: u32, dst_h: u32, out: &mut Vec<u8>) {
    let row = dst_w as usize * 4;
    let src_stride = src_w as usize * 4;
    out.resize(row * dst_h as usize, 0);
    for y in 0..dst_h as usize {
        out[y * row..(y + 1) * row].copy_from_slice(&src[y * src_stride..y * src_stride + row]);
    }
}

pub struct ViewerFrameCanvas {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    h264: Option<MfH264Decoder>,
    last_seq: u64,
    waiting_idr: bool,
    last_kf_request: Option<Instant>,
}

/// What the viewer should do with an incoming video frame (see [`ViewerFrameCanvas::admit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameAdmission {
    pub decode: bool,
    pub request_keyframe: bool,
}

/// True when an Annex B H.264 payload contains an IDR slice (NAL type 5).
pub fn h264_annexb_has_idr(payload: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 3 < payload.len() {
        if payload[i] == 0 && payload[i + 1] == 0 {
            let (sc, ok) = if payload[i + 2] == 1 {
                (3, true)
            } else if payload[i + 2] == 0 && payload[i + 3] == 1 {
                (4, true)
            } else {
                (0, false)
            };
            if ok {
                if let Some(&hdr) = payload.get(i + sc) {
                    if hdr & 0x1F == 5 {
                        return true;
                    }
                }
                i += sc;
                continue;
            }
        }
        i += 1;
    }
    false
}

impl Default for ViewerFrameCanvas {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewerFrameCanvas {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            bgra: Vec::new(),
            h264: None,
            last_seq: 0,
            waiting_idr: false,
            last_kf_request: None,
        }
    }

    /// Loss detection. A gap in `seq` means a frame was lost (UDP fragment, full queue).
    /// For H.264 every later P-frame would decode into grey smears, so we keep showing the
    /// last good image, skip until an IDR arrives and ask the host for one (rate limited).
    /// Tile frames are still applied, but a keyframe is requested to repaint stale tiles.
    pub fn admit(&mut self, seq: u64, encoding: FrameEncoding, payload: &[u8]) -> FrameAdmission {
        let is_h264 = encoding == FrameEncoding::H264AnnexB;
        // Host restarted its capture worker (reconnect / monitor switch): start over.
        if self.last_seq > 0 && seq + 1000 < self.last_seq {
            self.last_seq = 0;
        }
        if self.last_seq > 0 && seq <= self.last_seq {
            // Late duplicate or reordered frame: never decode it out of order.
            return FrameAdmission { decode: false, request_keyframe: false };
        }
        let gap = self.last_seq > 0 && seq != self.last_seq + 1;
        self.last_seq = seq;

        let mut want_kf = false;
        if is_h264 {
            if h264_annexb_has_idr(payload) {
                self.waiting_idr = false;
            } else if gap || self.waiting_idr {
                self.waiting_idr = true;
                want_kf = true;
            }
        } else {
            self.waiting_idr = false;
            want_kf = gap;
        }

        let request_keyframe = want_kf
            && self
                .last_kf_request
                .map(|t| t.elapsed() > Duration::from_millis(250))
                .unwrap_or(true);
        if request_keyframe {
            self.last_kf_request = Some(Instant::now());
        }
        FrameAdmission {
            decode: !(is_h264 && self.waiting_idr),
            request_keyframe,
        }
    }

    pub fn apply_frame(
        &mut self,
        encoding: FrameEncoding,
        frame_w: u32,
        frame_h: u32,
        payload: &[u8],
    ) -> Result<(u32, u32, Vec<u8>), CaptureError> {
        match encoding {
            FrameEncoding::HybridTiles => {
                if payload.len() < 4 || payload[0] != HYBRID_MAGIC {
                    return Err(CaptureError::Codec("invalid HybridTiles header".into()));
                }
                if frame_w == 0 || frame_h == 0 {
                    return Err(CaptureError::Codec("invalid HybridTiles dimensions".into()));
                }
                let need = frame_w as usize * frame_h as usize * 4;
                if self.width != frame_w || self.height != frame_h || self.bgra.len() != need {
                    self.width = frame_w;
                    self.height = frame_h;
                    self.bgra.resize(need, 0);
                }
                let rect_count = u16::from_le_bytes([payload[2], payload[3]]) as usize;
                let mut pos = 4usize;
                let dst_stride = frame_w as usize * 4;

                for _ in 0..rect_count {
                    if pos + 13 > payload.len() {
                        return Err(CaptureError::Codec("truncated HybridTiles rect header".into()));
                    }
                    let codec = payload[pos];
                    let rx = u16::from_le_bytes([payload[pos + 1], payload[pos + 2]]) as usize;
                    let ry = u16::from_le_bytes([payload[pos + 3], payload[pos + 4]]) as usize;
                    let rw = u16::from_le_bytes([payload[pos + 5], payload[pos + 6]]) as usize;
                    let rh = u16::from_le_bytes([payload[pos + 7], payload[pos + 8]]) as usize;
                    let blen = u32::from_le_bytes([
                        payload[pos + 9],
                        payload[pos + 10],
                        payload[pos + 11],
                        payload[pos + 12],
                    ]) as usize;
                    pos += 13;
                    if pos + blen > payload.len() {
                        return Err(CaptureError::Codec("truncated HybridTiles rect data".into()));
                    }
                    if rx + rw > frame_w as usize || ry + rh > frame_h as usize {
                        return Err(CaptureError::Codec("HybridTiles rect out of bounds".into()));
                    }
                    let rect_bytes = &payload[pos..pos + blen];
                    pos += blen;

                    let decoded_bgra = match codec {
                        CODEC_JPEG_BGRA => decode_jpeg_bgra_with_zune(rect_bytes)?,
                        CODEC_LZ4_BGRA => lz4_flex::decompress_size_prepended(rect_bytes)
                            .map_err(|e| CaptureError::Codec(e.to_string()))?,
                        other => {
                            return Err(CaptureError::Codec(format!("unknown tile codec {other}")));
                        }
                    };

                    let row_bytes = rw * 4;
                    if decoded_bgra.len() < rh * row_bytes {
                        return Err(CaptureError::Codec("decoded tile smaller than rect".into()));
                    }
                    for r in 0..rh {
                        let src_off = r * row_bytes;
                        let dst_off = (ry + r) * dst_stride + rx * 4;
                        self.bgra[dst_off..dst_off + row_bytes]
                            .copy_from_slice(&decoded_bgra[src_off..src_off + row_bytes]);
                    }
                }

                Ok((self.width, self.height, self.bgra.clone()))
            }
            FrameEncoding::Jpeg => {
                let (w, h, bgra) = decode_frame_to_bgra(FrameEncoding::Jpeg, payload)?;
                let w = if w > 0 { w } else { frame_w };
                let h = if h > 0 { h } else { frame_h };
                self.width = w;
                self.height = h;
                self.bgra = bgra.clone();
                Ok((w, h, bgra))
            }
            FrameEncoding::Lz4Rgba => {
                let mut raw = lz4_flex::decompress_size_prepended(payload)
                    .map_err(|e| CaptureError::Codec(e.to_string()))?;
                for px in raw.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
                self.width = frame_w;
                self.height = frame_h;
                self.bgra = raw.clone();
                Ok((frame_w, frame_h, raw))
            }
            FrameEncoding::H264AnnexB => {
                if self.h264.is_none() {
                    self.h264 = MfH264Decoder::try_new();
                }
                let dec = self
                    .h264
                    .as_mut()
                    .ok_or_else(|| CaptureError::Codec("H.264 decoder unavailable".into()))?;
                match dec.decode_to_bgra(payload, frame_w, frame_h) {
                    Ok(Some((w, h, bgra))) => {
                        self.width = w;
                        self.height = h;
                        self.bgra = bgra.clone();
                        Ok((w, h, bgra))
                    }
                    Ok(None) => {
                        if !self.bgra.is_empty() && self.width > 0 && self.height > 0 {
                            Ok((self.width, self.height, self.bgra.clone()))
                        } else {
                            Err(CaptureError::WouldBlock)
                        }
                    }
                    Err(e) => {
                        self.h264 = None;
                        Err(CaptureError::Codec(e))
                    }
                }
            }
        }
    }

    pub fn apply_frame_rgba(
        &mut self,
        encoding: FrameEncoding,
        frame_w: u32,
        frame_h: u32,
        payload: &[u8],
    ) -> Result<(u32, u32, Vec<u8>), CaptureError> {
        let (w, h, mut bgra) = self.apply_frame(encoding, frame_w, frame_h, payload)?;
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        Ok((w, h, bgra))
    }
}

pub(crate) fn decode_jpeg_bgra_with_zune(payload: &[u8]) -> Result<Vec<u8>, CaptureError> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA);
    let mut decoder = JpegDecoder::new_with_options(payload, options);
    decoder
        .decode()
        .map_err(|e| CaptureError::Codec(e.to_string()))
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Shared Level 1 + 2 capture thread used by all host binaries (`desktop-gpui`, `quicksupport`,
/// `agent-host`, and `desktop`): DXGI change-only BGRA capture, Hardware H.264 MFT (with
/// automatic fallback to the software MFT and then HybridTiles delta encoding), delay-based
/// rate control, and cursor state updates.
pub fn spawn_capture_worker(
    stop_rx: tokio::sync::watch::Receiver<bool>,
    abr: Arc<Mutex<AbrController>>,
    monitor_idx: Arc<AtomicUsize>,
    keyframe_req: Arc<AtomicBool>,
    codec_pref: Arc<AtomicU8>,
    pkt_tx: tokio::sync::mpsc::Sender<DataPacket>,
    cursor_tx: tokio::sync::mpsc::Sender<DataPacket>,
) {
    std::thread::spawn(move || {
        let mut active_mon = monitor_idx.load(Ordering::Relaxed);
        let mut cap = WindowsCapturer::new_for_monitor(active_mon);
        let mut encoder = HostStreamEncoder::new();
        let mut h264_enc: Option<MfH264Encoder> = None;
        let mut h264_scaled: Vec<u8> = Vec::new();
        let mut h264_disabled_until = Instant::now() - Duration::from_secs(10);
        // Set once a hardware encoder failed at runtime: next attempts use the software MFT.
        let mut h264_hw_broken = false;
        let mut h264_empty_streak = 0u16;
        let mut h264_force_next = false;
        let mut last_codec_label = String::new();
        let mut bgra_buf: Vec<u8> = Vec::new();
        let mut last_dims: Option<(u32, u32)> = None;
        let mut seq = 0u64;
        let mut last_cursor: Option<(f32, f32, bool, agentdesk_protocol::CursorShape)> = None;
        let mut last_cursor_sent = Instant::now() - Duration::from_secs(1);
        // Quality refinement: after the screen stops changing, re-encode the still image a few
        // times so H.264 sharpens text that was blurred while it was moving (scroll, drag).
        let mut refine_left = 0u8;
        let mut last_video_sent = Instant::now();

        while !*stop_rx.borrow() {
            let desired_mon = monitor_idx.load(Ordering::Relaxed);
            if desired_mon != active_mon {
                if cap.switch_monitor(desired_mon) {
                    active_mon = desired_mon;
                    encoder.force_keyframe();
                    h264_enc = None;
                    last_dims = None;
                } else {
                    monitor_idx.store(active_mon, Ordering::Relaxed);
                }
            }

            if last_cursor_sent.elapsed() >= Duration::from_millis(8) {
                if let Some(cur) = cap.capture_cursor_state() {
                    let changed = match last_cursor {
                        None => true,
                        Some((px, py, pv, ps)) => {
                            (cur.0 - px).abs() > 0.0005
                                || (cur.1 - py).abs() > 0.0005
                                || cur.2 != pv
                                || cur.3 != ps
                        }
                    };
                    if changed {
                        last_cursor = Some(cur);
                        last_cursor_sent = Instant::now();
                        let _ = cursor_tx.try_send(DataPacket::Control(
                            ControlMessage::CursorUpdate {
                                x_norm: cur.0,
                                y_norm: cur.1,
                                visible: cur.2,
                                shape: cur.3,
                            },
                        ));
                    }
                }
            }

            let (can_send, max_width, jpeg_quality, target_fps) = {
                let g = abr.lock().unwrap();
                (
                    g.can_send_now(),
                    g.config.max_width,
                    g.config.jpeg_quality,
                    g.config.max_fps.max(1) as u64,
                )
            };

            // Never encode a frame we cannot hand to the network task right away: a dropped
            // H.264 frame corrupts every following P-frame until the next IDR.
            if !can_send || pkt_tx.capacity() == 0 {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }

            let force_kf = keyframe_req.swap(false, Ordering::Relaxed);
            if force_kf {
                encoder.force_keyframe();
            }

            let budget = Duration::from_micros(1_000_000 / target_fps);
            let t0 = Instant::now();
            // Wait for a new desktop frame for at most one frame interval.
            let wait_ms = (1000 / target_fps as u32).clamp(4, 16);

            let mut is_refine = false;
            let updated_dims = match cap.capture_bgra_if_changed(wait_ms, &mut bgra_buf) {
                Ok(Some(dims)) => {
                    last_dims = Some(dims);
                    refine_left = 3;
                    Some(dims)
                }
                Ok(None) => {
                    if last_dims.is_none() {
                        if let Ok(raw) = cap.capture_rgba() {
                            bgra_buf = raw.rgba;
                            for px in bgra_buf.chunks_exact_mut(4) {
                                px.swap(0, 2);
                            }
                            let dims = (raw.width, raw.height);
                            last_dims = Some(dims);
                            Some(dims)
                        } else {
                            None
                        }
                    } else if force_kf || h264_force_next {
                        last_dims
                    } else if refine_left > 0
                        && h264_enc.is_some()
                        && last_video_sent.elapsed() >= Duration::from_millis(90)
                    {
                        refine_left -= 1;
                        is_refine = true;
                        last_dims
                    } else {
                        None
                    }
                }
                Err(_) => None,
            };

            if let Some((src_w, src_h)) = updated_dims {
                let pref = codec_pref.load(Ordering::Relaxed); // 0=Auto, 1=HardwareH264, 2=HybridTiles
                let try_h264 = pref != 2 && (pref == 1 || Instant::now() >= h264_disabled_until);
                let mut sent_h264 = false;

                if try_h264 {
                    let (tw, th, scaled_ref): (u32, u32, &[u8]) =
                        if max_width > 0 && src_w > max_width as u32 {
                            let dw = (max_width as u32).max(320) & !1;
                            let dh = (((src_h as u64 * dw as u64) / src_w as u64).max(2) as u32) & !1;
                            scale_bgra_bilinear(src_w, src_h, &bgra_buf, dw, dh, &mut h264_scaled);
                            (dw, dh, &h264_scaled)
                        } else {
                            let dw = src_w & !1;
                            let dh = src_h & !1;
                            if dw != src_w || dh != src_h {
                                crop_bgra(src_w, &bgra_buf, dw, dh, &mut h264_scaled);
                                (dw, dh, &h264_scaled)
                            } else {
                                (src_w, src_h, &bgra_buf)
                            }
                        };

                    let bitrate = h264_bitrate_for(tw, th, target_fps as u32, jpeg_quality);
                    let need_recreate = match &h264_enc {
                        Some(enc) => enc.dimensions() != (tw, th),
                        None => true,
                    };
                    if need_recreate {
                        h264_enc = MfH264Encoder::try_new_with(
                            tw,
                            th,
                            60,
                            bitrate,
                            !h264_hw_broken,
                            true,
                        );
                        h264_empty_streak = 0;
                        h264_force_next = true;
                        if h264_enc.is_none() {
                            h264_disabled_until = Instant::now() + Duration::from_secs(60);
                        }
                    } else if let Some(enc) = h264_enc.as_mut() {
                        // Live bitrate change: no new encoder, no IDR, no resolution jump.
                        let cur = enc.bitrate().max(1);
                        if bitrate.abs_diff(cur) * 10 > cur {
                            enc.set_bitrate(bitrate);
                        }
                    }

                    let want_kf = force_kf || h264_force_next;
                    let mut h264_failed = false;
                    if let Some(enc) = h264_enc.as_mut() {
                        match enc.encode_bgra(scaled_ref, tw, th, want_kf) {
                            Ok(Some(annex_b)) => {
                                h264_empty_streak = 0;
                                h264_force_next = false;
                                if last_codec_label != enc.codec_name {
                                    last_codec_label = enc.codec_name.clone();
                                    let _ = cursor_tx.try_send(DataPacket::Control(
                                        ControlMessage::VideoCodecActive {
                                            codec_name: enc.codec_name.clone(),
                                            hardware: enc.is_hardware,
                                        },
                                    ));
                                }
                                seq += 1;
                                let pkt = DataPacket::VideoFrame(VideoFrame {
                                    seq,
                                    timestamp_ms: unix_now_ms(),
                                    width: tw,
                                    height: th,
                                    encoding: FrameEncoding::H264AnnexB,
                                    payload: annex_b,
                                });
                                if pkt_tx.try_send(pkt).is_ok() {
                                    abr.lock().unwrap().on_frame_sent(seq);
                                    last_video_sent = Instant::now();
                                } else {
                                    // Reference chain broken: recover with an IDR.
                                    h264_force_next = true;
                                }
                                sent_h264 = true;
                            }
                            Ok(None) => {
                                // Async encoders legitimately return nothing while busy;
                                // only give up after ~1.5 s without a single output.
                                h264_empty_streak = h264_empty_streak.saturating_add(1);
                                sent_h264 = true;
                                if h264_empty_streak >= 90 {
                                    h264_failed = true;
                                }
                            }
                            Err(_) => h264_failed = true,
                        }
                    }
                    if h264_failed {
                        let was_hw = h264_enc.as_ref().map(|e| e.is_hardware).unwrap_or(false);
                        h264_enc = None;
                        sent_h264 = false;
                        if was_hw && !h264_hw_broken {
                            // Retry right away with the software MFT before giving up on video.
                            h264_hw_broken = true;
                        } else {
                            h264_disabled_until = Instant::now() + Duration::from_secs(45);
                        }
                    }
                } else if h264_enc.is_some() {
                    h264_enc = None;
                }

                if !sent_h264 && !is_refine {
                    if let Some((ew, eh, payload)) =
                        encoder.encode_bgra(src_w, src_h, &bgra_buf, max_width, jpeg_quality)
                    {
                        let tile_label = "HybridTiles (LZ4+JPEG)";
                        if last_codec_label != tile_label {
                            last_codec_label = tile_label.to_string();
                            let _ = cursor_tx.try_send(DataPacket::Control(
                                ControlMessage::VideoCodecActive {
                                    codec_name: tile_label.into(),
                                    hardware: false,
                                },
                            ));
                        }
                        seq += 1;
                        let pkt = DataPacket::VideoFrame(VideoFrame {
                            seq,
                            timestamp_ms: unix_now_ms(),
                            width: ew,
                            height: eh,
                            encoding: FrameEncoding::HybridTiles,
                            payload,
                        });
                        if pkt_tx.try_send(pkt).is_ok() {
                            abr.lock().unwrap().on_frame_sent(seq);
                            last_video_sent = Instant::now();
                        } else {
                            encoder.force_keyframe();
                        }
                    }
                }
            }

            let elapsed = t0.elapsed();
            if elapsed < budget {
                std::thread::sleep(budget - elapsed);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_encoder_keyframe_skip_dirty_tile_and_downscale() {
        let w = 1920u32;
        let h = 1080u32;
        let mut frame = vec![24u8; (w * h * 4) as usize];
        for p in frame.chunks_exact_mut(4) {
            p[3] = 255;
        }

        let mut enc = HostStreamEncoder::new();
        let mut canvas = ViewerFrameCanvas::new();

        // Downscale 1920x1080 -> 1280x720
        let (ew, eh, key) = enc
            .encode_bgra(w, h, &frame, 1280, 72)
            .expect("keyframe should encode");
        assert_eq!((ew, eh), (1280, 720));

        let (cw, ch, out_bgra) = canvas
            .apply_frame(FrameEncoding::HybridTiles, ew, eh, &key)
            .unwrap();
        assert_eq!((cw, ch), (1280, 720));
        assert_eq!(out_bgra.len(), 1280 * 720 * 4);

        // Identical second frame produces None (0 bytes sent)
        assert!(enc.encode_bgra(w, h, &frame, 1280, 72).is_none());

        // Modify a small UI patch -> produces compact partial LZ4 tile update
        for y in 100..150usize {
            for x in 100..220usize {
                let off = (y * w as usize + x) * 4;
                frame[off] = 240;
                frame[off + 1] = 240;
                frame[off + 2] = 240;
                frame[off + 3] = 255;
            }
        }
        let (pw, ph, patch) = enc
            .encode_bgra(w, h, &frame, 1280, 72)
            .expect("dirty patch should encode");
        assert!(patch.len() < 4096, "expected tiny dirty tile payload, got {}", patch.len());

        let (_, _, updated) = canvas
            .apply_frame(FrameEncoding::HybridTiles, pw, ph, &patch)
            .unwrap();
        // Verify scaled coordinate around (100 * 2/3 ≈ 80, 120 * 2/3 ≈ 80) reflects bright pixel
        let check_off = (82usize * 1280 + 100usize) * 4;
        assert!(updated[check_off] > 200);
    }

    #[test]
    fn abr_controller_backs_off_on_queuing_delay_not_on_base_rtt() {
        let mut abr = AbrController::new();
        assert_eq!(abr.config.max_width, 0, "native resolution by default");
        assert!(abr.can_send_now());

        // Long but stable path (120 ms base RTT): quality must climb, not drop.
        for seq in 1..=40u64 {
            abr.on_frame_sent(seq);
            abr.last_adapt = Instant::now() - Duration::from_millis(200);
            abr.on_ack(seq, 120, 4);
        }
        assert!(abr.config.jpeg_quality > 72, "q={}", abr.config.jpeg_quality);
        assert_eq!(abr.config.max_width, 0);
        // Window grows with RTT so 120 ms paths still reach high fps.
        assert!(abr.window() >= 8);

        // Queue builds up (RTT jumps to 300 ms): quality backs off, resolution untouched.
        let before = abr.config.jpeg_quality;
        for seq in 41..=80u64 {
            abr.on_frame_sent(seq);
            abr.last_adapt = Instant::now() - Duration::from_millis(200);
            abr.on_ack(seq, 300, 4);
        }
        assert!(abr.config.jpeg_quality < before);
        assert_eq!(abr.config.max_width, 0);
    }

    #[test]
    fn abr_window_gate() {
        let mut abr = AbrController::new();
        abr.on_ack(0, 10, 1);
        let win = abr.window();
        for seq in 1..=win {
            assert!(abr.can_send_now());
            abr.on_frame_sent(seq);
        }
        assert!(!abr.can_send_now());
        abr.on_ack(win, 10, 1);
        assert!(abr.can_send_now());
    }

    #[test]
    fn h264_bitrate_scales_with_quality() {
        let hi = h264_bitrate_for(2560, 1440, 60, 88);
        let lo = h264_bitrate_for(2560, 1440, 60, 38);
        assert!(hi > 20_000_000 && hi <= 40_000_000, "{hi}");
        assert!(lo < hi / 2);
    }

    #[test]
    fn viewer_skips_h264_until_idr_after_loss() {
        let idr = [0u8, 0, 0, 1, 0x67, 0x42, 0, 0, 1, 0x65, 0x88];
        let p = [0u8, 0, 0, 1, 0x41, 0x9a];
        assert!(h264_annexb_has_idr(&idr));
        assert!(!h264_annexb_has_idr(&p));

        let mut c = ViewerFrameCanvas::new();
        let a = c.admit(1, FrameEncoding::H264AnnexB, &idr);
        assert!(a.decode && !a.request_keyframe);
        assert!(c.admit(2, FrameEncoding::H264AnnexB, &p).decode);
        // seq 3 lost
        let a = c.admit(4, FrameEncoding::H264AnnexB, &p);
        assert!(!a.decode && a.request_keyframe);
        let a = c.admit(5, FrameEncoding::H264AnnexB, &p);
        assert!(!a.decode && !a.request_keyframe, "rate limited");
        let a = c.admit(6, FrameEncoding::H264AnnexB, &idr);
        assert!(a.decode);
        assert!(c.admit(7, FrameEncoding::H264AnnexB, &p).decode);
        // late duplicate ignored
        assert!(!c.admit(6, FrameEncoding::H264AnnexB, &p).decode);
    }

    #[test]
    fn bilinear_downscale_keeps_thin_lines_visible() {
        // 1px white vertical lines every 4 px; nearest-neighbour at 0.625 drops some entirely.
        let (w, h) = (64u32, 8u32);
        let mut src = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            for x in (0..w as usize).step_by(4) {
                let o = (y * w as usize + x) * 4;
                src[o..o + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let mut out = Vec::new();
        scale_bgra_bilinear(w, h, &src, 40, 5, &mut out);
        // every 2.5 dst px there is a line: no 4-px stretch should be fully black
        let row = &out[0..40 * 4];
        for x in 0..37usize {
            let m = (0..4).map(|k| row[(x + k) * 4]).max().unwrap();
            assert!(m > 40, "line lost near x={x}");
        }
    }
}
