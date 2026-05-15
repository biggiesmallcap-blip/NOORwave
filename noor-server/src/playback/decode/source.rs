//! Decode source helpers.

use anyhow::Context;
use futures::StreamExt as _;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;

pub(crate) const DASH_INITIAL_MEDIA_SEGMENTS: usize = 8;
pub(crate) const DASH_SEGMENT_TIMEOUT_SECS: u64 = 12;

pub(crate) struct StreamPipe {
    data: Vec<u8>,
    read_pos: usize,
    rx: Mutex<std::sync::mpsc::Receiver<Option<Vec<u8>>>>,
    eof: bool,
    known_length: Option<u64>,
    dynamic_length: bool,
}

impl StreamPipe {
    pub(crate) fn new(
        rx: std::sync::mpsc::Receiver<Option<Vec<u8>>>,
        known_length: Option<u64>,
    ) -> Self {
        Self::with_initial(Vec::new(), rx, known_length, false)
    }

    pub(crate) fn with_initial(
        initial: Vec<u8>,
        rx: std::sync::mpsc::Receiver<Option<Vec<u8>>>,
        known_length: Option<u64>,
        dynamic_length: bool,
    ) -> Self {
        Self {
            data: initial,
            read_pos: 0,
            rx: Mutex::new(rx),
            eof: false,
            known_length,
            dynamic_length,
        }
    }

    fn fill_to(&mut self, target: usize) {
        if let Ok(rx) = self.rx.lock() {
            while !self.eof && self.data.len() < target {
                match rx.recv() {
                    Ok(Some(chunk)) => self.data.extend_from_slice(&chunk),
                    _ => self.eof = true,
                }
            }
        }
    }

    fn recv_chunk(&mut self) {
        if self.eof {
            return;
        }
        if let Ok(rx) = self.rx.lock() {
            match rx.recv() {
                Ok(Some(chunk)) => self.data.extend_from_slice(&chunk),
                _ => self.eof = true,
            }
        } else {
            self.eof = true;
        }
    }
}

impl Read for StreamPipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while !self.eof && self.read_pos >= self.data.len() {
            self.recv_chunk();
        }
        let available = self.data.len().saturating_sub(self.read_pos);
        if available == 0 {
            return Ok(0);
        }
        let n = buf.len().min(available);
        buf[..n].copy_from_slice(&self.data[self.read_pos..self.read_pos + n]);
        self.read_pos += n;
        Ok(n)
    }
}

impl Seek for StreamPipe {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(n) => {
                let t = n as usize;
                if t > self.data.len() {
                    self.fill_to(t);
                }
                t.min(self.data.len())
            }
            SeekFrom::Current(n) => {
                let t = (self.read_pos as i64 + n).max(0) as usize;
                if t > self.data.len() {
                    self.fill_to(t);
                }
                t.min(self.data.len())
            }
            SeekFrom::End(n) => {
                if self.dynamic_length && self.known_length.is_none() {
                    (self.data.len() as i64 + n).max(0) as usize
                } else {
                    while !self.eof {
                        self.recv_chunk();
                    }
                    (self.data.len() as i64 + n).max(0) as usize
                }
            }
        };
        self.read_pos = target.min(self.data.len());
        Ok(self.read_pos as u64)
    }
}

impl symphonia::core::io::MediaSource for StreamPipe {
    fn is_seekable(&self) -> bool {
        !self.dynamic_length || self.eof || self.known_length.is_some()
    }

    fn byte_len(&self) -> Option<u64> {
        if self.eof {
            Some(self.data.len() as u64)
        } else if self.dynamic_length {
            None
        } else {
            self.known_length
        }
    }
}

pub(crate) async fn append_stream_bytes(
    http: &reqwest::Client,
    url: &str,
    segment_index: usize,
) -> anyhow::Result<Vec<u8>> {
    let segment_label = dash_segment_debug_label(url);
    tokio::time::timeout(
        std::time::Duration::from_secs(DASH_SEGMENT_TIMEOUT_SECS),
        async {
            let response = http
                .get(url)
                .send()
                .await
                .with_context(|| {
                    format!("DASH segment {segment_index} request failed ({segment_label})")
                })?
                .error_for_status()
                .with_context(|| {
                    format!("DASH segment {segment_index} returned error status ({segment_label})")
                })?;
            let content_length = response.content_length().unwrap_or(0) as usize;
            let mut stream = response.bytes_stream();
            let mut out = Vec::with_capacity(content_length);
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.with_context(|| {
                    format!("DASH segment {segment_index} chunk error ({segment_label})")
                })?;
                out.extend_from_slice(&bytes);
            }
            Ok(out)
        },
    )
    .await
    .with_context(|| {
        format!(
            "DASH segment {segment_index} timed out after {DASH_SEGMENT_TIMEOUT_SECS}s ({segment_label})"
        )
    })?
}

pub(crate) fn dash_initial_media_count(total_segments: usize) -> usize {
    total_segments.min(DASH_INITIAL_MEDIA_SEGMENTS)
}

