use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use rodio::source::SeekError;
use rodio::Source;
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use symphonia::default::get_probe;

/// Custom MediaSource wrapper that preserves the stream's exact byte length
/// so Symphonia container probes (like ISO MP4/M4A, FLAC, WAV) never fail with Unseekable.
struct SizedFileSource {
    reader: BufReader<File>,
    byte_len: u64,
}

impl Read for SizedFileSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }
}

impl Seek for SizedFileSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.reader.seek(pos)
    }
}

impl MediaSource for SizedFileSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.byte_len)
    }
}

/// A panic-free, fully-featured audio source powered directly by Symphonia.
///
/// Implements `rodio::Source<Item = f32>` so it can be played directly
/// through `rodio::Sink`, with support for all audio codecs (MP3, M4A/AAC, FLAC,
/// WAV, Vorbis, Opus, ALAC, etc.) and native sample-accurate seeking.
pub struct SymphoniaSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    channels: u16,
    sample_rate: u32,
    total_duration: Option<Duration>,

    conv_buf: Option<SampleBuffer<f32>>,
    current_spec: Option<SignalSpec>,
    sample_buf: Vec<f32>,
    sample_idx: usize,
}

impl SymphoniaSource {
    /// Opens an audio file at `path`, automatically detecting its format with
    /// extension hints, and preparing the decoder.
    pub fn open_file(path: &str) -> Result<Self, String> {
        let file_path = Path::new(path);
        let file = File::open(file_path).map_err(|e| format!("Failed to open file: {}", e))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("Failed to read file metadata: {}", e))?;
        let byte_len = metadata.len();

        if byte_len == 0 {
            return Err("Audio file is 0 bytes (empty)".to_string());
        }

        let reader = BufReader::new(file);
        let sized_source = Box::new(SizedFileSource { reader, byte_len });
        let mss = MediaSourceStream::new(sized_source, Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = file_path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let format_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let metadata_opts = MetadataOptions::default();

        let probed = get_probe()
            .format(&hint, mss, &format_opts, &metadata_opts)
            .map_err(|e| format!("Unsupported or unrecognized audio format: {}", e))?;

        let format = probed.format;

        // Select the first supported audio track with a non-null codec
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| "No supported audio track found in file".to_string())?
            .clone();

        let track_id = track.id;
        let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
        let channels = track
            .codec_params
            .channels
            .map(|c| c.count() as u16)
            .unwrap_or(2);

        let time_base = track.codec_params.time_base;
        let total_duration =
            if let (Some(tb), Some(n_frames)) = (time_base, track.codec_params.n_frames) {
                let time = tb.calc_time(n_frames);
                Some(
                    Duration::from_secs(time.seconds)
                        + Duration::from_nanos((time.frac * 1_000_000_000.0) as u64),
                )
            } else {
                None
            };

        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| format!("Failed to instantiate codec decoder: {}", e))?;

        let mut source = Self {
            format,
            decoder,
            track_id,
            channels,
            sample_rate,
            total_duration,
            conv_buf: None,
            current_spec: None,
            sample_buf: Vec::new(),
            sample_idx: 0,
        };

        // Pre-decode the first audio packet so that sample_rate and channels
        // accurately reflect the decoded bitstream rather than container headers
        // (e.g. 96kHz AAC in M4A containers declaring 48kHz base headers).
        source.fill_buffer();

        Ok(source)
    }

    /// Attempts to decode the next packet and fill `sample_buf`.
    /// Returns `true` if samples were buffered, or `false` on EOF / unrecoverable error.
    fn fill_buffer(&mut self) -> bool {
        loop {
            match self.format.next_packet() {
                Ok(packet) => {
                    if packet.track_id() != self.track_id {
                        continue;
                    }
                    match self.decoder.decode(&packet) {
                        Ok(decoded) => {
                            let spec = *decoded.spec();
                            // Update sample rate and channels if they differ from initial metadata
                            self.sample_rate = spec.rate;
                            self.channels = spec.channels.count() as u16;

                            if self.conv_buf.is_none()
                                || self.conv_buf.as_ref().unwrap().capacity() < decoded.capacity()
                                || self.current_spec != Some(spec)
                            {
                                self.conv_buf =
                                    Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
                                self.current_spec = Some(spec);
                            }
                            if let Some(ref mut conv) = self.conv_buf {
                                conv.copy_interleaved_ref(decoded);
                                self.sample_buf.clear();
                                self.sample_buf.extend_from_slice(conv.samples());
                                self.sample_idx = 0;
                                return !self.sample_buf.is_empty();
                            }
                        }
                        Err(Error::DecodeError(_)) => {
                            // Non-fatal frame decode error: skip to next packet
                            continue;
                        }
                        Err(_) => {
                            // Fatal decoder error
                            return false;
                        }
                    }
                }
                Err(Error::IoError(ref e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return false;
                }
                Err(_) => {
                    return false;
                }
            }
        }
    }
}

impl Iterator for SymphoniaSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.sample_idx < self.sample_buf.len() {
                let sample = self.sample_buf[self.sample_idx];
                self.sample_idx += 1;
                return Some(sample);
            }

            if !self.fill_buffer() {
                return None;
            }
        }
    }
}

impl Source for SymphoniaSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let time = Time::from(pos.as_secs_f64());
        let seek_to = SeekTo::Time {
            time,
            track_id: Some(self.track_id),
        };

        match self.format.seek(SeekMode::Accurate, seek_to) {
            Ok(_) => {
                self.decoder.reset();
                self.sample_buf.clear();
                self.sample_idx = 0;
                Ok(())
            }
            Err(_) => Err(SeekError::NotSupported {
                underlying_source: "symphonia",
            }),
        }
    }
}
