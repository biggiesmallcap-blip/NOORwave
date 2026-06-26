//! Track download engine: fetch a TIDAL stream, decode it, and re-encode it to a
//! real `.flac` or `.mp3` file on disk.
//!
//! Why re-encode at all? TIDAL delivers lossless as DASH fMP4 segments (so the raw
//! bytes concatenate into an `.mp4`, not a `.flac`) and never serves MP3. Decoding to
//! PCM and re-encoding gives one consistent, tag-able container. FLAC re-encode is
//! bit-perfect (lossless in, lossless out); MP3 is a lossy 320 kbps convenience copy.
//!
//! Memory: the symphonia decode feeds the encoder block-by-block via a streaming
//! `Source`, so the raw PCM is never fully buffered. We do hold the encoded TIDAL
//! source bytes in memory (symphonia needs random access because TIDAL puts the MP4
//! `moov` atom at the end of the file).

use std::collections::VecDeque;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::{StreamExt, TryStreamExt};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use crate::db::models::Track;

/// Output container chosen by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadFormat {
    Flac,
    Mp3,
    /// AAC saved straight from TIDAL's `HIGH` tier with no transcode (`.m4a`). AAC beats
    /// MP3 at the same bitrate, and skipping the re-encode means zero added loss + the
    /// fastest path (just fetch and write).
    Aac,
}

impl DownloadFormat {
    pub fn from_query(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "flac" => Some(Self::Flac),
            "mp3" => Some(Self::Mp3),
            "aac" | "m4a" => Some(Self::Aac),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Aac => "aac",
        }
    }

    /// On-disk extension. AAC audio lives in an MP4 container, so it's `.m4a`.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Aac => "m4a",
        }
    }
}

/// Source quality for lossless (FLAC) downloads. TIDAL exposes two lossless tiers and
/// they differ a lot in size, so this is user-selectable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlacQuality {
    /// CD quality: 16-bit / 44.1 kHz (TIDAL `LOSSLESS`). Much smaller files.
    Cd,
    /// Best available master, up to 24-bit / 192 kHz (TIDAL `HI_RES_LOSSLESS`).
    HiRes,
}

impl Default for FlacQuality {
    fn default() -> Self {
        Self::HiRes
    }
}

impl FlacQuality {
    pub fn from_query(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "cd" => Some(Self::Cd),
            "hires" | "hi_res" | "hi-res" => Some(Self::HiRes),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cd => "cd",
            Self::HiRes => "hires",
        }
    }

    pub fn tidal_quality(self) -> &'static str {
        match self {
            Self::Cd => "LOSSLESS",
            Self::HiRes => "HI_RES_LOSSLESS",
        }
    }
}

/// Source tier to transcode an MP3 from. Both squash to 320 kbps MP3; the difference is
/// fetch/decode speed vs how clean the source is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3Source {
    /// TIDAL `HIGH` (AAC ~320 kbps): small + fast. The MP3 is then a second lossy hop,
    /// but the difference is inaudible for a portable copy.
    Aac,
    /// TIDAL `LOSSLESS` (FLAC 16/44.1): bigger + slower, but a single lossy hop, so the
    /// best-sounding MP3.
    Lossless,
}

impl Default for Mp3Source {
    fn default() -> Self {
        Self::Aac
    }
}

impl Mp3Source {
    pub fn from_query(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "aac" => Some(Self::Aac),
            "lossless" => Some(Self::Lossless),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Lossless => "lossless",
        }
    }

    pub fn tidal_quality(self) -> &'static str {
        match self {
            Self::Aac => "HIGH",
            Self::Lossless => "LOSSLESS",
        }
    }
}

/// Resolve the TIDAL `audioquality` string to request for a download, from the chosen
/// format and the user's per-format source/quality settings.
pub fn resolve_tidal_quality(
    format: DownloadFormat,
    flac_quality: FlacQuality,
    mp3_source: Mp3Source,
) -> &'static str {
    match format {
        DownloadFormat::Mp3 => mp3_source.tidal_quality(),
        DownloadFormat::Flac => flac_quality.tidal_quality(),
        DownloadFormat::Aac => "HIGH",
    }
}

/// Result of a single-track download.
#[derive(Debug, Clone)]
pub enum DownloadOutcome {
    Saved(PathBuf),
    AlreadyExists(PathBuf),
}

