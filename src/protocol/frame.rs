//! Packet framing.
//!
//! A frame is a VarInt length, followed by that many bytes, which hold a VarInt
//! packet id and a body. Before the server sends `login_compression`, that is
//! the whole format. Afterwards the frame carries a second length and its
//! contents are zlib-deflated.
//!
//! The length is validated against a hard ceiling before any buffer is sized to
//! it, because the value arrives from the network and an allocation sized to an
//! unchecked 2 GB length is a denial of service.

use std::io;

use super::{DecodeError, VarInt};

/// The largest frame this server will read.
///
/// Matches `MAX_PACKET_SIZE` in the decompiled server. The upper bound is not
/// arbitrary: a chunk in view distance 32 is already tens of thousands of
/// bytes, and anything past a few megabytes is not a real packet.
pub const MAX_FRAME_SIZE: usize = 2_097_152;

/// Default compression threshold.
///
/// Minecraft's own default is 256. Frames below it go out uncompressed, which
/// matters because a compressed frame carries an extra length field and the
/// client has to inflate it even for a one-byte packet.
pub const DEFAULT_COMPRESSION_THRESHOLD: i32 = 256;

/// Reads frames off a byte stream.
///
/// The buffer holds whatever has arrived. Frames are pulled out of it as they
/// become complete and the remainder is kept for the next read, because TCP
/// gives no guarantee that one read is one frame.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    threshold: Option<i32>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Turns compression on at `threshold`, as the login phase does.
    ///
    /// The switch applies to what is sent next, not to what has already been
    /// buffered: the client is told the threshold in the packet that carries it,
    /// so it also starts compressing after that packet, and applying it to
    /// buffered bytes would desynchronise the stream.
    pub fn set_compression(&mut self, threshold: i32) {
        self.threshold = Some(threshold);
    }

    /// Appends freshly read bytes.
    pub fn queue(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Takes the next complete frame body, or `None` if more bytes are needed.
    ///
    /// The body is copied into a caller-owned `Vec` rather than borrowed: a
    /// borrow would stop the caller from calling this again while holding the
    /// previous result, which makes the obvious loop impossible to write. One
    /// allocation per frame is cheap next to the network read it came from.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        let mut r = &self.buf[..];

        // An empty buffer is the normal state between reads, not a malformed
        // stream. Reporting it as an error would tear down a healthy
        // connection every time the socket goes quiet.
        if r.is_empty() {
            return Ok(None);
        }

        // A VarInt length can itself straddle a read boundary, so a partial
        // prefix means "wait for more" rather than "this is broken".
        let (VarInt(len), _) = match VarInt::read(&mut r) {
            Ok(v) => v,
            Err(DecodeError::UnexpectedEof { .. }) => return Ok(None),
            Err(e) => return Err(FrameError::Decode(e)),
        };

        let len = usize::try_from(len).map_err(|_| FrameError::TooLarge)?;
        if len > MAX_FRAME_SIZE {
            return Err(FrameError::TooLarge);
        }

        // Not yet arrived. The length is kept in the buffer along with the rest,
        // so the next call re-reads it rather than losing track of where the
        // frame starts.
        if r.len() < len {
            return Ok(None);
        }

        // Measured before the buffer is drained, because it is the number of
        // bytes the outer length prefix occupied on top of the frame itself.
        let prefix_len = self.buf.len() - r.len();

        let body = r[..len].to_vec();

        // The frame is complete, so it is removed from the buffer either way.
        // Which of its two layouts it has depends on the threshold.
        match self.threshold {
            Some(_) => {
                let mut inner = &body[..];
                let (VarInt(data_len), _) = VarInt::read(&mut inner).map_err(FrameError::Decode)?;
                let data_len = usize::try_from(data_len).map_err(|_| FrameError::TooLarge)?;

                self.consume(prefix_len + len);

                if data_len == 0 {
                    // Under the threshold: the payload is whatever followed a
                    // zero length inside the frame.
                    Ok(Some(inner.to_vec()))
                } else {
                    let mut out = Vec::with_capacity(data_len);
                    inflate_into(inner, data_len, &mut out)?;
                    Ok(Some(out))
                }
            }
            None => {
                self.consume(prefix_len + len);
                Ok(Some(body))
            }
        }
    }

    /// Drops the first `n` bytes, which have been consumed as a whole frame.
    fn consume(&mut self, n: usize) {
        self.buf.drain(..n.min(self.buf.len()));
    }
}

/// Inflates exactly `data_len` bytes of `out` from `compressed`.
///
/// `read_exact` rather than `read_to_end` on purpose: `data_len` comes off the
/// wire, and reading to the end would let a small payload followed by a large
/// compressed stream fill memory to whatever the sender chose.
fn inflate_into(compressed: &[u8], data_len: usize, out: &mut Vec<u8>) -> Result<(), FrameError> {
    use std::io::Read;

    out.resize(data_len, 0);

    let mut dec = flate2::read::ZlibDecoder::new(compressed);
    dec.read_exact(out)
        .map_err(|e| FrameError::Inflate(e.to_string()))?;

    Ok(())
}

/// Why a frame could not be produced.
#[derive(Debug)]
pub enum FrameError {
    Decode(DecodeError),
    /// The announced length exceeds [`MAX_FRAME_SIZE`].
    TooLarge,
    Inflate(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Decode(e) => write!(f, "malformed frame: {e}"),
            FrameError::TooLarge => write!(f, "frame exceeds {MAX_FRAME_SIZE} bytes"),
            FrameError::Inflate(e) => write!(f, "could not inflate frame: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Inflate(e.to_string())
    }
}

/// Writes frames, compressing once a threshold is set.
///
/// Kept separate from [`FrameDecoder`] because the two differ on one important
/// point: what is *sent* has to be compressed from the moment the threshold is
/// announced, while what is *received* is not compressed until after the
/// client's own reply.
#[derive(Debug, Default)]
pub struct FrameEncoder {
    threshold: Option<i32>,
}

impl FrameEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_compression(&mut self, threshold: i32) {
        self.threshold = Some(threshold);
    }

    /// Encodes one packet body into `out`.
///
/// Once a threshold is set, **every** frame carries an inner length: the
/// uncompressed size when the payload is deflated, and zero when it is not.
/// Writing that zero is not optional. `CompressionEncoder.encode` upstream
/// writes it in both branches, because the client decides whether to inflate by
/// reading it, and a frame that omits it makes the client read the first byte
/// of the payload as a length. That surfaces as `DataFormatException:
/// incorrect header check`, which says nothing about the real cause.
pub fn encode(&self, body: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
        let Some(threshold) = self.threshold else {
            VarInt(body.len() as i32).write(out);
            out.extend_from_slice(body);
            return Ok(());
        };

        let mut inner = Vec::with_capacity(body.len() + 8);

        if (body.len() as i32) < threshold {
            // Under the threshold: the length is zero and the payload follows as
            // it is.
            VarInt(0).write(&mut inner);
            inner.extend_from_slice(body);
        } else {
            let mut deflated = Vec::new();
            {
                use std::io::Write;
                let mut z = flate2::write::ZlibEncoder::new(
                    &mut deflated,
                    flate2::Compression::default(),
                );
                z.write_all(body).map_err(|e| FrameError::Inflate(e.to_string()))?;
            }
            // The length written is the uncompressed size, which is what the
            // client allocates before inflating.
            VarInt(body.len() as i32).write(&mut inner);
            inner.extend_from_slice(&deflated);
        }

        VarInt(inner.len() as i32).write(out);
        out.extend_from_slice(&inner);
        Ok(())
    }
}