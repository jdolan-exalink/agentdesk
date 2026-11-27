/// Integer BT.601 BGRA -> NV12 conversion (even dimensions required).
pub fn bgra_to_nv12(bgra: &[u8], w: u32, h: u32, out: &mut Vec<u8>) {
    let w = (w & !1) as usize;
    let h = (h & !1) as usize;
    if w == 0 || h == 0 || bgra.len() < w * h * 4 {
        out.clear();
        return;
    }
    let y_len = w * h;
    let uv_len = y_len / 2;
    out.resize(y_len + uv_len, 0);
    let (y_plane, uv_plane) = out.split_at_mut(y_len);

    for row in 0..h {
        let src_row = &bgra[row * w * 4..(row + 1) * w * 4];
        let y_row = &mut y_plane[row * w..(row + 1) * w];
        for col in 0..w {
            let p = col * 4;
            let b = src_row[p] as i32;
            let g = src_row[p + 1] as i32;
            let r = src_row[p + 2] as i32;
            let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            y_row[col] = y.clamp(16, 235) as u8;
        }
    }

    for row2 in 0..(h / 2) {
        let r0 = &bgra[(row2 * 2) * w * 4..(row2 * 2 + 1) * w * 4];
        let r1 = &bgra[(row2 * 2 + 1) * w * 4..(row2 * 2 + 2) * w * 4];
        let uv_row = &mut uv_plane[row2 * w..(row2 + 1) * w];
        for col2 in 0..(w / 2) {
            let p0 = col2 * 8;
            let b = (r0[p0] as i32 + r0[p0 + 4] as i32 + r1[p0] as i32 + r1[p0 + 4] as i32 + 2) >> 2;
            let g = (r0[p0 + 1] as i32
                + r0[p0 + 5] as i32
                + r1[p0 + 1] as i32
                + r1[p0 + 5] as i32
                + 2)
                >> 2;
            let r = (r0[p0 + 2] as i32
                + r0[p0 + 6] as i32
                + r1[p0 + 2] as i32
                + r1[p0 + 6] as i32
                + 2)
                >> 2;
            let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
            let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
            uv_row[col2 * 2] = u.clamp(16, 240) as u8;
            uv_row[col2 * 2 + 1] = v.clamp(16, 240) as u8;
        }
    }
}

/// Integer BT.601 NV12 -> BGRA conversion.
pub fn nv12_to_bgra(
    nv12: &[u8],
    w: u32,
    h: u32,
    stride_y: usize,
    stride_uv: usize,
    out: &mut Vec<u8>,
) {
    let w = w as usize;
    let h = h as usize;
    if w == 0 || h == 0 || nv12.len() < stride_y * h + stride_uv * (h / 2) {
        out.clear();
        return;
    }
    let uv_base = stride_y * h;
    out.resize(w * h * 4, 255);
    for row in 0..h {
        let y_row = &nv12[row * stride_y..row * stride_y + w];
        let uv_row = &nv12[uv_base + (row / 2) * stride_uv..uv_base + (row / 2) * stride_uv + w];
        let dst = &mut out[row * w * 4..(row + 1) * w * 4];
        for col in 0..w {
            let y = (y_row[col] as i32 - 16).max(0);
            let uv_idx = col & !1;
            let d = uv_row[uv_idx] as i32 - 128;
            let e = uv_row[uv_idx + 1] as i32 - 128;
            let c298 = 298 * y;
            let r = (c298 + 409 * e + 128) >> 8;
            let g = (c298 - 100 * d - 208 * e + 128) >> 8;
            let b = (c298 + 516 * d + 128) >> 8;
            let p = col * 4;
            dst[p] = b.clamp(0, 255) as u8;
            dst[p + 1] = g.clamp(0, 255) as u8;
            dst[p + 2] = r.clamp(0, 255) as u8;
            dst[p + 3] = 255;
        }
    }
}

