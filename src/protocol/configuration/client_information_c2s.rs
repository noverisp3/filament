//! The settings the client says it has.

use super::id;
use crate::protocol::{DecodeError, VarInt};

/// `ServerboundClientInformationPacket`.
///
/// Recorded because the client sends it during configuration and this server
/// has to acknowledge that it arrived rather than treating it as unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInformation<'a> {
    pub language: &'a str,
    /// A single byte, not a VarInt. That changed from 1.20 and copying the old
    /// field layout would desynchronise every field after it.
    pub view_distance: u8,
    pub chat_visibility: i32,
    pub chat_colors: bool,
    pub model_customisation: u8,
    pub main_hand: i32,
    pub text_filtering_enabled: bool,
    pub allows_listing: bool,
    pub particle_status: i32,
}

impl<'a> ClientInformation<'a> {
    /// Reads the packet body including its id.
    pub fn read(body: &'a [u8]) -> Result<Self, DecodeError> {
        let mut r = body;

        let (VarInt(packet), _) = VarInt::read(&mut r)?;
        if packet != id::SERVERBOUND_CLIENT_INFORMATION {
            return Err(DecodeError::TooLong);
        }

        let (VarInt(len), _) = VarInt::read(&mut r)?;
        if len < 0 || len as usize > r.len() {
            return Err(DecodeError::StringTooLong {
                len: len.max(0) as usize,
                available: r.len(),
            });
        }
        let language = std::str::from_utf8(&r[..len as usize]).map_err(|_| DecodeError::NotUtf8)?;
        r = &r[len as usize..];

        let take_u8 = |r: &mut &[u8]| -> Result<u8, DecodeError> {
            let (b, rest) = r
                .split_first()
                .ok_or(DecodeError::UnexpectedEof {
                    wanted: "byte",
                    got: 0,
                })?;
            *r = rest;
            Ok(*b)
        };
        let take_bool = |r: &mut &[u8]| Ok(take_u8(r)? != 0);

        let view_distance = take_u8(&mut r)?;
        let chat_visibility = VarInt::read(&mut r)?.0.0;
        let chat_colors = take_bool(&mut r)?;
        let model_customisation = take_u8(&mut r)?;
        let main_hand = VarInt::read(&mut r)?.0.0;
        let text_filtering_enabled = take_bool(&mut r)?;
        let allows_listing = take_bool(&mut r)?;
        let particle_status = VarInt::read(&mut r)?.0.0;

        Ok(ClientInformation {
            language,
            view_distance,
            chat_visibility,
            chat_colors,
            model_customisation,
            main_hand,
            text_filtering_enabled,
            allows_listing,
            particle_status,
        })
    }
}
