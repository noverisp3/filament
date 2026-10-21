//! The registries the client needs, captured from a stock server.

use crate::protocol::{DecodeError, VarInt};

/// One captured `ClientboundRegistryDataPacket` frame after the next.
///
/// Yields the frame body with its packet id, which is what `FrameEncoder::encode`
/// takes. The frames are walked rather than decoded: the ids are still in place
/// and nothing here needs to be inside one.
pub struct RegistryDataFrames<'a> {
    data: &'a [u8],
    offset: usize,
    /// Set when a length could not be read, so the caller can tell a truncated
    /// file apart from one that simply ended.
    failed: Option<DecodeError>,
}

impl RegistryDataFrames<'_> {
    /// The error that ended the iteration early, if any.
    ///
    /// Separate from `Iterator::next` because an error here is reported as an
    /// item, and a caller that stops on the first error never sees the end of
    /// the walk that would tell it whether there was one.
    pub fn failure(&self) -> Option<&DecodeError> {
        self.failed.as_ref()
    }
}

impl<'a> Iterator for RegistryDataFrames<'a> {
    type Item = Result<&'a [u8], DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.data.len() {
            return None;
        }

        let mut slice = &self.data[self.offset..];
        let (length, consumed) = match VarInt::read(&mut slice) {
            Ok(v) => v,
            Err(e) => {
                // The offset cannot be advanced past a frame whose length is
                // unknown, so end the iteration instead of looping.
                self.offset = self.data.len();
                self.failed = Some(e);
                return None;
            }
        };
        if length.0 < 0 {
            let available = slice.len();
            self.offset = self.data.len();
            return Some(Err(DecodeError::StringTooLong {
                len: length.0.unsigned_abs() as usize,
                available,
            }));
        }

        let length = length.0 as usize;
        let body = self.offset + consumed;
        let end = body + length;
        if end > self.data.len() {
            let available = self.data.len() - body;
            self.offset = self.data.len();
            return Some(Err(DecodeError::StringTooLong { len: length, available }));
        }

        self.offset = end;
        Some(Ok(&self.data[body..end]))
    }
}

/// Borrows an iterator over the captured frames.
///
/// The size hint is a lower bound only: how many frames there are would mean
/// walking the file, which is what the caller is about to do anyway.
pub fn registry_data_frames() -> RegistryDataFrames<'static> {
    RegistryDataFrames {
        data: crate::generated::REGISTRY_DATA_FRAMES,
        offset: 0,
        failed: None,
    }
}

/// How many `registry_data` frames the capture holds.
pub fn registry_data_count() -> usize {
    crate::generated::REGISTRY_DATA_COUNT
}
