use agentdesk_protocol::{AudioCodec, AudioPacket, DataPacket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

const TARGET_SAMPLE_RATE: u32 = 24_000;
const SILENCE_PEAK_THRESHOLD: i16 = 48;

const INDEX_TABLE: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];
const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

fn encode_ima_nibble(sample: i16, predictor: &mut i32, index: &mut usize) -> u8 {
    let step = STEP_TABLE[*index];
    let mut diff = sample as i32 - *predictor;
    let mut code: u8 = 0;
    if diff < 0 {
        code = 8;
        diff = -diff;
    }
    let mut delta = step >> 3;
    if diff >= step {
        code |= 4;
        diff -= step;
        delta += step;
    }
    if diff >= (step >> 1) {
        code |= 2;
        diff -= step >> 1;
        delta += step >> 1;
    }
    if diff >= (step >> 2) {
        code |= 1;
        delta += step >> 2;
    }
    if (code & 8) != 0 {
        *predictor -= delta;
    } else {
        *predictor += delta;
    }
    *predictor = (*predictor).clamp(-32768, 32767);
    *index = ((*index as i32 + INDEX_TABLE[code as usize] as i32).clamp(0, 88)) as usize;
    code & 0x0F
}

fn decode_ima_nibble(nibble: u8, predictor: &mut i32, index: &mut usize) -> i16 {
    let step = STEP_TABLE[*index];
    let mut delta = step >> 3;
    if (nibble & 4) != 0 {
        delta += step;
    }
    if (nibble & 2) != 0 {
        delta += step >> 1;
    }
    if (nibble & 1) != 0 {
        delta += step >> 2;
    }
    if (nibble & 8) != 0 {
        *predictor -= delta;
    } else {
        *predictor += delta;
    }
    *predictor = (*predictor).clamp(-32768, 32767);
    *index = ((*index as i32 + INDEX_TABLE[(nibble & 0x0F) as usize] as i32).clamp(0, 88)) as usize;
    *predictor as i16
}

/// Encodes interleaved stereo `i16` PCM `[L, R, L, R, ...]` using 4-bit IMA-ADPCM (~4:1 ratio).
pub fn encode_ima_adpcm_stereo_i16(samples: &[i16]) -> Vec<u8> {
    let frames = samples.len() / 2;
    if frames == 0 {
        return Vec::new();
    }
    let mut pred_l = samples[0] as i32;
    let mut idx_l = 0usize;
    let mut pred_r = samples[1] as i32;
    let mut idx_r = 0usize;

    let mut out = Vec::with_capacity(8 + frames);
    out.extend_from_slice(&(pred_l as i16).to_le_bytes());
    out.push(idx_l as u8);
    out.push(0);
    out.extend_from_slice(&(pred_r as i16).to_le_bytes());
    out.push(idx_r as u8);
    out.push(0);

    for pair in samples[2..frames * 2].chunks_exact(2) {
        let nl = encode_ima_nibble(pair[0], &mut pred_l, &mut idx_l);
        let nr = encode_ima_nibble(pair[1], &mut pred_r, &mut idx_r);
        out.push(nl | (nr << 4));
    }
    out
}

pub fn decode_ima_adpcm_stereo_i16(payload: &[u8]) -> Vec<i16> {
    if payload.len() < 8 {
        return Vec::new();
    }
    let mut pred_l = i16::from_le_bytes([payload[0], payload[1]]) as i32;
    let mut idx_l = (payload[2] as usize).min(88);
    let mut pred_r = i16::from_le_bytes([payload[4], payload[5]]) as i32;
    let mut idx_r = (payload[6] as usize).min(88);

    let body = &payload[8..];
    let mut out = Vec::with_capacity((1 + body.len()) * 2);
    out.push(pred_l as i16);
    out.push(pred_r as i16);
    for &b in body {
        let sl = decode_ima_nibble(b & 0x0F, &mut pred_l, &mut idx_l);
        let sr = decode_ima_nibble((b >> 4) & 0x0F, &mut pred_r, &mut idx_r);
        out.push(sl);
        out.push(sr);
    }
    out
}

pub fn is_silence(samples: &[i16]) -> bool {
    let mut peak = 0i16;
    for &s in samples {
        let a = s.saturating_abs();
        if a > peak {
            peak = a;
            if peak > SILENCE_PEAK_THRESHOLD {
                return false;
            }
        }
    }
    true
}

