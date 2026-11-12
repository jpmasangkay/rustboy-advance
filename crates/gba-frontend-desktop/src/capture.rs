use std::fs::{self, File};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gba_core::{LCD_HEIGHT, LCD_WIDTH, SAMPLE_RATE_HZ};

use crate::config::{AppConfig, ColorProfileChoice};
use crate::filters::{agb_srgb_lut, ags_001_lut, ags_101_lut, linear_lut};

const GIF_QUANT_SPEED: i32 = 10;
const WAV_HEADER_LEN: u64 = 44;
const MAX_WAV_DATA_BYTES: u64 = (u32::MAX as u64) - 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GifFpsMode {
    Fps60,
    Fps30,
}

impl GifFpsMode {
    pub(crate) const fn frame_skip(self) -> u64 {
        match self {
            Self::Fps60 => 1,
            Self::Fps30 => 2,
        }
    }

    pub(crate) const fn delay_cs(self) -> u16 {
        match self {
            Self::Fps60 => 2,
            Self::Fps30 => 3,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Fps60 => "60 FPS (Every Frame)",
            Self::Fps30 => "30 FPS (Skip 1 Frame)",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GifCaptureSummary {
    pub(crate) path: Option<PathBuf>,
    pub(crate) frames_written: u64,
    #[allow(dead_code)]
    pub(crate) source_frames: u64,
    #[allow(dead_code)]
    pub(crate) fps_mode: GifFpsMode,
}

#[derive(Clone, Debug)]
pub(crate) struct WavCaptureSummary {
    pub(crate) path: Option<PathBuf>,
    pub(crate) samples_written: u64,
    #[allow(dead_code)]
    pub(crate) data_bytes: u64,
    pub(crate) sample_rate: u32,
}

impl WavCaptureSummary {
    pub(crate) fn stereo_frames(&self) -> u64 {
        self.samples_written / 2
    }

    pub(crate) fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            (self.stereo_frames() as f64) / f64::from(self.sample_rate)
        }
    }
}

/// Converts a 240x160 GBA BGR555 framebuffer into RGBA8888 using the active color LUT.
pub(crate) fn bgr555_to_rgba(
    fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
    color_correction: bool,
    profile: ColorProfileChoice,
    out_rgba: &mut [u8],
) {
    let expected = LCD_WIDTH * LCD_HEIGHT * 4;
    assert_eq!(out_rgba.len(), expected, "RGBA output buffer size mismatch");

    let lut = if !color_correction || profile == ColorProfileChoice::Linear {
        linear_lut()
    } else {
        match profile {
            ColorProfileChoice::Agb001 => agb_srgb_lut(),
            ColorProfileChoice::Ags101 => ags_101_lut(),
            ColorProfileChoice::Ags001 => ags_001_lut(),
            ColorProfileChoice::Linear => linear_lut(),
        }
    };

    for (&px, dst) in fb_bgr555
        .iter()
        .zip(out_rgba.as_chunks_mut::<4>().0.iter_mut())
    {
        dst.copy_from_slice(&lut[(px & 0x7FFF) as usize]);
    }
}

pub(crate) struct GifRecorder<W: Write> {
    encoder: Option<gif::Encoder<W>>,
    output_path: Option<PathBuf>,
    fps_mode: GifFpsMode,
    source_frames: u64,
    frames_written: u64,
    scratch_rgba: Vec<u8>,
}

impl GifRecorder<BufWriter<File>> {
    pub(crate) fn start(path: PathBuf, fps_mode: GifFpsMode) -> Result<Self, String> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|e| format!("Failed creating capture dir: {e}"))?;
        }
        let file = File::create(&path).map_err(|e| format!("Failed creating GIF file: {e}"))?;
        let mut rec = Self::start_with_writer(BufWriter::new(file), fps_mode)?;
        rec.output_path = Some(path);
        Ok(rec)
    }
}