#[cfg(windows)]
mod win_mf {
    use super::*;
    use std::sync::Once;
    use std::time::{Duration, Instant};
    use windows::core::{Interface, GUID, VARIANT};
    use windows::Win32::Media::MediaFoundation::{
        eAVEncH264VProfile_High, ICodecAPI, IMFActivate, IMFMediaEventGenerator, IMFSample,
        IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFStartup, MFTEnumEx, MFVideoFormat_H264, MFVideoFormat_NV12,
        MFVideoInterlace_Progressive, CODECAPI_AVEncCommonMeanBitRate,
        CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVDefaultBPictureCount,
        CODECAPI_AVEncCommonBufferSize, METransformHaveOutput, MF_EVENT_FLAG_NO_WAIT,
        MF_E_NO_EVENTS_AVAILABLE,
        CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode,
        METransformNeedInput, MFT_CATEGORY_VIDEO_DECODER, MFT_CATEGORY_VIDEO_ENCODER,
        MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_LOCALMFT,
        MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_FRIENDLY_NAME_Attribute,
        MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
        MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES,
        MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MF_E_TRANSFORM_NEED_MORE_INPUT,
        MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
        MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
        MF_MT_MPEG2_PROFILE, MF_MT_SUBTYPE, MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK,
        MF_VERSION, MFSTARTUP_NOSOCKET,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

    static MF_ONCE: Once = Once::new();

    fn ensure_mf() {
        MF_ONCE.call_once(|| unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
        });
    }

    #[inline]
    fn pack_u32_pair(hi: u32, lo: u32) -> u64 {
        ((hi as u64) << 32) | (lo as u64)
    }

    #[inline]
    fn unpack_u32_pair(v: u64) -> (u32, u32) {
        ((v >> 32) as u32, (v & 0xFFFF_FFFF) as u32)
    }

