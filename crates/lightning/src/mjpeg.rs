//! Découpage d'un flux MJPEG (images JPEG concaténées) en images.

use std::io::Read;

/// Itérateur sur les images JPEG d'un flux.
pub struct JpegSplitter<R> {
    reader: R,
    buf: Vec<u8>,
    eof: bool,
}

impl<R: Read> JpegSplitter<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buf: Vec::with_capacity(1 << 18),
            eof: false,
        }
    }

    fn take_frame(&mut self) -> Option<Vec<u8>> {
        let start = find(&self.buf, &[0xFF, 0xD8], 0)?;
        let end = find(&self.buf, &[0xFF, 0xD9], start + 2)? + 2;
        let frame = self.buf[start..end].to_vec();
        self.buf.drain(..end);
        Some(frame)
    }
}

fn find(haystack: &[u8], needle: &[u8; 2], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(2)
        .position(|w| w == needle)
        .map(|i| i + from)
}

impl<R: Read> Iterator for JpegSplitter<R> {
    type Item = std::io::Result<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut chunk = [0u8; 1 << 16];
        loop {
            if let Some(frame) = self.take_frame() {
                return Some(Ok(frame));
            }
            if self.eof {
                return None;
            }
            match self.reader.read(&mut chunk) {
                Ok(0) => self.eof = true,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lecteur qui ne rend que quelques octets à la fois.
    struct Trickle<'a>(&'a [u8], usize);

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.1.min(self.0.len()).min(buf.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    #[test]
    fn splits_concatenated_frames_across_reads() {
        let a = [0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9];
        let b = [0xFF, 0xD8, 9, 0xFF, 0x00, 8, 0xFF, 0xD9];
        let mut stream = vec![0u8, 7]; // déchets avant la première image
        stream.extend_from_slice(&a);
        stream.extend_from_slice(&b);
        stream.extend_from_slice(&[0xFF, 0xD8, 5]); // image tronquée en fin de flux
        for step in [1, 3, 1024] {
            let frames: Vec<Vec<u8>> = JpegSplitter::new(Trickle(&stream, step))
                .map(Result::unwrap)
                .collect();
            assert_eq!(frames, vec![a.to_vec(), b.to_vec()], "pas de {step}");
        }
    }

    #[test]
    fn empty_stream_yields_nothing() {
        assert_eq!(JpegSplitter::new(&[][..]).count(), 0);
    }
}