impl DownloadOutcome {
    pub fn path(&self) -> &Path {
        match self {
            Self::Saved(p) | Self::AlreadyExists(p) => p.as_path(),
        }
    }
}

/// Per-track download error, classified so the batch worker can decide whether to
/// refresh the token, retry once, or mark the track failed.
#[derive(Debug)]
pub enum DownloadError {
    /// Track has no `tidal_id` (non-TIDAL source or unresolved pending row).
    NoTidalId,
    /// TIDAL session expired: refresh the token and retry.
    SessionExpired,
    /// Permanent: geo-restricted, no stream asset, pulled from the catalogue.
    NotAvailable(String),
    /// Transient: network/timeout/5xx. Worth one retry.
    Transient(String),
    /// Decode/encode/tag failure for this track. Fatal for the track, not the batch.
    Encode(String),
    /// Filesystem error writing the output.
    Io(String),
}

impl DownloadError {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient(_))
    }

    /// Short, user-facing reason for the failed-list summary.
    pub fn reason(&self) -> String {
        match self {
            Self::NoTidalId => "Not on TIDAL".to_string(),
            Self::SessionExpired => "TIDAL session expired".to_string(),
            Self::NotAvailable(m) => m.clone(),
            Self::Transient(m) => m.clone(),
            Self::Encode(m) => m.clone(),
            Self::Io(m) => m.clone(),
        }
    }
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason())
    }
}

impl std::error::Error for DownloadError {}

// ─── Filename / path layout ──────────────────────────────────────────────────────

/// Windows reserved device names that can't be a bare file/dir stem.
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Sanitize one path segment (a folder or file stem) for cross-platform safety.
///
/// Strips characters illegal on Windows (`<>:"/\|?*` and control chars), trims trailing
/// dots/spaces (also illegal on Windows), guards reserved device names, caps length so
/// the whole path stays well under MAX_PATH, and never returns an empty string.
pub fn sanitize_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => out.push('_'),
            c if (c as u32) < 0x20 => out.push('_'),
            c => out.push(c),
        }
    }
    // Trim trailing dots/spaces and leading/trailing whitespace.
    let trimmed = out.trim().trim_end_matches(['.', ' ']).trim();
    let mut cleaned = trimmed.to_string();

    if cleaned.is_empty() {
        cleaned = "Unknown".to_string();
    }

    // Reserved device names (case-insensitive, compared on the stem).
    if RESERVED_NAMES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&cleaned))
    {
        cleaned.push('_');
    }

    // Cap a single segment so the assembled path stays under Windows MAX_PATH.
    const MAX_SEGMENT: usize = 120;
    if cleaned.chars().count() > MAX_SEGMENT {
        cleaned = cleaned.chars().take(MAX_SEGMENT).collect::<String>();
        cleaned = cleaned
            .trim()
            .trim_end_matches(['.', ' '])
            .trim()
            .to_string();
    }

    cleaned
}

/// Build the nested library-relative path for a track: `Artist/Album/NN - Title.ext`.
///
/// Falls back gracefully: missing artist -> "Unknown Artist"; missing album -> no album
/// subfolder (a loose single in the artist folder); missing track number -> no `NN -`
/// prefix; multi-disc -> the track number is prefixed with the disc number.
pub fn relative_path_for(track: &Track, format: DownloadFormat) -> PathBuf {
    let artist = sanitize_segment(track.artist_name.as_deref().unwrap_or("Unknown Artist"));

    let mut path = PathBuf::from(artist);
    if let Some(album) = track
        .album_title
        .as_deref()
        .filter(|a| !a.trim().is_empty())
    {
        path.push(sanitize_segment(album));
    }

    let mut stem = String::new();
    if let Some(track_no) = track.track_number {
        if let Some(disc) = track.disc_number.filter(|d| *d > 1) {
            stem.push_str(&format!("{disc}-{track_no:02} "));
        } else {
            stem.push_str(&format!("{track_no:02} "));
        }
        stem.push_str("- ");
    }
    stem.push_str(&track.title);
    let file = format!("{}.{}", sanitize_segment(&stem), format.extension());
    path.push(file);
    path
}

// ─── TIDAL fetch ─────────────────────────────────────────────────────────────────