    unsafe fn friendly_name(act: &IMFActivate) -> String {
        let mut pw = windows::core::PWSTR::null();
        let mut cch = 0u32;
        if act
            .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut pw, &mut cch)
            .is_ok()
            && !pw.is_null()
        {
            let s = pw.to_string().unwrap_or_default();
            CoTaskMemFree(Some(pw.0 as *const _));
            if !s.is_empty() {
                return s;
            }
        }
        "H.264 MFT".into()
    }

    unsafe fn enum_mfts(
        category: GUID,
        flags: windows::Win32::Media::MediaFoundation::MFT_ENUM_FLAG,
        input: Option<MFT_REGISTER_TYPE_INFO>,
        output: Option<MFT_REGISTER_TYPE_INFO>,
    ) -> Vec<Option<IMFActivate>> {
        let mut ptr = std::ptr::null_mut();
        let mut count = 0u32;
        if MFTEnumEx(
            category,
            flags,
            input.as_ref().map(|x| x as *const _),
            output.as_ref().map(|x| x as *const _),
            &mut ptr,
            &mut count,
        )
        .is_err()
            || ptr.is_null()
        {
            return Vec::new();
        }
        let slice = std::slice::from_raw_parts_mut(ptr, count as usize);
        let mut out = Vec::with_capacity(count as usize);
        for slot in slice.iter_mut() {
            out.push(slot.take());
        }
        CoTaskMemFree(Some(ptr as *const _));
        out
    }

    unsafe fn try_unlock_async(mft: &IMFTransform) -> Option<IMFMediaEventGenerator> {
        let Ok(attrs) = mft.GetAttributes() else {
            return None;
        };
        let is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
        if is_async {
            let _ = attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1);
            mft.cast::<IMFMediaEventGenerator>().ok()
        } else {
            None
        }
    }

    pub struct MfH264Encoder {
        mft: IMFTransform,
        ev_gen: Option<IMFMediaEventGenerator>,
        codec_api: Option<ICodecAPI>,
        width: u32,
        height: u32,
        fps: u32,
        frame_idx: u64,
        nv12_buf: Vec<u8>,
        out_provides_samples: bool,
        /// Async (hardware) MFTs only accept input after a METransformNeedInput event.
        pending_inputs: u32,
        bitrate: u32,
        started: Instant,
        last_ts: i64,
        pub codec_name: String,
        pub is_hardware: bool,
    }

    // COM is initialized with COINIT_MULTITHREADED; exclusive &mut self ownership is Send-safe.
    unsafe impl Send for MfH264Encoder {}

    /// VBV buffer of ~1.5 frames: keeps every frame close to the average size so a single frame
    /// never takes several RTTs to drain (the main source of "rubber band" latency).
    fn low_latency_vbv(bitrate_bps: u32, fps: u32) -> u32 {
        ((bitrate_bps as u64 * 3 / 2) / fps.max(1) as u64).clamp(64_000, 8_000_000) as u32
    }

    impl MfH264Encoder {
        pub fn try_new(width: u32, height: u32, fps: u32, bitrate_bps: u32) -> Option<Self> {
            Self::try_new_with(width, height, fps, bitrate_bps, true, true)
        }

        /// `allow_hw` / `allow_sw` let the caller retry with the Microsoft software MFT when a
        /// hardware encoder was created but then failed at runtime.
        pub fn try_new_with(
            width: u32,
            height: u32,
            fps: u32,
            bitrate_bps: u32,
            allow_hw: bool,
            allow_sw: bool,
        ) -> Option<Self> {
            ensure_mf();
            let w = width & !1;
            let h = height & !1;
            if w < 64 || h < 64 {
                return None;
            }
            let out_info = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_H264,
            };
            unsafe {
                let hw_acts = enum_mfts(
                    MFT_CATEGORY_VIDEO_ENCODER,
                    MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
                    None,
                    Some(out_info),
                );
                let sw_acts = enum_mfts(
                    MFT_CATEGORY_VIDEO_ENCODER,
                    MFT_ENUM_FLAG_SYNCMFT
                        | MFT_ENUM_FLAG_ASYNCMFT
                        | MFT_ENUM_FLAG_LOCALMFT
                        | MFT_ENUM_FLAG_SORTANDFILTER,
                    None,
                    Some(out_info),
                );
                let hw_acts = if allow_hw { hw_acts } else { Vec::new() };
                let sw_acts = if allow_sw { sw_acts } else { Vec::new() };
                let mut candidates = Vec::new();
                for a in hw_acts {
                    if let Some(act) = a {
                        candidates.push((act, true));
                    }
                }
                for a in sw_acts {
                    if let Some(act) = a {
                        candidates.push((act, false));
                    }
                }

                for (act, is_hw) in candidates {
                    let name = friendly_name(&act);
                    let Ok(mft) = act.ActivateObject::<IMFTransform>() else {
                        continue;
                    };
                    let ev_gen = try_unlock_async(&mft);

                    let Ok(out_mt) = MFCreateMediaType() else {
                        continue;
                    };
                    let _ = out_mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
                    let _ = out_mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264);
                    let _ = out_mt.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_bps);
                    let _ = out_mt.SetUINT64(&MF_MT_FRAME_SIZE, pack_u32_pair(w, h));
                    let _ = out_mt.SetUINT64(&MF_MT_FRAME_RATE, pack_u32_pair(fps.max(1), 1));
                    let _ = out_mt.SetUINT32(
                        &MF_MT_INTERLACE_MODE,
                        MFVideoInterlace_Progressive.0 as u32,
                    );
                    let _ = out_mt.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32);
                    if mft.SetOutputType(0, &out_mt, 0).is_err() {
                        continue;
                    }

                    let Ok(in_mt) = MFCreateMediaType() else {
                        continue;
                    };
                    let _ = in_mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
                    let _ = in_mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12);
                    let _ = in_mt.SetUINT64(&MF_MT_FRAME_SIZE, pack_u32_pair(w, h));
                    let _ = in_mt.SetUINT64(&MF_MT_FRAME_RATE, pack_u32_pair(fps.max(1), 1));
                    let _ = in_mt.SetUINT32(
                        &MF_MT_INTERLACE_MODE,
                        MFVideoInterlace_Progressive.0 as u32,
                    );
                    if mft.SetInputType(0, &in_mt, 0).is_err() {
                        continue;
                    }

                    let codec_api = mft.cast::<ICodecAPI>().ok();
                    if let Some(ref capi) = codec_api {
                        let v_true = VARIANT::from(true);
                        let _ = capi.SetValue(&CODECAPI_AVLowLatencyMode, &v_true);
                        // eAVEncCommonRateControlMode_CBR = 0. Hardware encoders honour CBR +
                        // a small VBV best in low-latency mode (2 was UnconstrainedVBR).
                        let v_rc = VARIANT::from(0u32);
                        let _ = capi.SetValue(&CODECAPI_AVEncCommonRateControlMode, &v_rc);
                        let v_br = VARIANT::from(bitrate_bps);
                        let _ = capi.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &v_br);
                        let v_vbv = VARIANT::from(low_latency_vbv(bitrate_bps, fps));
                        let _ = capi.SetValue(&CODECAPI_AVEncCommonBufferSize, &v_vbv);
                        // Long GOP: IDR frames are big and slow; the viewer asks for one when it
                        // detects a lost frame, so periodic keyframes are only a safety net.
                        let v_gop = VARIANT::from(fps.max(1) * 10);
                        let _ = capi.SetValue(&CODECAPI_AVEncMPVGOPSize, &v_gop);
                        let v_b = VARIANT::from(0u32);
                        let _ = capi.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &v_b);
                    }

                    let stream_info = mft.GetOutputStreamInfo(0).unwrap_or_default();
                    let out_provides = (stream_info.dwFlags
                        & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                            | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32))
                        != 0;

                    if mft
                        .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                        .is_err()
                    {
                        continue;
                    }
                    let _ = mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);

                    return Some(Self {
                        mft,
                        ev_gen,
                        codec_api,
                        width: w,
                        height: h,
                        fps: fps.max(1),
                        frame_idx: 0,
                        nv12_buf: Vec::new(),
                        out_provides_samples: out_provides,
                        pending_inputs: 0,
                        bitrate: bitrate_bps,
                        started: Instant::now(),
                        last_ts: -1,
                        codec_name: if is_hw {
                            format!("HW · {name}")
                        } else {
                            name
                        },
                        is_hardware: is_hw,
                    });
                }
            }
            None
        }

        pub fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        pub fn bitrate(&self) -> u32 {
            self.bitrate
        }

        /// Live bitrate change without recreating the encoder (no IDR, no resolution change).
        pub fn set_bitrate(&mut self, bitrate_bps: u32) {
            if bitrate_bps == self.bitrate {
                return;
            }
            self.bitrate = bitrate_bps;
            if let Some(ref capi) = self.codec_api {
                unsafe {
                    let _ = capi.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT::from(bitrate_bps));
                    let _ = capi.SetValue(
                        &CODECAPI_AVEncCommonBufferSize,
                        &VARIANT::from(low_latency_vbv(bitrate_bps, self.fps)),
                    );
                }
            }
        }

        pub fn encode_bgra(
            &mut self,
            bgra: &[u8],
            w: u32,
            h: u32,
            force_keyframe: bool,
        ) -> Result<Option<Vec<u8>>, String> {
            let ew = w & !1;
            let eh = h & !1;
            if ew != self.width || eh != self.height {
                return Err("dimension mismatch".into());
            }

            unsafe {
                let mut encoded = Vec::new();

                if self.ev_gen.is_some() {
                    // Async hardware MFT (NVENC / QuickSync / AMF): collect any finished output
                    // and wait (bounded) for the encoder to ask for input.
                    self.pump_async_events(&mut encoded, Duration::from_millis(12), true, false)?;
                    if self.pending_inputs == 0 {
                        // Encoder still busy: skip this capture rather than queueing latency.
                        return Ok(if encoded.is_empty() { None } else { Some(encoded) });
                    }
                }

                bgra_to_nv12(bgra, ew, eh, &mut self.nv12_buf);
                if self.nv12_buf.is_empty() {
                    return Ok(None);
                }

                if force_keyframe {
                    if let Some(ref capi) = self.codec_api {
                        let v = VARIANT::from(1u32);
                        let _ = capi.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &v);
                    }
                }

                let sample: IMFSample = MFCreateSample().map_err(|e| e.to_string())?;
                let buf = MFCreateMemoryBuffer(self.nv12_buf.len() as u32)
                    .map_err(|e| e.to_string())?;
                let mut ptr = std::ptr::null_mut();
                buf.Lock(&mut ptr, None, None).map_err(|e| e.to_string())?;
                std::ptr::copy_nonoverlapping(self.nv12_buf.as_ptr(), ptr, self.nv12_buf.len());
                buf.Unlock().map_err(|e| e.to_string())?;
                buf.SetCurrentLength(self.nv12_buf.len() as u32)
                    .map_err(|e| e.to_string())?;
                sample.AddBuffer(&buf).map_err(|e| e.to_string())?;

                // Wall-clock timestamps (100 ns units): frames only exist when the screen changes,
                // so a fixed 1/fps step would make CBR rate control under-spend the bitrate.
                let min_step = 10_000_000i64 / 120;
                let now_ts = self.started.elapsed().as_nanos() as i64 / 100;
                let ts = now_ts.max(self.last_ts + min_step);
                let dur = if self.last_ts < 0 {
                    10_000_000i64 / self.fps as i64
                } else {
                    (ts - self.last_ts).clamp(min_step, 10_000_000)
                };
                self.last_ts = ts;
                self.frame_idx += 1;
                let _ = sample.SetSampleTime(ts);
                let _ = sample.SetSampleDuration(dur);

                self.mft
                    .ProcessInput(0, &sample, 0)
                    .map_err(|e| e.to_string())?;

                if self.ev_gen.is_some() {
                    self.pending_inputs -= 1;
                    // Hardware encoders finish a frame in 2-8 ms; wait for it so the frame goes
                    // out now instead of one capture interval later.
                    self.pump_async_events(&mut encoded, Duration::from_millis(14), false, true)?;
                } else {
                    for _ in 0..8 {
                        match self.process_output_once()? {
                            Some(chunk) => encoded.extend_from_slice(&chunk),
                            None => break,
                        }
                    }
                }

                if encoded.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(encoded))
                }
            }
        }

        /// Drains the async MFT event queue. Every METransformHaveOutput yields exactly one
        /// ProcessOutput call; every METransformNeedInput grants one ProcessInput.
        /// Returns early when `stop_on_input` and input is allowed, or when `stop_on_output`
        /// and at least one new output was collected; otherwise polls until `max_wait`.
        unsafe fn pump_async_events(
            &mut self,
            out: &mut Vec<u8>,
            max_wait: Duration,
            stop_on_input: bool,
            stop_on_output: bool,
        ) -> Result<(), String> {
            let Some(eg) = self.ev_gen.clone() else {
                return Ok(());
            };
            let deadline = Instant::now() + max_wait;
            let mut got_output = false;
            loop {
                match eg.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                    Ok(ev) => {
                        let t = ev.GetType().unwrap_or(0);
                        if t == METransformNeedInput.0 as u32 {
                            self.pending_inputs = self.pending_inputs.saturating_add(1);
                        } else if t == METransformHaveOutput.0 as u32 {
                            if let Some(chunk) = self.process_output_once()? {
                                out.extend_from_slice(&chunk);
                                got_output = true;
                            }
                        }
                        continue;
                    }
                    Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {}
                    Err(e) => return Err(e.to_string()),
                }
                if (stop_on_input && self.pending_inputs > 0)
                    || (stop_on_output && got_output)
                    || Instant::now() >= deadline
                {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_micros(500));
            }
        }

        unsafe fn process_output_once(&mut self) -> Result<Option<Vec<u8>>, String> {
            for _ in 0..3 {
                let mut out_sample_holder: Option<IMFSample> = None;
                if !self.out_provides_samples {
                    let os = MFCreateSample().map_err(|e| e.to_string())?;
                    let ob = MFCreateMemoryBuffer((self.width * self.height).max(65536))
                        .map_err(|e| e.to_string())?;
                    let _ = os.AddBuffer(&ob);
                    out_sample_holder = Some(os);
                }
                let mut out_buf = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(out_sample_holder),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let hr = self.mft.ProcessOutput(0, &mut out_buf, &mut status);
                let got_sample = std::mem::ManuallyDrop::into_inner(std::ptr::read(&out_buf[0].pSample));
                let _ = std::mem::ManuallyDrop::into_inner(std::ptr::read(&out_buf[0].pEvents));
                if let Err(err) = hr {
                    if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                        return Ok(None);
                    }
                    if err.code() == MF_E_TRANSFORM_STREAM_CHANGE {
                        if let Ok(mt) = self.mft.GetOutputAvailableType(0, 0) {
                            let _ = self.mft.SetOutputType(0, &mt, 0);
                        }
                        let stream_info = self.mft.GetOutputStreamInfo(0).unwrap_or_default();
                        self.out_provides_samples = (stream_info.dwFlags
                            & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                                | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32))
                            != 0;
                        continue;
                    }
                    return Err(err.to_string());
                }
                let Some(s) = got_sample else {
                    return Ok(None);
                };
                let mut bytes = Vec::new();
                if let Ok(flat) = s.ConvertToContiguousBuffer() {
                    let mut p = std::ptr::null_mut();
                    let mut len = 0u32;
                    if flat.Lock(&mut p, None, Some(&mut len)).is_ok() {
                        if !p.is_null() && len > 0 {
                            bytes.extend_from_slice(std::slice::from_raw_parts(p, len as usize));
                        }
                        let _ = flat.Unlock();
                    }
                }
                return Ok(Some(bytes));
            }
            Ok(None)
        }
    }
    pub struct MfH264Decoder {
        mft: IMFTransform,
        width: u32,
        height: u32,
        stride: usize,
        out_provides_samples: bool,
        bgra_out: Vec<u8>,
        frame_idx: u64,
    }

    // COM is initialized with COINIT_MULTITHREADED; exclusive &mut self ownership is Send-safe.
    unsafe impl Send for MfH264Decoder {}

    impl MfH264Decoder {
        pub fn try_new() -> Option<Self> {
            ensure_mf();
            let in_info = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_H264,
            };
            unsafe {
                let acts = enum_mfts(
                    MFT_CATEGORY_VIDEO_DECODER,
                    MFT_ENUM_FLAG_SYNCMFT
                        | MFT_ENUM_FLAG_LOCALMFT
                        | MFT_ENUM_FLAG_SORTANDFILTER,
                    Some(in_info),
                    None,
                );
                for a in acts.into_iter().flatten() {
                    let Ok(mft) = a.ActivateObject::<IMFTransform>() else {
                        continue;
                    };
                    if let Ok(capi) = mft.cast::<ICodecAPI>() {
                        let _ = capi.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true));
                    }
                    let Ok(in_mt) = MFCreateMediaType() else {
                        continue;
                    };
                    let _ = in_mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
                    let _ = in_mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264);
                    if mft.SetInputType(0, &in_mt, 0).is_err() {
                        continue;
                    }

                    let mut out_ok = false;
                    for i in 0..16u32 {
                        let Ok(cand) = mft.GetOutputAvailableType(0, i) else {
                            break;
                        };
                        if cand.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) {
                            if mft.SetOutputType(0, &cand, 0).is_ok() {
                                out_ok = true;
                                break;
                            }
                        }
                    }
                    if !out_ok {
                        continue;
                    }

                    let stream_info = mft.GetOutputStreamInfo(0).unwrap_or_default();
                    let out_provides = (stream_info.dwFlags
                        & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                            | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32))
                        != 0;
                    let _ = mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
                    let _ = mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);

                    return Some(Self {
                        mft,
                        width: 0,
                        height: 0,
                        stride: 0,
                        out_provides_samples: out_provides,
                        bgra_out: Vec::new(),
                        frame_idx: 0,
                    });
                }
            }
            None
        }

        pub fn decode_to_bgra(
            &mut self,
            annex_b: &[u8],
            hint_w: u32,
            hint_h: u32,
        ) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
            if annex_b.is_empty() {
                return Ok(None);
            }
            unsafe {
                let sample: IMFSample = MFCreateSample().map_err(|e| e.to_string())?;
                let buf = MFCreateMemoryBuffer(annex_b.len() as u32).map_err(|e| e.to_string())?;
                let mut ptr = std::ptr::null_mut();
                buf.Lock(&mut ptr, None, None).map_err(|e| e.to_string())?;
                std::ptr::copy_nonoverlapping(annex_b.as_ptr(), ptr, annex_b.len());
                buf.Unlock().map_err(|e| e.to_string())?;
                buf.SetCurrentLength(annex_b.len() as u32)
                    .map_err(|e| e.to_string())?;
                sample.AddBuffer(&buf).map_err(|e| e.to_string())?;

                let dur = 333_333i64;
                let _ = sample.SetSampleTime(self.frame_idx as i64 * dur);
                let _ = sample.SetSampleDuration(dur);
                self.frame_idx += 1;

                self.mft
                    .ProcessInput(0, &sample, 0)
                    .map_err(|e| e.to_string())?;

                let mut last_frame = None;
                for _ in 0..8 {
                    let alloc_w = self.width.max(hint_w).max(64);
                    let alloc_h = self.height.max(hint_h).max(64);
                    let mut out_sample_holder: Option<IMFSample> = None;
                    if !self.out_provides_samples {
                        let os = MFCreateSample().map_err(|e| e.to_string())?;
                        let ob = MFCreateMemoryBuffer(alloc_w * alloc_h * 2 + 4096)
                            .map_err(|e| e.to_string())?;
                        let _ = os.AddBuffer(&ob);
                        out_sample_holder = Some(os);
                    }
                    let mut out_buf = [MFT_OUTPUT_DATA_BUFFER {
                        dwStreamID: 0,
                        pSample: std::mem::ManuallyDrop::new(out_sample_holder),
                        dwStatus: 0,
                        pEvents: std::mem::ManuallyDrop::new(None),
                    }];
                    let mut status = 0u32;
                    let hr = self.mft.ProcessOutput(0, &mut out_buf, &mut status);
                    let got_sample = std::mem::ManuallyDrop::into_inner(std::ptr::read(&out_buf[0].pSample));
                    let _ = std::mem::ManuallyDrop::into_inner(std::ptr::read(&out_buf[0].pEvents));

                    if let Err(err) = hr {
                        if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                            break;
                        }
                        if err.code() == MF_E_TRANSFORM_STREAM_CHANGE {
                            for i in 0..16u32 {
                                let Ok(cand) = self.mft.GetOutputAvailableType(0, i) else {
                                    break;
                                };
                                if cand.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) {
                                    if self.mft.SetOutputType(0, &cand, 0).is_ok() {
                                        if let Ok(packed) = cand.GetUINT64(&MF_MT_FRAME_SIZE) {
                                            let (fw, fh) = unpack_u32_pair(packed);
                                            if fw > 0 && fh > 0 {
                                                self.width = fw;
                                                self.height = fh;
                                            }
                                        }
                                        if let Ok(st) = cand.GetUINT32(&MF_MT_DEFAULT_STRIDE) {
                                            self.stride = st as usize;
                                        }
                                        let stream_info =
                                            self.mft.GetOutputStreamInfo(0).unwrap_or_default();
                                        self.out_provides_samples = (stream_info.dwFlags
                                            & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                                                | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32))
                                            != 0;
                                        break;
                                    }
                                }
                            }
                            continue;
                        }
                        return Err(err.to_string());
                    }

                    if let Some(s) = got_sample {
                        if self.width == 0 || self.height == 0 {
                            if let Ok(cur) = self.mft.GetOutputCurrentType(0) {
                                if let Ok(packed) = cur.GetUINT64(&MF_MT_FRAME_SIZE) {
                                    let (fw, fh) = unpack_u32_pair(packed);
                                    if fw > 0 && fh > 0 {
                                        self.width = fw;
                                        self.height = fh;
                                    }
                                }
                                if let Ok(st) = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE) {
                                    self.stride = st as usize;
                                }
                            }
                        }
                        let w = self.width.max(hint_w);
                        let h = self.height.max(hint_h);
                        let stride = if self.stride >= w as usize {
                            self.stride
                        } else {
                            w as usize
                        };
                        if let Ok(flat) = s.ConvertToContiguousBuffer() {
                            let mut p = std::ptr::null_mut();
                            let mut len = 0u32;
                            if flat.Lock(&mut p, None, Some(&mut len)).is_ok() {
                                if !p.is_null() && len > 0 {
                                    let sl = std::slice::from_raw_parts(p, len as usize);
                                    let draw_w = hint_w.min(w).max(2);
                                    let draw_h = hint_h.min(h).max(2);
                                    nv12_to_bgra(sl, draw_w, draw_h, stride, stride, &mut self.bgra_out);
                                    if !self.bgra_out.is_empty() {
                                        last_frame = Some((draw_w, draw_h, self.bgra_out.clone()));
                                    }
                                }
                                let _ = flat.Unlock();
                            }
                        }
                    } else {
                        break;
                    }
                }
                Ok(last_frame)
            }
        }
    }
}