impl<W: Write> GifRecorder<W> {
    pub(crate) fn start_with_writer(writer: W, fps_mode: GifFpsMode) -> Result<Self, String> {
        let mut encoder = gif::Encoder::new(writer, LCD_WIDTH as u16, LCD_HEIGHT as u16, &[])
            .map_err(|e| format!("Failed initializing GIF encoder: {e}"))?;
        if let Err(e) = encoder.set_repeat(gif::Repeat::Infinite) {
            let _ = encoder.into_inner();
            return Err(format!("Failed setting GIF repeat extension: {e}"));
        }
        Ok(Self {
            encoder: Some(encoder),
            output_path: None,
            fps_mode,
            source_frames: 0,
            frames_written: 0,
            scratch_rgba: vec![0u8; LCD_WIDTH * LCD_HEIGHT * 4],
        })
    }

    pub(crate) fn frames_written(&self) -> u64 {
        self.frames_written
    }

    pub(crate) fn push_rgba_frame(&mut self, rgba: &[u8]) -> Result<bool, String> {
        let expected = LCD_WIDTH * LCD_HEIGHT * 4;
        if rgba.len() != expected {
            return Err(format!(
                "Invalid RGBA frame length for GIF: got {}, expected {expected}",
                rgba.len()
            ));
        }

        let take = self.source_frames.is_multiple_of(self.fps_mode.frame_skip());
        self.source_frames += 1;
        if !take {
            return Ok(false);
        }

        self.scratch_rgba.copy_from_slice(rgba);
        self.encode_scratch_frame()?;
        Ok(true)
    }

    pub(crate) fn push_bgr555_frame(
        &mut self,
        fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
        color_correction: bool,
        profile: ColorProfileChoice,
    ) -> Result<bool, String> {
        let take = self.source_frames.is_multiple_of(self.fps_mode.frame_skip());
        self.source_frames += 1;
        if !take {
            return Ok(false);
        }

        bgr555_to_rgba(
            fb_bgr555,
            color_correction,
            profile,
            &mut self.scratch_rgba,
        );
        self.encode_scratch_frame()?;
        Ok(true)
    }

    fn encode_scratch_frame(&mut self) -> Result<(), String> {
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| "GIF encoder already closed".to_string())?;
        let mut frame = gif::Frame::from_rgba_speed(
            LCD_WIDTH as u16,
            LCD_HEIGHT as u16,
            &mut self.scratch_rgba,
            GIF_QUANT_SPEED,
        );
        frame.delay = self.fps_mode.delay_cs();
        encoder
            .write_frame(&frame)
            .map_err(|e| format!("Failed writing GIF frame: {e}"))?;
        self.frames_written += 1;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(W, GifCaptureSummary), String> {
        let encoder = self
            .encoder
            .take()
            .ok_or_else(|| "GIF encoder already closed".to_string())?;
        let mut writer = encoder
            .into_inner()
            .map_err(|e| format!("Failed finalizing GIF trailer: {e}"))?;
        writer
            .flush()
            .map_err(|e| format!("Failed flushing GIF output: {e}"))?;
        let summary = GifCaptureSummary {
            path: self.output_path.take(),
            frames_written: self.frames_written,
            source_frames: self.source_frames,
            fps_mode: self.fps_mode,
        };
        Ok((writer, summary))
    }
}

impl<W: Write> Drop for GifRecorder<W> {
    fn drop(&mut self) {
        if let Some(encoder) = self.encoder.take() {
            // Drain into_inner safely so gif::Encoder::drop never panics on I/O error.
            if let Ok(mut w) = encoder.into_inner() {
                let _ = w.flush();
            }
        }
    }
}

pub(crate) struct WavRecorder<W: Write + Seek> {
    writer: Option<W>,
    output_path: Option<PathBuf>,
    sample_rate: u32,
    data_bytes: u64,
    samples_written: u64,
    byte_buf: Vec<u8>,
}