/// Fetch the full encoded audio for a track. Unlike the analysis prescan there is no
/// byte cap: we need the whole file (TIDAL's MP4s place the `moov` atom at the end).
async fn fetch_encoded_bytes(
    http_client: &reqwest::Client,
    access_token: &str,
    tidal_id: i64,
    quality: &str,
) -> Result<Vec<u8>, DownloadError> {
    let stream_info = crate::services::tidal::stream::get_stream_url(
        http_client,
        access_token,
        tidal_id,
        quality,
    )
    .await
    .map_err(|e| {
        if e.is_session_expired() {
            DownloadError::SessionExpired
        } else {
            DownloadError::NotAvailable(format!("Couldn't resolve TIDAL stream: {e}"))
        }
    })?;

    // For DASH SegmentTemplate manifests `url` is the init segment and audio lives in
    // `segment_urls`; for BTS/JSON manifests `segment_urls` is empty and `url` is the
    // whole file. The init/whole-file URL must come first; media segments are fetched
    // concurrently (order preserved) so latency doesn't stack up across dozens of GETs.
    let mut buf = fetch_url_bytes(http_client, &stream_info.url).await?;

    if !stream_info.segment_urls.is_empty() {
        const FETCH_CONCURRENCY: usize = 6;
        let segments: Vec<Vec<u8>> = futures::stream::iter(stream_info.segment_urls.clone())
            .map(|url| {
                let client = http_client.clone();
                async move { fetch_url_bytes(&client, &url).await }
            })
            .buffered(FETCH_CONCURRENCY)
            .try_collect()
            .await?;
        for seg in segments {
            buf.extend_from_slice(&seg);
        }
    }

    if buf.len() < 1024 {
        return Err(DownloadError::NotAvailable(
            "TIDAL returned an empty stream".to_string(),
        ));
    }
    Ok(buf)
}

/// GET one URL fully into a `Vec<u8>`, mapping network errors to a transient failure.
async fn fetch_url_bytes(
    http_client: &reqwest::Client,
    url: &str,
) -> Result<Vec<u8>, DownloadError> {
    let resp = http_client
        .get(url)
        .send()
        .await
        .map_err(|e| DownloadError::Transient(format!("Stream fetch failed: {e}")))?;
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| DownloadError::Transient(format!("Stream read: {e}")))?;
    Ok(bytes.to_vec())
}

// ─── FLAC: streaming symphonia -> flacenc ────────────────────────────────────────

/// A `flacenc::source::Source` that pulls native-integer PCM from a symphonia decoder
/// block-by-block, so the whole signal is never buffered as one `Vec`.
struct SymphoniaI32Source {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    channels: usize,
    bits_per_sample: usize,
    sample_rate: usize,
    /// Leftover interleaved samples decoded but not yet consumed by the encoder.
    pending: Vec<i32>,
    eof: bool,
}

impl SymphoniaI32Source {
    /// Decode one more packet's worth of samples into `pending`. Returns false at EOF.
    fn decode_one(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => {
                    self.eof = true;
                    return false;
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    let mut sb = symphonia::core::audio::SampleBuffer::<i32>::new(
                        decoded.capacity() as u64,
                        spec,
                    );
                    sb.copy_interleaved_ref(decoded);
                    self.pending.extend_from_slice(sb.samples());
                    return true;
                }
                // Skip transient decode hiccups, mirroring the prescan decoder.
                Err(_) => continue,
            }
        }
    }
}

impl flacenc::source::Source for SymphoniaI32Source {
    fn channels(&self) -> usize {
        self.channels
    }
    fn bits_per_sample(&self) -> usize {
        self.bits_per_sample
    }
    fn sample_rate(&self) -> usize {
        self.sample_rate
    }

    fn read_samples<F: flacenc::source::Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> Result<usize, flacenc::error::SourceError> {
        let need = block_size * self.channels;
        while self.pending.len() < need && !self.eof {
            self.decode_one();
        }
        let take = need.min(self.pending.len());
        // Keep the slice a whole number of inter-channel frames.
        let take = take - (take % self.channels.max(1));
        if take == 0 {
            return Ok(0);
        }
        dest.fill_interleaved(&self.pending[..take])?;
        self.pending.drain(..take);
        Ok(take / self.channels)
    }
}