#[cfg(not(windows))]
mod win_mf {
    pub struct MfH264Encoder {
        pub codec_name: String,
        pub is_hardware: bool,
    }

    impl MfH264Encoder {
        pub fn try_new(_width: u32, _height: u32, _fps: u32, _bitrate_bps: u32) -> Option<Self> {
            None
        }

        pub fn try_new_with(
            _width: u32,
            _height: u32,
            _fps: u32,
            _bitrate_bps: u32,
            _allow_hw: bool,
            _allow_sw: bool,
        ) -> Option<Self> {
            None
        }

        pub fn dimensions(&self) -> (u32, u32) {
            (0, 0)
        }

        pub fn bitrate(&self) -> u32 {
            0
        }

        pub fn set_bitrate(&mut self, _bitrate_bps: u32) {}

        pub fn encode_bgra(
            &mut self,
            _bgra: &[u8],
            _w: u32,
            _h: u32,
            _force_keyframe: bool,
        ) -> Result<Option<Vec<u8>>, String> {
            Err("H.264 MFT not available on non-Windows".into())
        }
    }

    pub struct MfH264Decoder;

    impl MfH264Decoder {
        pub fn try_new() -> Option<Self> {
            None
        }

        pub fn decode_to_bgra(
            &mut self,
            _annex_b: &[u8],
            _hint_w: u32,
            _hint_h: u32,
        ) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
            Err("H.264 MFT not available on non-Windows".into())
        }
    }
}

pub use win_mf::{MfH264Decoder, MfH264Encoder};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_nv12_roundtrip_preserves_colors() {
        let w = 16u32;
        let h = 16u32;
        let mut src = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let b = ((x * 15) & 0xFF) as u8;
                let g = ((y * 15) & 0xFF) as u8;
                let r = 180u8;
                src.extend_from_slice(&[b, g, r, 255]);
            }
        }
        let mut nv12 = Vec::new();
        bgra_to_nv12(&src, w, h, &mut nv12);
        assert_eq!(nv12.len(), (w * h * 3 / 2) as usize);

        let mut dst = Vec::new();
        nv12_to_bgra(&nv12, w, h, w as usize, w as usize, &mut dst);
        assert_eq!(dst.len(), src.len());

        let center = ((8 * w + 8) * 4) as usize;
        assert!((dst[center + 2] as i32 - src[center + 2] as i32).abs() < 12);
    }
}
