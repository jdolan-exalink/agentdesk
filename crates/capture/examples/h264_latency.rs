//! Measures frame delay through the real encoder + decoder: does frame N come out of the
//! decoder when frame N goes in, or only after later frames arrive?
//! `cargo run --release -p agentdesk-capture --example h264_latency`
use agentdesk_capture::{MfH264Decoder, MfH264Encoder};
use std::time::{Duration, Instant};

fn frame(w: u32, h: u32, shade: u8) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * 4) as usize];
    for px in v.chunks_exact_mut(4) {
        px[0] = shade;
        px[1] = shade;
        px[2] = shade;
        px[3] = 255;
    }
    v
}

fn mean(b: &[u8]) -> u32 {
    let n = (b.len() / 4).max(1) as u64;
    (b.chunks_exact(4).map(|p| p[1] as u64).sum::<u64>() / n) as u32
}

fn main() {
    let (w, h) = (1280u32, 1024u32);
    let mut enc = MfH264Encoder::try_new(w, h, 60, 8_000_000).expect("encoder");
    let mut dec = MfH264Decoder::try_new().expect("decoder");
    for i in 0..80u32 {
        let shade = (10 + (i * 3) % 240) as u8;
        let t = Instant::now();
        let mut out = enc.encode_bgra(&frame(w, h, shade), w, h, i == 0).unwrap();
        if out.is_none() {
            out = enc.poll_output(Duration::from_millis(100)).unwrap();
        }
        let enc_ms = t.elapsed().as_millis();
        let Some(nal) = out else {
            println!("frame {i} shade {shade}: encoder produced nothing");
            continue;
        };
        let t = Instant::now();
        let d = dec.decode_to_bgra(&nal, w, h).unwrap();
        let dec_ms = t.elapsed().as_millis();
        match d {
            Some((_, _, bgra)) => println!("in frame {i} shade {shade} enc {enc_ms} ms dec {dec_ms} ms -> OUT shade ~{}", mean(&bgra)),
            None => println!("in frame {i} shade {shade} enc {enc_ms} ms dec {dec_ms} ms -> no output yet"),
        }
        // Like a mostly-static desktop: one change, then nothing for a while.
        std::thread::sleep(Duration::from_millis(20));
    }
}