/// Probe + open a symphonia decoder over the in-memory encoded bytes. TIDAL streams are
/// MP4/BTS containers detected by content, so no extension hint is needed.
fn open_decoder(
    encoded: Vec<u8>,
) -> Result<
    (
        Box<dyn symphonia::core::formats::FormatReader>,
        Box<dyn symphonia::core::codecs::Decoder>,
        u32,         // track id
        usize,       // channels
        u32,         // sample rate
        Option<u32>, // bits per sample
    ),
    DownloadError,
> {
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let mss = MediaSourceStream::new(Box::new(Cursor::new(encoded)), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::default(),
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| DownloadError::Encode(format!("Couldn't read stream container: {e}")))?;
    let format = probed.format;

    let track = format
        .default_track()
        .ok_or_else(|| DownloadError::Encode("No audio track in stream".to_string()))?;
    let codec_params = track.codec_params.clone();
    let track_id = track.id;
    let sample_rate = codec_params
        .sample_rate
        .ok_or_else(|| DownloadError::Encode("Unknown sample rate".to_string()))?;
    let channels = codec_params.channels.map(|c| c.count()).unwrap_or(2).max(1);
    let bits = codec_params.bits_per_sample;

    let decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|e| DownloadError::Encode(format!("No decoder for stream: {e}")))?;

    Ok((format, decoder, track_id, channels, sample_rate, bits))
}

/// Decode the encoded bytes and write a bit-perfect FLAC to `out_path`. Blocking.
fn encode_flac(encoded: Vec<u8>, out_path: &Path) -> Result<(), DownloadError> {
    use flacenc::error::Verify;

    let (format, decoder, track_id, channels, sample_rate, bits) = open_decoder(encoded)?;
    // FLAC sources expose their real bit depth; lossy (AAC) sources report none, so the
    // re-encode targets 16-bit.
    let bits_per_sample = bits.unwrap_or(16) as usize;

    let source = SymphoniaI32Source {
        format,
        decoder,
        track_id,
        channels,
        bits_per_sample,
        sample_rate: sample_rate as usize,
        pending: Vec::new(),
        eof: false,
    };

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, e)| DownloadError::Encode(format!("FLAC config invalid: {e}")))?;
    let block_size = config.block_size;

    let stream = flacenc::encode_with_fixed_block_size(&config, source, block_size)
        .map_err(|e| DownloadError::Encode(format!("FLAC encode failed: {e}")))?;

    use flacenc::component::BitRepr;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| DownloadError::Encode(format!("FLAC serialize failed: {e}")))?;

    std::fs::write(out_path, sink.into_inner())
        .map_err(|e| DownloadError::Io(format!("Couldn't write FLAC: {e}")))
}

// ─── MP3: streaming symphonia (f32) -> LAME ──────────────────────────────────────