/// Linearly resamples interleaved stereo `f32` frames between sample rates into stereo `i16`.
pub fn resample_stereo_f32_to_i16(src: &[f32], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    let src_frames = src.len() / 2;
    if src_frames == 0 || src_rate == 0 || dst_rate == 0 {
        return Vec::new();
    }
    if src_rate == dst_rate {
        return src[..src_frames * 2]
            .iter()
            .map(|&v| (v.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
    }
    let dst_frames = ((src_frames as u64 * dst_rate as u64) / src_rate as u64).max(1) as usize;
    let mut out = Vec::with_capacity(dst_frames * 2);
    let ratio = src_rate as f64 / dst_rate as f64;
    for i in 0..dst_frames {
        let pos = i as f64 * ratio;
        let idx = (pos as usize).min(src_frames - 1);
        let next = (idx + 1).min(src_frames - 1);
        let frac = (pos - idx as f64) as f32;
        let l = src[idx * 2] * (1.0 - frac) + src[next * 2] * frac;
        let r = src[idx * 2 + 1] * (1.0 - frac) + src[next * 2 + 1] * frac;
        out.push((l.clamp(-1.0, 1.0) * 32767.0) as i16);
        out.push((r.clamp(-1.0, 1.0) * 32767.0) as i16);
    }
    out
}

/// Resamples interleaved stereo `i16` to interleaved `f32` stereo.
pub fn resample_stereo_i16_to_f32(src: &[i16], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    let src_frames = src.len() / 2;
    if src_frames == 0 || src_rate == 0 || dst_rate == 0 {
        return Vec::new();
    }
    let dst_frames = if src_rate == dst_rate {
        src_frames
    } else {
        ((src_frames as u64 * dst_rate as u64) / src_rate as u64).max(1) as usize
    };
    let mut out = Vec::with_capacity(dst_frames * 2);
    let ratio = src_rate as f64 / dst_rate as f64;
    for i in 0..dst_frames {
        let pos = i as f64 * ratio;
        let idx = (pos as usize).min(src_frames - 1);
        let next = (idx + 1).min(src_frames - 1);
        let frac = (pos - idx as f64) as f32;
        let l0 = src[idx * 2] as f32 / 32768.0;
        let l1 = src[next * 2] as f32 / 32768.0;
        let r0 = src[idx * 2 + 1] as f32 / 32768.0;
        let r1 = src[next * 2 + 1] as f32 / 32768.0;
        out.push(l0 * (1.0 - frac) + l1 * frac);
        out.push(r0 * (1.0 - frac) + r1 * frac);
    }
    out
}

#[cfg(windows)]
mod win_audio {
    use super::*;
    use std::collections::VecDeque;
    use std::time::Duration;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient,
        IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVE_FORMAT_PCM,
    };
    use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
    use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };

    // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT = {00000003-0000-0010-8000-00AA00389B71}
    const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: windows::core::GUID =
        windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

    unsafe fn format_info(fmt: *const WAVEFORMATEX) -> (u32, usize, bool, u16) {
        let rate = (*fmt).nSamplesPerSec;
        let ch = (*fmt).nChannels.max(1) as usize;
        let bits = (*fmt).wBitsPerSample;
        let tag = (*fmt).wFormatTag;
        let is_float = if tag == WAVE_FORMAT_IEEE_FLOAT as u16 {
            true
        } else if tag == WAVE_FORMAT_EXTENSIBLE as u16 && (*fmt).cbSize >= 22 {
            let ext = &*(fmt as *const WAVEFORMATEXTENSIBLE);
            let sub = ext.SubFormat;
            sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            tag != WAVE_FORMAT_PCM as u16 && bits == 32
        };
        (rate, ch, is_float, bits)
    }

    pub fn spawn_capture(
        stop_rx: watch::Receiver<bool>,
        enabled: Arc<AtomicBool>,
        audio_tx: mpsc::Sender<DataPacket>,
    ) {
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let mut seq = 0u64;

            while !*stop_rx.borrow() {
                let mut retry_sleep = Duration::from_millis(800);
                if let Ok(enumerator) =
                    CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
                {
                    if let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
                        if let Ok(client) = device.Activate::<IAudioClient>(CLSCTX_ALL, None) {
                            if let Ok(fmt_ptr) = client.GetMixFormat() {
                                let (src_rate, src_ch, is_float, bits) = format_info(fmt_ptr);
                                let init_ok = client
                                    .Initialize(
                                        AUDCLNT_SHAREMODE_SHARED,
                                        AUDCLNT_STREAMFLAGS_LOOPBACK,
                                        400_000,
                                        0,
                                        fmt_ptr,
                                        None,
                                    )
                                    .is_ok();
                                CoTaskMemFree(Some(fmt_ptr as *const _));

                                if init_ok {
                                    if let Ok(cap) = client.GetService::<IAudioCaptureClient>() {
                                        if client.Start().is_ok() {
                                            let mut stereo_scratch = Vec::<f32>::with_capacity(2048);
                                            while !*stop_rx.borrow() {
                                                std::thread::sleep(Duration::from_millis(16));
                                                let mut next_len = match cap.GetNextPacketSize() {
                                                    Ok(n) => n,
                                                    Err(_) => break,
                                                };
                                                while next_len > 0 {
                                                    let mut data_ptr = std::ptr::null_mut();
                                                    let mut num_frames = 0u32;
                                                    let mut flags = 0u32;
                                                    if cap
                                                        .GetBuffer(
                                                            &mut data_ptr,
                                                            &mut num_frames,
                                                            &mut flags,
                                                            None,
                                                            None,
                                                        )
                                                        .is_err()
                                                    {
                                                        break;
                                                    }
                                                    let silent_flag = (flags & 0x2) != 0;
                                                    if enabled.load(Ordering::Relaxed)
                                                        && !silent_flag
                                                        && !data_ptr.is_null()
                                                        && num_frames > 0
                                                    {
                                                        stereo_scratch.clear();
                                                        let nf = num_frames as usize;
                                                        if is_float && bits == 32 {
                                                            let sl = std::slice::from_raw_parts(
                                                                data_ptr as *const f32,
                                                                nf * src_ch,
                                                            );
                                                            for f in 0..nf {
                                                                let l = sl[f * src_ch];
                                                                let r = if src_ch > 1 {
                                                                    sl[f * src_ch + 1]
                                                                } else {
                                                                    l
                                                                };
                                                                stereo_scratch.push(l);
                                                                stereo_scratch.push(r);
                                                            }
                                                        } else if bits == 16 {
                                                            let sl = std::slice::from_raw_parts(
                                                                data_ptr as *const i16,
                                                                nf * src_ch,
                                                            );
                                                            for f in 0..nf {
                                                                let l = sl[f * src_ch] as f32 / 32768.0;
                                                                let r = if src_ch > 1 {
                                                                    sl[f * src_ch + 1] as f32 / 32768.0
                                                                } else {
                                                                    l
                                                                };
                                                                stereo_scratch.push(l);
                                                                stereo_scratch.push(r);
                                                            }
                                                        }
                                                    }
                                                    let _ = cap.ReleaseBuffer(num_frames);

                                                    if !stereo_scratch.is_empty()
                                                        && enabled.load(Ordering::Relaxed)
                                                    {
                                                        let pcm = resample_stereo_f32_to_i16(
                                                            &stereo_scratch,
                                                            src_rate,
                                                            TARGET_SAMPLE_RATE,
                                                        );
                                                        if pcm.len() >= 16 && !is_silence(&pcm) {
                                                            let payload =
                                                                encode_ima_adpcm_stereo_i16(&pcm);
                                                            seq += 1;
                                                            let pkt = DataPacket::Audio(AudioPacket {
                                                                seq,
                                                                sample_rate: TARGET_SAMPLE_RATE,
                                                                channels: 2,
                                                                codec: AudioCodec::ImaAdpcm,
                                                                payload,
                                                            });
                                                            let _ = audio_tx.try_send(pkt);
                                                        }
                                                        stereo_scratch.clear();
                                                    }

                                                    next_len = match cap.GetNextPacketSize() {
                                                        Ok(n) => n,
                                                        Err(_) => break,
                                                    };
                                                }
                                            }
                                            let _ = client.Stop();
                                            retry_sleep = Duration::from_millis(250);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if !*stop_rx.borrow() {
                    std::thread::sleep(retry_sleep);
                }
            }
            CoUninitialize();
        });
    }

    pub fn spawn_player_thread(
        muted: Arc<AtomicBool>,
        stopped: Arc<AtomicBool>,
        rx: std::sync::mpsc::Receiver<AudioPacket>,
    ) {
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            while !stopped.load(Ordering::Relaxed) {
                let enumerator = match CoCreateInstance::<_, IMMDeviceEnumerator>(
                    &MMDeviceEnumerator,
                    None,
                    CLSCTX_ALL,
                ) {
                    Ok(e) => e,
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                };
                let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) else {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                };
                let Ok(client) = device.Activate::<IAudioClient>(CLSCTX_ALL, None) else {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                };
                let Ok(fmt_ptr) = client.GetMixFormat() else {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                };
                let (dst_rate, dst_ch, is_float, bits) = format_info(fmt_ptr);
                let init_ok = client
                    .Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 800_000, 0, fmt_ptr, None)
                    .is_ok();
                CoTaskMemFree(Some(fmt_ptr as *const _));
                if !init_ok {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                let Ok(buf_frames) = client.GetBufferSize() else {
                    continue;
                };
                let Ok(render) = client.GetService::<IAudioRenderClient>() else {
                    continue;
                };
                if client.Start().is_err() {
                    continue;
                }

                let mut queue = VecDeque::<f32>::with_capacity(48000);
                while !stopped.load(Ordering::Relaxed) {
                    match rx.recv_timeout(Duration::from_millis(10)) {
                        Ok(pkt) => {
                            if !muted.load(Ordering::Relaxed) {
                                let i16_pcm = decode_ima_adpcm_stereo_i16(&pkt.payload);
                                let f32_pcm =
                                    resample_stereo_i16_to_f32(&i16_pcm, pkt.sample_rate, dst_rate);
                                if queue.len() > (dst_rate as usize * 2 / 4) {
                                    queue.clear();
                                }
                                queue.extend(f32_pcm);
                            } else {
                                queue.clear();
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    while let Ok(pkt) = rx.try_recv() {
                        if !muted.load(Ordering::Relaxed) {
                            let i16_pcm = decode_ima_adpcm_stereo_i16(&pkt.payload);
                            let f32_pcm =
                                resample_stereo_i16_to_f32(&i16_pcm, pkt.sample_rate, dst_rate);
                            if queue.len() > (dst_rate as usize * 2 / 4) {
                                queue.clear();
                            }
                            queue.extend(f32_pcm);
                        }
                    }

                    let queued_frames = queue.len() / 2;
                    if queued_frames == 0 {
                        continue;
                    }
                    let Ok(padding) = client.GetCurrentPadding() else {
                        break;
                    };
                    let avail = buf_frames.saturating_sub(padding) as usize;
                    let write_frames = queued_frames.min(avail);
                    if write_frames == 0 {
                        continue;
                    }
                    let Ok(out_ptr) = render.GetBuffer(write_frames as u32) else {
                        break;
                    };
                    if is_float && bits == 32 {
                        let out_sl = std::slice::from_raw_parts_mut(
                            out_ptr as *mut f32,
                            write_frames * dst_ch,
                        );
                        for f in 0..write_frames {
                            let l = queue.pop_front().unwrap_or(0.0);
                            let r = queue.pop_front().unwrap_or(l);
                            out_sl[f * dst_ch] = l;
                            if dst_ch > 1 {
                                out_sl[f * dst_ch + 1] = r;
                            }
                            for c in 2..dst_ch {
                                out_sl[f * dst_ch + c] = 0.0;
                            }
                        }
                    } else if bits == 16 {
                        let out_sl = std::slice::from_raw_parts_mut(
                            out_ptr as *mut i16,
                            write_frames * dst_ch,
                        );
                        for f in 0..write_frames {
                            let l = (queue.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0) * 32767.0) as i16;
                            let r = (queue.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0) * 32767.0) as i16;
                            out_sl[f * dst_ch] = l;
                            if dst_ch > 1 {
                                out_sl[f * dst_ch + 1] = r;
                            }
                            for c in 2..dst_ch {
                                out_sl[f * dst_ch + c] = 0;
                            }
                        }
                    }
                    let _ = render.ReleaseBuffer(write_frames as u32, 0);
                }
                let _ = client.Stop();
            }
            CoUninitialize();
        });
    }
}

pub fn spawn_audio_capture_worker(
    stop_rx: watch::Receiver<bool>,
    enabled: Arc<AtomicBool>,
    audio_tx: mpsc::Sender<DataPacket>,
) {
    #[cfg(windows)]
    win_audio::spawn_capture(stop_rx, enabled, audio_tx);

    #[cfg(not(windows))]
    {
        let _ = (stop_rx, enabled, audio_tx);
    }
}

pub struct AudioPlayer {
    tx: SyncSender<AudioPacket>,
    muted: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}

impl AudioPlayer {
    pub fn new() -> Self {
        let (tx, rx) = sync_channel::<AudioPacket>(8);
        let muted = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));

        #[cfg(windows)]
        win_audio::spawn_player_thread(muted.clone(), stopped.clone(), rx);
        #[cfg(not(windows))]
        let _ = rx;

        Self { tx, muted, stopped }
    }

    pub fn set_muted(&self, mute: bool) {
        self.muted.store(mute, Ordering::Relaxed);
    }

    pub fn push(&self, pkt: AudioPacket) {
        if !self.muted.load(Ordering::Relaxed) {
            let _ = self.tx.try_send(pkt);
        }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ima_adpcm_stereo_roundtrip_preserves_waveform() {
        let mut pcm = Vec::with_capacity(960);
        for i in 0..480 {
            let t = i as f32 * 0.12;
            let l = (t.sin() * 14000.0) as i16;
            let r = ((t * 0.5).cos() * 10000.0) as i16;
            pcm.push(l);
            pcm.push(r);
        }
        assert!(!is_silence(&pcm));
        let enc = encode_ima_adpcm_stereo_i16(&pcm);
        assert!(enc.len() < pcm.len()); // ~4x smaller than i16 bytes
        let dec = decode_ima_adpcm_stereo_i16(&enc);
        assert_eq!(dec.len(), pcm.len());
        let max_steady_err = pcm[40..]
            .iter()
            .zip(dec[40..].iter())
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap();
        assert!(
            max_steady_err < 1600,
            "unexpected steady-state ADPCM distortion: {max_steady_err}"
        );
    }
}
