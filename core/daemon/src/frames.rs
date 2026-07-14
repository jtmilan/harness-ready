//! Streaming frames (snapshot / delta / error) — the on-wire output stream a
//! subscribed `Attach` connection emits (Phase 08 Sub-build 3 / slice 3, design §3.4).
//!
//! Frames are newline-delimited JSON on the SAME connection as the request/response
//! `SocketResponse` lines; a client tells them apart by the `"frame"` tag (frames) vs
//! the `"ok"` field (responses). Binary PTY bytes are base64-encoded so a chunk
//! containing a raw `\n` or invalid UTF-8 never corrupts the line framing.
//!
//! `base64` is implemented locally (a few lines, no dependency) so the gated-OFF daemon
//! adds no third-party crate for this slice.

use serde::{Deserialize, Serialize};

/// One output-stream frame for a subscription. `id` tags every frame so ONE connection
/// can multiplex several subscriptions (design §5 Q1).
///
/// `Deserialize` is additive (the DAEMON only serializes these; the app's Q4 attach-streaming
/// CLIENT, `app/.../daemon_stream.rs`, deserializes them). Keeping ONE wire definition here is
/// the SSOT — dead in any build that does not deserialize a frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum StreamFrame {
    /// Sent ONCE right after `Attach` succeeds: the retained scrollback window
    /// (base64) + the `baseline` (== `total_pushed` at snapshot time). The first
    /// `delta` on this subscription has `prev_total == baseline`.
    Snapshot {
        id: String,
        /// Always `"OK"` — the snapshot itself never carries a failure.
        code: String,
        baseline: u64,
        /// Decoded byte length of `data` (a cheap integrity cross-check for the client).
        data_len: usize,
        data: String,
    },
    /// One reader chunk pushed AFTER the snapshot. `prev_total`/`new_total` bracket the
    /// chunk (`new_total - prev_total == decoded(data).len()`); a client seeing
    /// `prev_total != its last new_total` has a GAP and should re-`Attach`.
    Delta {
        id: String,
        prev_total: u64,
        new_total: u64,
        data: String,
    },
    /// Closes ONE subscription (the connection stays alive if others remain): the pane
    /// died ([`code::PANE_DIED`]) or the subscription overflowed and was dropped
    /// ([`code::OVERFLOW`] → the client may re-`Attach` for a fresh snapshot).
    Error {
        id: String,
        code: String,
        detail: String,
    },
    /// A liveness probe the server emits on an idle streaming tick INSTEAD of closing a
    /// connection whose subscribed panes are simply quiet (agent panes idle far longer than
    /// the idle window). Carries no pane data; a client treats it as "still alive" and need
    /// not act. A write failure on the probe reaps a genuinely gone/wedged peer.
    Keepalive,
}

/// Frame `code` strings (distinct from the request/response `response_code`s).
pub mod code {
    /// A `snapshot` frame's code — never a failure.
    pub const OK: &str = "OK";
    /// Error frame: the pane died mid-subscription (its reader thread ended).
    pub const PANE_DIED: &str = "PANE_DIED";
    /// Error frame: the subscription's bounded queue overflowed (a slow consumer) and
    /// was dropped to keep the reader unblocked (MF-B). Re-`Attach` for a fresh snapshot.
    pub const OVERFLOW: &str = "OVERFLOW";
}

impl StreamFrame {
    /// Build the one-shot `snapshot` frame from the retained window + baseline.
    pub fn snapshot(id: &str, baseline: u64, data: &[u8]) -> Self {
        StreamFrame::Snapshot {
            id: id.to_string(),
            code: code::OK.to_string(),
            baseline,
            data_len: data.len(),
            data: b64_encode(data),
        }
    }

    /// Build a `delta` frame for one reader chunk.
    pub fn delta(id: &str, prev_total: u64, new_total: u64, data: &[u8]) -> Self {
        StreamFrame::Delta {
            id: id.to_string(),
            prev_total,
            new_total,
            data: b64_encode(data),
        }
    }

    /// Build an `error` frame closing one subscription.
    pub fn error(id: &str, code: &str, detail: &str) -> Self {
        StreamFrame::Error {
            id: id.to_string(),
            code: code.to_string(),
            detail: detail.to_string(),
        }
    }

    /// Build the idle-tick `keepalive` liveness probe.
    pub fn keepalive() -> Self {
        StreamFrame::Keepalive
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 encode (RFC 4648, `+/` alphabet, `=` padding). Self-contained so the
/// daemon needs no base64 dependency.
pub fn b64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64[((n >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Standard base64 decode (the inverse of [`b64_encode`]). `None` on any malformed
/// input (bad length, illegal char, misplaced padding). Used by the round-trip tests
/// (and available to an in-process client).
pub fn b64_decode(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let val = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        if pad > 2 {
            return None;
        }
        // padding only ever trails.
        if pad > 0 && (chunk[3] != b'=' || (pad == 2 && chunk[2] != b'=')) {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= if c == b'=' { 0 } else { val(c)? } << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_including_binary_and_newlines() {
        for case in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            b"line1\nline2\n",
            &[0u8, 255, 1, 254, 10, 13, 0],
        ] {
            let enc = b64_encode(case);
            assert_eq!(
                b64_decode(&enc).as_deref(),
                Some(case),
                "round-trip {case:?} via {enc}"
            );
            // a base64 string is itself newline-free → safe as a JSON line field.
            assert!(!enc.contains('\n'));
        }
    }

    #[test]
    fn base64_known_vectors() {
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_decode("Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn base64_decode_rejects_malformed() {
        assert!(b64_decode("Zm9v=").is_none(), "bad length");
        assert!(b64_decode("Zm9*").is_none(), "illegal char");
        assert!(b64_decode("Z===").is_none(), "too much padding");
    }

    #[test]
    fn snapshot_frame_serializes_with_tag_and_fields() {
        let f = StreamFrame::snapshot("ws-1", 6, b"hello ");
        let v: serde_json::Value = serde_json::to_value(&f).unwrap();
        assert_eq!(v["frame"], "snapshot");
        assert_eq!(v["id"], "ws-1");
        assert_eq!(v["code"], "OK");
        assert_eq!(v["baseline"], 6);
        assert_eq!(v["data_len"], 6);
        assert_eq!(b64_decode(v["data"].as_str().unwrap()).unwrap(), b"hello ");
    }

    #[test]
    fn delta_frame_serializes_with_totals() {
        let f = StreamFrame::delta("ws-1", 6, 11, b"world");
        let v: serde_json::Value = serde_json::to_value(&f).unwrap();
        assert_eq!(v["frame"], "delta");
        assert_eq!(v["prev_total"], 6);
        assert_eq!(v["new_total"], 11);
        assert_eq!(b64_decode(v["data"].as_str().unwrap()).unwrap(), b"world");
    }

    #[test]
    fn error_frame_serializes() {
        let v = serde_json::to_value(StreamFrame::error("ws-1", code::PANE_DIED, "gone")).unwrap();
        assert_eq!(v["frame"], "error");
        assert_eq!(v["code"], "PANE_DIED");
        assert_eq!(v["detail"], "gone");
    }
}