impl WavRecorder<BufWriter<File>> {
    pub(crate) fn start(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|e| format!("Failed creating capture dir: {e}"))?;
        }
        let file = File::create(&path).map_err(|e| format!("Failed creating WAV file: {e}"))?;
        let mut rec = Self::start_with_writer(BufWriter::new(file))?;
        rec.output_path = Some(path);
        Ok(rec)
    }
}

impl<W: Write + Seek> WavRecorder<W> {
    pub(crate) fn start_with_writer(mut writer: W) -> Result<Self, String> {
        let header = build_wav_header(SAMPLE_RATE_HZ, 0);
        writer
            .write_all(&header)
            .map_err(|e| format!("Failed writing WAV header: {e}"))?;
        Ok(Self {
            writer: Some(writer),
            output_path: None,
            sample_rate: SAMPLE_RATE_HZ,
            data_bytes: 0,
            samples_written: 0,
            byte_buf: Vec::with_capacity(4096),
        })
    }

    pub(crate) fn duration_secs(&self) -> f64 {
        let stereo_frames = self.samples_written / 2;
        (stereo_frames as f64) / f64::from(self.sample_rate.max(1))
    }

    pub(crate) fn push_samples(&mut self, samples: &[i16]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }
        let add_bytes = (samples.len() as u64) * 2;
        if self.data_bytes.saturating_add(add_bytes) > MAX_WAV_DATA_BYTES {
            return Err("WAV 4 GiB RIFF size limit reached".into());
        }

        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| "WAV writer already closed".to_string())?;
        self.byte_buf.clear();
        self.byte_buf.reserve(samples.len() * 2);
        for &s in samples {
            self.byte_buf.extend_from_slice(&s.to_le_bytes());
        }
        writer
            .write_all(&self.byte_buf)
            .map_err(|e| format!("Failed writing WAV audio samples: {e}"))?;
        self.data_bytes += add_bytes;
        self.samples_written += samples.len() as u64;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(W, WavCaptureSummary), String> {
        let mut writer = self
            .writer
            .take()
            .ok_or_else(|| "WAV writer already closed".to_string())?;
        patch_wav_sizes(&mut writer, self.data_bytes)
            .map_err(|e| format!("Failed finalizing WAV header: {e}"))?;
        let summary = WavCaptureSummary {
            path: self.output_path.take(),
            samples_written: self.samples_written,
            data_bytes: self.data_bytes,
            sample_rate: self.sample_rate,
        };
        Ok((writer, summary))
    }
}

impl<W: Write + Seek> Drop for WavRecorder<W> {
    fn drop(&mut self) {
        if let Some(mut writer) = self.writer.take() {
            let _ = patch_wav_sizes(&mut writer, self.data_bytes);
        }
    }
}