/// Decode the encoded bytes and write a 320 kbps CBR MP3 to `out_path`. Blocking.
///
/// MP3 is lossy, so we decode to normalized f32 and feed LAME's float API (cleaner than
/// scaling integer samples to LAME's full-range int convention).
fn encode_mp3(encoded: Vec<u8>, out_path: &Path) -> Result<(), DownloadError> {
    use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, InterleavedPcm, Quality};

    let (mut format, mut decoder, track_id, channels, sample_rate, _bits) = open_decoder(encoded)?;

    let mut builder = Builder::new()
        .ok_or_else(|| DownloadError::Encode("Couldn't create LAME encoder".to_string()))?;
    builder
        .set_num_channels(2)
        .map_err(|e| DownloadError::Encode(format!("LAME channels: {e}")))?;
    builder
        .set_sample_rate(sample_rate)
        .map_err(|e| DownloadError::Encode(format!("LAME sample rate {sample_rate}: {e}")))?;
    builder
        .set_brate(Bitrate::Kbps320)
        .map_err(|e| DownloadError::Encode(format!("LAME bitrate: {e}")))?;
    builder
        .set_quality(Quality::Best)
        .map_err(|e| DownloadError::Encode(format!("LAME quality: {e}")))?;
    let mut encoder = builder
        .build()
        .map_err(|e| DownloadError::Encode(format!("LAME init: {e}")))?;

    let mut file = std::fs::File::create(out_path)
        .map_err(|e| DownloadError::Io(format!("Couldn't create MP3: {e}")))?;

    // Reusable stereo-interleaved f32 buffer fed to LAME per packet.
    let mut stereo: Vec<f32> = Vec::new();
    let mut mp3_chunk: Vec<u8> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let spec = *decoded.spec();
        let mut sb =
            symphonia::core::audio::SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sb.copy_interleaved_ref(decoded);
        let samples = sb.samples();

        // Map n-channel interleaved -> stereo (duplicate mono; take first two channels).
        let frames = samples.len() / channels.max(1);
        stereo.clear();
        stereo.reserve(frames * 2);
        for f in 0..frames {
            let base = f * channels;
            let l = samples[base];
            let r = if channels >= 2 { samples[base + 1] } else { l };
            stereo.push(l);
            stereo.push(r);
        }

        mp3_chunk.reserve(mp3lame_encoder::max_required_buffer_size(frames));
        encoder
            .encode_to_vec(InterleavedPcm(&stereo), &mut mp3_chunk)
            .map_err(|e| DownloadError::Encode(format!("MP3 encode: {e}")))?;
        file.write_all(&mp3_chunk)
            .map_err(|e| DownloadError::Io(format!("MP3 write: {e}")))?;
        mp3_chunk.clear();
    }

    mp3_chunk.reserve(7200);
    encoder
        .flush_to_vec::<FlushNoGap>(&mut mp3_chunk)
        .map_err(|e| DownloadError::Encode(format!("MP3 flush: {e}")))?;
    file.write_all(&mp3_chunk)
        .map_err(|e| DownloadError::Io(format!("MP3 write: {e}")))?;
    file.flush()
        .map_err(|e| DownloadError::Io(format!("MP3 flush write: {e}")))?;
    Ok(())
}

// ─── Tagging ─────────────────────────────────────────────────────────────────────

/// Stamp Vorbis comments onto a freshly-written FLAC file. Best-effort: a tag failure
/// shouldn't lose the audio that already landed.
fn tag_flac(path: &Path, track: &Track) {
    let mut tag = match metaflac::Tag::read_from_path(path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(target = "noor.download", "FLAC tag read failed: {e}");
            return;
        }
    };
    tag.set_vorbis("TITLE", vec![track.title.clone()]);
    if let Some(artist) = &track.artist_name {
        tag.set_vorbis("ARTIST", vec![artist.clone()]);
    }
    if let Some(album) = &track.album_title {
        tag.set_vorbis("ALBUM", vec![album.clone()]);
    }
    if let Some(n) = track.track_number {
        tag.set_vorbis("TRACKNUMBER", vec![n.to_string()]);
    }
    if let Some(d) = track.disc_number {
        tag.set_vorbis("DISCNUMBER", vec![d.to_string()]);
    }
    if let Some(isrc) = &track.isrc {
        tag.set_vorbis("ISRC", vec![isrc.clone()]);
    }
    if let Err(e) = tag.write_to_path(path) {
        tracing::warn!(target = "noor.download", "FLAC tag write failed: {e}");
    }
}

/// Stamp an ID3v2.4 tag onto a freshly-written MP3 file. Best-effort.
fn tag_mp3(path: &Path, track: &Track) {
    use id3::TagLike;
    let mut tag = id3::Tag::new();
    tag.set_title(track.title.clone());
    if let Some(artist) = &track.artist_name {
        tag.set_artist(artist.clone());
    }
    if let Some(album) = &track.album_title {
        tag.set_album(album.clone());
    }
    if let Some(n) = track.track_number {
        tag.set_track(n as u32);
    }
    if let Some(d) = track.disc_number {
        tag.set_disc(d as u32);
    }
    if let Err(e) = tag.write_to_path(path, id3::Version::Id3v24) {
        tracing::warn!(target = "noor.download", "MP3 tag write failed: {e}");
    }
}