// Concurrency window for the background DASH segment downloader. Segments are
// consumed by Symphonia in order, but fetched in parallel up to this many
// in-flight requests so per-segment CDN latency doesn't starve the decoder.
// The downstream chunk channel (capacity 32) still bounds peak memory, and
// `futures::StreamExt::buffered` preserves output order.
pub(crate) fn dash_background_fetch_window() -> usize {
    4
}

pub(crate) fn build_tidal_cdn_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("TIDAL_ANDROID/1039 okhttp/3.14.9")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

pub(crate) fn dash_segment_debug_label(url: &str) -> String {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let path = base.split_once("://").map(|(_, rest)| rest).unwrap_or(base);
    let (host, path_tail) = path.split_once('/').unwrap_or((path, ""));
    let file_name = path_tail.rsplit('/').find(|part| !part.is_empty());
    let base_label = match file_name {
        Some(file_name) if !host.is_empty() => format!("{host}/{file_name}"),
        _ => path.to_string(),
    };

    if query.is_empty() {
        return base_label;
    }

    let mut keys = query
        .split('&')
        .filter_map(|part| part.split_once('=').map(|(key, _)| key).or(Some(part)))
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    if keys.is_empty() {
        base_label
    } else {
        format!("{}?{}", base_label, keys.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};

    #[test]
    fn stream_pipe_reports_known_length_before_eof() {
        let (_tx, rx) = std::sync::mpsc::sync_channel::<Option<Vec<u8>>>(1);
        let pipe = StreamPipe::new(rx, Some(641_302));

        assert_eq!(
            symphonia::core::io::MediaSource::byte_len(&pipe),
            Some(641_302)
        );
    }

    #[test]
    fn stream_pipe_hides_dynamic_length_for_dash_prebuffer() {
        let (_tx, rx) = std::sync::mpsc::sync_channel::<Option<Vec<u8>>>(1);
        let pipe = StreamPipe::with_initial(vec![1, 2, 3], rx, None, true);

        assert!(!symphonia::core::io::MediaSource::is_seekable(&pipe));
        assert_eq!(symphonia::core::io::MediaSource::byte_len(&pipe), None);
    }

    #[test]
    fn stream_pipe_dynamic_seek_end_uses_buffered_length_without_draining() {
        let (_tx, rx) = std::sync::mpsc::sync_channel::<Option<Vec<u8>>>(1);
        let mut pipe = StreamPipe::with_initial(vec![1, 2, 3], rx, None, true);

        assert_eq!(pipe.seek(SeekFrom::End(0)).unwrap(), 3);
        assert_eq!(symphonia::core::io::MediaSource::byte_len(&pipe), None);
    }

    #[test]
    fn stream_pipe_reports_dynamic_length_after_eof() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Option<Vec<u8>>>(2);
        tx.send(Some(vec![4, 5])).unwrap();
        tx.send(None).unwrap();
        let mut pipe = StreamPipe::with_initial(vec![1, 2, 3], rx, None, true);

        let mut out = Vec::new();
        pipe.read_to_end(&mut out).unwrap();

        assert_eq!(out, vec![1, 2, 3, 4, 5]);
        assert!(symphonia::core::io::MediaSource::is_seekable(&pipe));
        assert_eq!(symphonia::core::io::MediaSource::byte_len(&pipe), Some(5));
    }

    #[test]
    fn stream_pipe_seek_end_buffers_all_chunks() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Option<Vec<u8>>>(2);
        tx.send(Some(vec![1, 2, 3, 4])).unwrap();
        tx.send(None).unwrap();
        let mut pipe = StreamPipe::new(rx, Some(4));

        assert_eq!(pipe.seek(SeekFrom::End(0)).unwrap(), 4);
        assert_eq!(symphonia::core::io::MediaSource::byte_len(&pipe), Some(4));
    }

    #[test]
    fn dash_initial_media_count_prefers_multiple_segments() {
        assert_eq!(dash_initial_media_count(0), 0);
        assert_eq!(dash_initial_media_count(1), 1);
        assert_eq!(
            dash_initial_media_count(DASH_INITIAL_MEDIA_SEGMENTS + 10),
            DASH_INITIAL_MEDIA_SEGMENTS
        );
    }

    #[test]
    fn dash_background_fetch_window_pipelines_segments() {
        // Must be >=2 so per-segment HTTPS round-trips overlap and the decoder
        // doesn't starve when one segment's fetch exceeds its audio duration.
        assert!(dash_background_fetch_window() >= 2);
    }

    #[test]
    fn dash_segment_debug_label_redacts_query_values() {
        let label = dash_segment_debug_label(
            "https://audio.example.test/path/seg-9.m4s?Signature=secret&Expires=123",
        );

        assert_eq!(label, "audio.example.test/seg-9.m4s?Expires,Signature");
        assert!(!label.contains("secret"));
        assert!(!label.contains("123"));
    }
}