fn build_wav_header(sample_rate: u32, data_bytes: u32) -> [u8; WAV_HEADER_LEN as usize] {
    let channels: u16 = 2;
    let bits_per_sample: u16 = 16;
    let block_align: u16 = channels * (bits_per_sample / 8);
    let byte_rate: u32 = sample_rate * u32::from(block_align);
    let riff_size: u32 = 36u32.saturating_add(data_bytes);

    let mut hdr = [0u8; WAV_HEADER_LEN as usize];
    hdr[0..4].copy_from_slice(b"RIFF");
    hdr[4..8].copy_from_slice(&riff_size.to_le_bytes());
    hdr[8..12].copy_from_slice(b"WAVE");
    hdr[12..16].copy_from_slice(b"fmt ");
    hdr[16..20].copy_from_slice(&16u32.to_le_bytes());
    hdr[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    hdr[22..24].copy_from_slice(&channels.to_le_bytes());
    hdr[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    hdr[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    hdr[32..34].copy_from_slice(&block_align.to_le_bytes());
    hdr[34..36].copy_from_slice(&bits_per_sample.to_le_bytes());
    hdr[36..40].copy_from_slice(b"data");
    hdr[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    hdr
}

fn patch_wav_sizes<W: Write + Seek>(writer: &mut W, data_bytes: u64) -> std::io::Result<()> {
    let data_u32 = u32::try_from(data_bytes).unwrap_or(u32::MAX - 36);
    let riff_u32 = 36u32.saturating_add(data_u32);
    writer.flush()?;
    writer.seek(SeekFrom::Start(4))?;
    writer.write_all(&riff_u32.to_le_bytes())?;
    writer.seek(SeekFrom::Start(40))?;
    writer.write_all(&data_u32.to_le_bytes())?;
    writer.flush()
}

pub(crate) fn sanitize_capture_stem(rom_title: &str) -> String {
    let cleaned: String = rom_title
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "rustboy".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn capture_output_path(dir: &Path, rom_title: &str, ext: &str) -> PathBuf {
    let stem = sanitize_capture_stem(rom_title);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let candidate = dir.join(format!("{stem}_{ts}.{ext}"));
    if !candidate.exists() {
        return candidate;
    }
    for suffix in 2u32..1000 {
        let next = dir.join(format!("{stem}_{ts}_{suffix}.{ext}"));
        if !next.exists() {
            return next;
        }
    }
    candidate
}

pub(crate) struct CaptureController {
    pub(crate) gif: Option<GifRecorder<BufWriter<File>>>,
    pub(crate) wav: Option<WavRecorder<BufWriter<File>>>,
    pub(crate) gif_fps_mode: GifFpsMode,
}

impl Default for CaptureController {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureController {
    pub(crate) fn new() -> Self {
        Self {
            gif: None,
            wav: None,
            gif_fps_mode: GifFpsMode::Fps60,
        }
    }

    pub(crate) fn is_gif_recording(&self) -> bool {
        self.gif.is_some()
    }

    pub(crate) fn is_wav_recording(&self) -> bool {
        self.wav.is_some()
    }

    pub(crate) fn gif_frames(&self) -> u64 {
        self.gif.as_ref().map_or(0, GifRecorder::frames_written)
    }

    pub(crate) fn wav_duration_secs(&self) -> f64 {
        self.wav.as_ref().map_or(0.0, WavRecorder::duration_secs)
    }

    pub(crate) fn start_gif(
        &mut self,
        rom_title: &str,
        custom_path: Option<PathBuf>,
    ) -> Result<PathBuf, String> {
        if self.gif.is_some() {
            return Err("GIF recording is already active".into());
        }
        let path = custom_path
            .unwrap_or_else(|| capture_output_path(&AppConfig::screenshots_dir(), rom_title, "gif"));
        let rec = GifRecorder::start(path.clone(), self.gif_fps_mode)?;
        self.gif = Some(rec);
        Ok(path)
    }

    pub(crate) fn stop_gif(&mut self) -> Result<Option<GifCaptureSummary>, String> {
        let Some(rec) = self.gif.take() else {
            return Ok(None);
        };
        let (_w, summary) = rec.finish()?;
        Ok(Some(summary))
    }

    pub(crate) fn start_wav(
        &mut self,
        rom_title: &str,
        custom_path: Option<PathBuf>,
    ) -> Result<PathBuf, String> {
        if self.wav.is_some() {
            return Err("WAV audio recording is already active".into());
        }
        let path = custom_path
            .unwrap_or_else(|| capture_output_path(&AppConfig::screenshots_dir(), rom_title, "wav"));
        let rec = WavRecorder::start(path.clone())?;
        self.wav = Some(rec);
        Ok(path)
    }

    pub(crate) fn stop_wav(&mut self) -> Result<Option<WavCaptureSummary>, String> {
        let Some(rec) = self.wav.take() else {
            return Ok(None);
        };
        let (_w, summary) = rec.finish()?;
        Ok(Some(summary))
    }

    pub(crate) fn push_video_frame(
        &mut self,
        fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
        color_correction: bool,
        profile: ColorProfileChoice,
    ) -> Result<(), String> {
        let Some(gif) = &mut self.gif else {
            return Ok(());
        };
        if let Err(e) = gif.push_bgr555_frame(fb_bgr555, color_correction, profile) {
            let _ = self.stop_gif();
            return Err(e);
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn push_video_rgba(&mut self, rgba: &[u8]) -> Result<(), String> {
        let Some(gif) = &mut self.gif else {
            return Ok(());
        };
        if let Err(e) = gif.push_rgba_frame(rgba) {
            let _ = self.stop_gif();
            return Err(e);
        }
        Ok(())
    }

    pub(crate) fn push_audio_samples(&mut self, samples: &[i16]) -> Result<(), String> {
        let Some(wav) = &mut self.wav else {
            return Ok(());
        };
        if let Err(e) = wav.push_samples(samples) {
            let _ = self.stop_wav();
            return Err(e);
        }
        Ok(())
    }

    pub(crate) fn stop_all(
        &mut self,
    ) -> (
        Result<Option<GifCaptureSummary>, String>,
        Result<Option<WavCaptureSummary>, String>,
    ) {
        (self.stop_gif(), self.stop_wav())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn solid_bgr555_frame(color: u16) -> [u16; LCD_WIDTH * LCD_HEIGHT] {
        [color & 0x7FFF; LCD_WIDTH * LCD_HEIGHT]
    }

    #[test]
    fn gif_recorder_writes_valid_gif89a_and_decodes_frames() {
        // 1. 60fps mode (frame_skip = 1, delay = 2)
        let mut rec_60 = GifRecorder::start_with_writer(Vec::new(), GifFpsMode::Fps60).unwrap();
        let colors = [0x001Fu16, 0x03E0u16, 0x7C00u16, 0x7FFFu16];
        for &c in &colors {
            let fb = solid_bgr555_frame(c);
            assert!(
                rec_60
                    .push_bgr555_frame(&fb, true, ColorProfileChoice::Agb001)
                    .unwrap()
            );
        }
        let (gif_bytes, summary) = rec_60.finish().unwrap();
        assert_eq!(summary.frames_written, 4);
        assert_eq!(summary.source_frames, 4);
        assert!(gif_bytes.starts_with(b"GIF89a"));
        assert_eq!(*gif_bytes.last().unwrap(), 0x3B);

        let mut decoder = gif::DecodeOptions::new();
        decoder.set_color_output(gif::ColorOutput::RGBA);
        let mut reader = decoder.read_info(Cursor::new(&gif_bytes)).unwrap();
        assert_eq!(reader.width(), LCD_WIDTH as u16);
        assert_eq!(reader.height(), LCD_HEIGHT as u16);
        let mut decoded_count = 0usize;
        while let Some(frame) = reader.read_next_frame().unwrap() {
            assert_eq!(frame.delay, 2);
            assert_eq!(frame.width, LCD_WIDTH as u16);
            assert_eq!(frame.height, LCD_HEIGHT as u16);
            assert_eq!(frame.buffer.len(), LCD_WIDTH * LCD_HEIGHT * 4);
            decoded_count += 1;
        }
        assert_eq!(decoded_count, 4);

        // 2. 30fps mode (frame_skip = 2, delay = 3): 4 source frames -> 2 encoded GIF frames
        let mut rec_30 = GifRecorder::start_with_writer(Vec::new(), GifFpsMode::Fps30).unwrap();
        let mut rgba = vec![0u8; LCD_WIDTH * LCD_HEIGHT * 4];
        for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px[0] = (i % 256) as u8;
            px[1] = ((i * 3) % 256) as u8;
            px[2] = ((i * 7) % 256) as u8;
            px[3] = 255;
        }
        assert!(rec_30.push_rgba_frame(&rgba).unwrap()); // frame 0 -> taken
        assert!(!rec_30.push_rgba_frame(&rgba).unwrap()); // frame 1 -> skipped
        assert!(rec_30.push_rgba_frame(&rgba).unwrap()); // frame 2 -> taken
        assert!(!rec_30.push_rgba_frame(&rgba).unwrap()); // frame 3 -> skipped

        let (gif30_bytes, summary30) = rec_30.finish().unwrap();
        assert_eq!(summary30.frames_written, 2);
        assert_eq!(summary30.source_frames, 4);
        assert!(gif30_bytes.starts_with(b"GIF89a"));

        let mut dec30 = gif::DecodeOptions::new().read_info(Cursor::new(&gif30_bytes)).unwrap();
        let mut count30 = 0usize;
        while let Some(frame) = dec30.read_next_frame().unwrap() {
            assert_eq!(frame.delay, 3);
            count30 += 1;
        }
        assert_eq!(count30, 2);
    }

    #[test]
    fn wav_recorder_writes_valid_riff_wave_with_exact_byte_counts() {
        let cursor = Cursor::new(Vec::new());
        let mut wav = WavRecorder::start_with_writer(cursor).unwrap();

        let batch1: Vec<i16> = vec![100, -100, 1234, -5678, 32767, -32768];
        let batch2: Vec<i16> = vec![0, 42, -42, 999];
        wav.push_samples(&batch1).unwrap();
        wav.push_samples(&batch2).unwrap();

        let (cursor, summary) = wav.finish().unwrap();
        let bytes = cursor.into_inner();

        let total_samples = batch1.len() + batch2.len();
        let expected_data_bytes = (total_samples * 2) as u32;
        assert_eq!(summary.samples_written, total_samples as u64);
        assert_eq!(summary.data_bytes, u64::from(expected_data_bytes));
        assert_eq!(bytes.len(), 44 + expected_data_bytes as usize);

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            36 + expected_data_bytes
        );
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(
            u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            16
        );
        assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 1); // PCM
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2); // Stereo
        assert_eq!(
            u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            SAMPLE_RATE_HZ
        );
        assert_eq!(
            u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
            SAMPLE_RATE_HZ * 4
        );
        assert_eq!(u16::from_le_bytes([bytes[32], bytes[33]]), 4); // block_align
        assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 16); // bits_per_sample
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(
            u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]),
            expected_data_bytes
        );

        let decoded_samples: Vec<i16> = bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        let mut expected_samples = batch1;
        expected_samples.extend(batch2);
        assert_eq!(decoded_samples, expected_samples);
    }

    #[test]
    fn disk_capture_creates_valid_gif_and_wav_files() {
        let dir = std::env::temp_dir().join("rustboy_capture_disk_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let gif_path = capture_output_path(&dir, "Pokemon Emerald!", "gif");
        let wav_path = capture_output_path(&dir, "Pokemon Emerald!", "wav");
        assert!(
            gif_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Pokemon_Emerald_")
        );

        let mut gif = GifRecorder::start(gif_path.clone(), GifFpsMode::Fps60).unwrap();
        gif.push_bgr555_frame(&solid_bgr555_frame(0x03E0), false, ColorProfileChoice::Linear)
            .unwrap();
        let (_, gsum) = gif.finish().unwrap();
        assert_eq!(gsum.frames_written, 1);
        assert_eq!(gsum.path.as_deref(), Some(gif_path.as_path()));

        let mut wav = WavRecorder::start(wav_path.clone()).unwrap();
        wav.push_samples(&[500, -500, 1000, -1000]).unwrap();
        let (_, wsum) = wav.finish().unwrap();
        assert_eq!(wsum.data_bytes, 8);
        assert_eq!(wsum.path.as_deref(), Some(wav_path.as_path()));

        let gif_disk = fs::read(&gif_path).unwrap();
        assert!(gif_disk.starts_with(b"GIF89a"));

        let wav_disk = fs::read(&wav_path).unwrap();
        assert_eq!(wav_disk.len(), 52);
        assert_eq!(&wav_disk[0..4], b"RIFF");
        assert_eq!(&wav_disk[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes([wav_disk[4], wav_disk[5], wav_disk[6], wav_disk[7]]), 44);
        assert_eq!(
            u32::from_le_bytes([wav_disk[40], wav_disk[41], wav_disk[42], wav_disk[43]]),
            8
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