/// Stamp MP4/iTunes-style tags onto a freshly-written `.m4a` (AAC) file. Best-effort:
/// the audio is valid regardless, and TIDAL's fragmented MP4 may not always be writable.
fn tag_m4a(path: &Path, track: &Track) {
    let mut tag = match mp4ameta::Tag::read_from_path(path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(target = "noor.download", "M4A tag read failed: {e}");
            return;
        }
    };
    tag.set_title(&track.title);
    if let Some(artist) = &track.artist_name {
        tag.set_artist(artist);
    }
    if let Some(album) = &track.album_title {
        tag.set_album(album);
    }
    if let Some(n) = track.track_number {
        tag.set_track_number(n as u16);
    }
    if let Some(d) = track.disc_number {
        tag.set_disc_number(d as u16);
    }
    if let Err(e) = tag.write_to_path(path) {
        tracing::warn!(target = "noor.download", "M4A tag write failed: {e}");
    }
}

// ─── Orchestration ───────────────────────────────────────────────────────────────

/// Download a single track to `dest_root` in the chosen format. Resolves the TIDAL
/// stream, decodes it, re-encodes to FLAC/MP3, tags the file, and writes it into the
/// nested `Artist/Album/NN - Title.ext` library layout. Skips if the file already exists.
pub async fn download_track(
    http_client: &reqwest::Client,
    access_token: &str,
    track: &Track,
    dest_root: &Path,
    format: DownloadFormat,
    quality: &str,
) -> Result<DownloadOutcome, DownloadError> {
    let tidal_id = track.tidal_id.ok_or(DownloadError::NoTidalId)?;

    let rel = relative_path_for(track, format);
    let final_path = dest_root.join(&rel);
    if final_path.exists() {
        return Ok(DownloadOutcome::AlreadyExists(final_path));
    }

    let encoded = fetch_encoded_bytes(http_client, access_token, tidal_id, quality).await?;

    // Encode + tag on a blocking thread (CPU-bound). Write to a `.part` sidecar then
    // rename so a crash mid-encode never leaves a half file masquerading as complete.
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| DownloadError::Io(format!("Couldn't create folder: {e}")))?;
    }
    let part_path = final_path.with_extension(format!("{}.part", format.extension()));
    let part_for_task = part_path.clone();
    let track_for_task = track.clone();

    let result = tokio::task::spawn_blocking(move || {
        match format {
            DownloadFormat::Flac => encode_flac(encoded, &part_for_task)?,
            DownloadFormat::Mp3 => encode_mp3(encoded, &part_for_task)?,
            // AAC is TIDAL's HIGH stream already; write it through with no transcode.
            DownloadFormat::Aac => std::fs::write(&part_for_task, &encoded)
                .map_err(|e| DownloadError::Io(format!("Couldn't write AAC: {e}")))?,
        }
        match format {
            DownloadFormat::Flac => tag_flac(&part_for_task, &track_for_task),
            DownloadFormat::Mp3 => tag_mp3(&part_for_task, &track_for_task),
            DownloadFormat::Aac => tag_m4a(&part_for_task, &track_for_task),
        }
        Ok::<(), DownloadError>(())
    })
    .await
    .map_err(|e| DownloadError::Encode(format!("Encode task panicked: {e}")))?;

    if let Err(e) = result {
        let _ = std::fs::remove_file(&part_path);
        return Err(e);
    }

    std::fs::rename(&part_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&part_path);
        DownloadError::Io(format!("Couldn't finalize file: {e}"))
    })?;

    Ok(DownloadOutcome::Saved(final_path))
}

// ─── Config (server_config key/value) ────────────────────────────────────────────

/// OS Music dir, falling back to `~/Music`, then the current dir. Used as the default
/// download folder until the user picks one in Settings.
pub fn default_download_folder() -> PathBuf {
    dirs::audio_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Music")))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn read_download_folder(conn: &Connection) -> PathBuf {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'download_folder'",
            [],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    match stored.filter(|s| !s.trim().is_empty()) {
        Some(s) => PathBuf::from(s),
        None => default_download_folder(),
    }
}

pub fn write_download_folder(conn: &Connection, folder: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('download_folder', ?1)",
        [folder],
    )?;
    Ok(())
}

pub fn read_default_format(conn: &Connection) -> DownloadFormat {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'download_format_default'",
            [],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    stored
        .and_then(|s| DownloadFormat::from_query(&s))
        .unwrap_or(DownloadFormat::Flac)
}

pub fn write_default_format(conn: &Connection, format: DownloadFormat) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('download_format_default', ?1)",
        [format.as_str()],
    )?;
    Ok(())
}

