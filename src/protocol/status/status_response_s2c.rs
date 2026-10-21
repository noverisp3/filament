//! The answer the server gives: the server's description, as JSON.

use std::borrow::Cow;

use crate::protocol::VarInt;

/// The JSON a server answers with when a client asks for its status.
///
/// Only the fields the client actually needs are here. `favicon` is left out
/// entirely rather than sent as null: it is optional upstream, and a null where
/// the client expects a string is the kind of thing that shows up as a blank
/// server icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub description: String,
    /// Shown as `N / max` in the browser.
    pub online: u32,
    pub max: u32,
    /// Who is on, shown when the browser list is hovered. Empty sends no
    /// `sample` field at all rather than an empty list, which the client
    /// renders as a server that forgot to say.
    pub sample: Vec<(String, String)>,
    /// The name in the browser, e.g. `filament 26.3`.
    pub version_name: String,
    pub protocol_version: i32,
}

impl ServerStatus {
    /// Renders the status as the JSON string that goes on the wire.
    ///
    /// Written by hand rather than through a JSON library because the shape is
    /// fixed and the escaping rules are narrow: only the description and the
    /// version name are free text. Every character that needs escaping in a
    /// JSON string is replaced, so a description containing a quote cannot
    /// produce invalid JSON, which the client treats as a broken server.
    pub fn to_json(&self) -> String {
        // The player list, if there is anyone to name. Written as a fragment
        // so the shape without one stays exactly what it was.
        let sample = if self.sample.is_empty() {
            String::new()
        } else {
            let entries: Vec<String> = self
                .sample
                .iter()
                .map(|(name, uuid)| {
                    format!(
                        r#"{{"name":{},"id":{}}}"#,
                        json_escape(name),
                        json_escape(uuid)
                    )
                })
                .collect();
            format!(r#","sample":[{}]"#, entries.join(","))
        };

        format!(
            r#"{{"version":{{"name":{},"protocol":{}}},
"players":{{"max":{},"online":{}{}}},
"description":{{"text":{}}}}}"#,
            json_escape(&self.version_name),
            self.protocol_version,
            self.max,
            self.online,
            sample,
            json_escape(&self.description),
        )
    }
}

/// Escapes a Rust string into a JSON string literal, including the quotes.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Control characters have no short escape and must not go through
            // raw, or they produce invalid JSON.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Writes a status response body, given a packet id of 0.
pub fn status_response_body(status: &ServerStatus) -> Vec<u8> {
    let json = status.to_json();
    let mut body = Vec::with_capacity(json.len() + 8);

    VarInt(0).write(&mut body);
    VarInt(json.len() as i32).write(&mut body);
    body.extend_from_slice(json.as_bytes());
    body
}

/// Convenience for building a status from parts, used by the server.
pub fn status(
    description: impl Into<Cow<'static, str>>,
    version_name: impl Into<Cow<'static, str>>,
    protocol_version: i32,
    online: u32,
    max: u32,
) -> ServerStatus {
    ServerStatus {
        description: description.into().into_owned(),
        version_name: version_name.into().into_owned(),
        protocol_version,
        online,
        max,
        sample: Vec::new(),
    }
}