pub fn read_flac_quality(conn: &Connection) -> FlacQuality {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'download_flac_quality'",
            [],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    stored
        .and_then(|s| FlacQuality::from_query(&s))
        .unwrap_or_default()
}

pub fn write_flac_quality(conn: &Connection, quality: FlacQuality) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('download_flac_quality', ?1)",
        [quality.as_str()],
    )?;
    Ok(())
}

pub fn read_mp3_source(conn: &Connection) -> Mp3Source {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'download_mp3_source'",
            [],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    stored
        .and_then(|s| Mp3Source::from_query(&s))
        .unwrap_or_default()
}

pub fn write_mp3_source(conn: &Connection, source: Mp3Source) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('download_mp3_source', ?1)",
        [source.as_str()],
    )?;
    Ok(())
}

// ─── Job queue + worker status ───────────────────────────────────────────────────

/// One queued download (a single track in a chosen format).
#[derive(Debug, Clone)]
pub struct DownloadJobItem {
    pub track_id: i64,
    pub format: DownloadFormat,
}

/// A track the worker couldn't save, kept for the end-of-run summary + Retry.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadFailedItem {
    pub id: i64,
    pub title: String,
    pub reason: String,
}

/// Status snapshot for `GET /api/downloads/status` (survives UI navigation).
#[derive(Debug, Clone, Serialize)]
pub struct DownloadStatus {
    pub running: bool,
    pub cancelling: bool,
    pub done: u32,
    pub total: u32,
    pub ok: u32,
    pub failed_count: u32,
    pub current_title: Option<String>,
    pub failed: Vec<DownloadFailedItem>,
}

#[derive(Default)]
struct ManagerInner {
    queue: VecDeque<DownloadJobItem>,
    active: bool,
    total: u32,
    done: u32,
    ok: u32,
    failed: Vec<DownloadFailedItem>,
    current_title: Option<String>,
    cancelling: bool,
}

/// The single unified download queue + one worker's status. All state lives under one
/// mutex so progress counters, the queue, and the active flag can never disagree.
/// Single-track requests are inserted at the front (run next); batch items append.
#[derive(Clone, Default)]
pub struct DownloadManager {
    inner: Arc<Mutex<ManagerInner>>,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ManagerInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Enqueue items. Resets the progress session if the worker was idle. Returns true
    /// when the caller should spawn a worker (the queue transitioned idle -> active).
    pub fn enqueue(&self, items: Vec<DownloadJobItem>, prioritize: bool) -> bool {
        let mut inner = self.lock();
        if !inner.active {
            inner.total = 0;
            inner.done = 0;
            inner.ok = 0;
            inner.failed.clear();
            inner.cancelling = false;
            inner.current_title = None;
        }
        let added = items.len() as u32;
        if prioritize {
            for item in items.into_iter().rev() {
                inner.queue.push_front(item);
            }
        } else {
            for item in items {
                inner.queue.push_back(item);
            }
        }
        inner.total += added;
        if inner.active {
            false
        } else {
            inner.active = true;
            true
        }
    }

    /// Pop the next item. When the queue is empty this also ends the session (clears the
    /// active flag under the same lock, so a racing `enqueue` cleanly starts a new one).
    pub fn next_item(&self) -> Option<DownloadJobItem> {
        let mut inner = self.lock();
        if inner.cancelling {
            inner.queue.clear();
        }
        match inner.queue.pop_front() {
            Some(item) => Some(item),
            None => {
                inner.active = false;
                inner.current_title = None;
                None
            }
        }
    }

    pub fn set_current(&self, title: Option<String>) {
        self.lock().current_title = title;
    }

    pub fn record_success(&self) {
        let mut inner = self.lock();
        inner.done += 1;
        inner.ok += 1;
    }

    pub fn record_failure(&self, id: i64, title: String, reason: String) {
        let mut inner = self.lock();
        inner.done += 1;
        inner.failed.push(DownloadFailedItem { id, title, reason });
    }

    pub fn request_cancel(&self) {
        let mut inner = self.lock();
        inner.cancelling = true;
        inner.queue.clear();
    }

    pub fn is_cancelling(&self) -> bool {
        self.lock().cancelling
    }

    /// (done, total, current_title) for a progress broadcast.
    pub fn progress(&self) -> (u32, u32, Option<String>) {
        let inner = self.lock();
        (inner.done, inner.total, inner.current_title.clone())
    }

    /// (ok, failed) final counts for the completion broadcast.
    pub fn final_counts(&self) -> (u32, u32) {
        let inner = self.lock();
        (inner.ok, inner.failed.len() as u32)
    }

    pub fn snapshot(&self) -> DownloadStatus {
        let inner = self.lock();
        DownloadStatus {
            running: inner.active,
            cancelling: inner.cancelling,
            done: inner.done,
            total: inner.total,
            ok: inner.ok,
            failed_count: inner.failed.len() as u32,
            current_title: inner.current_title.clone(),
            failed: inner.failed.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: Option<&str>, album: Option<&str>) -> Track {
        Track {
            id: 1,
            title: title.to_string(),
            artist_id: 1,
            artist_name: artist.map(str::to_string),
            album_id: None,
            album_title: album.map(str::to_string),
            disc_number: None,
            track_number: Some(3),
            duration_ms: None,
            isrc: None,
            tidal_id: Some(42),
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: None,
            best_source: None,
            fidelity_score: 0,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: None,
            source: "tidal".to_string(),
            artwork_url: None,
        }
    }

    #[test]
    fn sanitizes_illegal_characters() {
        assert_eq!(sanitize_segment("AC/DC: Back?"), "AC_DC_ Back_");
        assert_eq!(sanitize_segment("trailing dots..."), "trailing dots");
        assert_eq!(sanitize_segment("   "), "Unknown");
    }

    #[test]
    fn guards_reserved_names() {
        assert_eq!(sanitize_segment("CON"), "CON_");
        assert_eq!(sanitize_segment("nul"), "nul_");
    }

    #[test]
    fn nested_path_layout() {
        let t = track("Thunderstruck", Some("AC/DC"), Some("The Razors Edge"));
        let p = relative_path_for(&t, DownloadFormat::Flac);
        assert_eq!(
            p,
            PathBuf::from("AC_DC")
                .join("The Razors Edge")
                .join("03 - Thunderstruck.flac")
        );
    }

    #[test]
    fn missing_album_is_loose_single() {
        let t = track("B-side", Some("Someone"), None);
        let p = relative_path_for(&t, DownloadFormat::Mp3);
        assert_eq!(p, PathBuf::from("Someone").join("03 - B-side.mp3"));
    }

    /// Prove the FLAC re-encode is lossless: encode a known integer PCM signal with the
    /// same `flacenc` path the engine uses, decode it back with `claxon` (the reference
    /// pure-Rust FLAC decoder), and assert the samples are byte-identical. This also
    /// confirms the engine writes genuinely valid, standard FLAC.
    #[test]
    fn flac_encode_is_lossless_and_valid() {
        use flacenc::component::BitRepr;
        use flacenc::error::Verify;

        let channels = 2usize;
        let bits = 16usize;
        let rate = 44_100usize;

        // A deterministic waveform within 16-bit range.
        let mut pcm: Vec<i32> = Vec::new();
        for t in 0..20_000i32 {
            let left = ((t * 37) % 30_000) - 15_000;
            let right = 12_000 - ((t * 19) % 24_000);
            pcm.push(left);
            pcm.push(right);
        }

        let config = flacenc::config::Encoder::default()
            .into_verified()
            .expect("flac config");
        let source = flacenc::source::MemSource::from_samples(&pcm, channels, bits, rate);
        let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
            .expect("encode");
        let mut sink = flacenc::bitsink::ByteSink::new();
        stream.write(&mut sink).expect("serialize");
        let flac_bytes = sink.as_slice().to_vec();

        // Decode with claxon and compare to the original samples.
        let mut reader =
            claxon::FlacReader::new(Cursor::new(flac_bytes)).expect("claxon reads valid FLAC");
        let info = reader.streaminfo();
        assert_eq!(info.channels as usize, channels);
        assert_eq!(info.bits_per_sample as usize, bits);
        assert_eq!(info.sample_rate as usize, rate);

        let decoded: Vec<i32> = reader
            .samples()
            .map(|s| s.expect("decode sample"))
            .collect();
        assert_eq!(decoded, pcm, "FLAC re-encode must be bit-perfect");
    }
}
