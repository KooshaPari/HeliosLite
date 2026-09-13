//! `forge_sharecli::transport` — wire-protocol adapters sitting on top of
//! `ShareHub`.
//!
//! Two transports are provided:
//!
//! * [`sse`] — Server-Sent Events over HTTP/1.1. A GET on `/sse/<topic>`
//!   opens a long-lived response stream; a POST on `/publish/<topic>` reads
//!   a JSON body and forwards it to `ShareHub::publish`. All wire encoding
//!   is implemented by hand with `tokio::net::TcpStream`; no HTTP/WS crate
//!   is involved.
//! * [`ws`] — Raw RFC 6455 WebSocket frames. A GET on `/ws/<topic>` performs
//!   the standard `Upgrade` handshake (Sec-WebSocket-Accept computed with
//!   inline SHA-1) and then exchanges JSON envelopes
//!   `{ "topic": "...", "message": <ShareMessage> }` as text frames.
//!
//! Both transports:
//!
//! * subscribe to a [`crate::ShareHub`] topic and stream every published
//!   [`crate::ShareMessage`] to the client;
//! * accept inbound messages from the client and call
//!   [`crate::ShareHub::publish`];
//! * tolerate slow consumers via a bounded internal mpsc and a `tracing::warn!`
//!   when the buffer fills (lagged frame is dropped, the loop continues);
//! * are cancel-safe — dropping the inbound future terminates the connection
//!   without panic.
//!
//! Both are reachable through the `serve_sse` / `serve_ws` entry points,
//! each of which takes an already-accepted `tokio::net::TcpStream` and a
//! `ShareHub` handle, so they can be slotted behind any TCP acceptor
//! (e.g. `tokio::net::TcpListener`).

#![allow(clippy::module_name_repetitions)]

pub mod sse;
pub mod ws;

/// Maximum outbound frame payload kept in the bounded relay between the
/// `ShareHub` subscriber and the wire writer. When the consumer falls
/// behind by more than this, messages are dropped with a `tracing::warn!`
/// rather than blocking the publisher.
pub(crate) const RELAY_CAP: usize = 64;

/// Compute the `Sec-WebSocket-Accept` value from the client-supplied
/// `Sec-WebSocket-Key`, per RFC 6455 §1.3.
///
/// `key` is the raw value of the `Sec-WebSocket-Key` header (the base64
/// nonce the client sent). The accept value is
/// `base64(sha1(key ++ "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))`.
pub(crate) fn ws_accept_key(key: &str) -> String {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    const WS_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

    // The SHA-1 used here is the WebSocket protocol's mandated one — it is
    // *not* a security primitive (the handshake value is a hash of public
    // data), it is only a protocol constant. We implement it inline rather
    // than pull in a crypto crate, mirroring the precedent in
    // `forge_tracera::sink` for HMAC-SHA256.
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID);
    let digest = hasher.finalize();
    STANDARD.encode(digest)
}

// ---------------------------------------------------------------------------
// Minimal SHA-1 (RFC 3174) — WebSocket handshake only.
// ---------------------------------------------------------------------------
// Avoids pulling a crypto dep just to compute the protocol's well-known
// accept value. Mirrors the precedent set by `forge_tracera::sink` for
// HMAC-SHA256. The 64-byte block size and 20-byte digest size match the
// RFC, the round constants and initial state are verbatim from §5.3.1.
//
// The whole module is a tight fixed-size byte-level implementation of an
// unrolled RFC; every index access below is provably within a known-bounded
// `SHA1_BLOCK` / 80-word schedule. We suppress the workspace-wide
// `indexing_slicing` lint on the struct + impl rather than littering the
// computation with `unsafe { *_unchecked }` calls.
#[allow(clippy::indexing_slicing)]
const SHA1_BLOCK: usize = 64;
#[allow(clippy::indexing_slicing)]
const SHA1_DIGEST: usize = 20;

#[allow(clippy::indexing_slicing)]
pub(crate) struct Sha1 {
    h: [u32; 5],
    buf: [u8; SHA1_BLOCK],
    buf_len: usize,
    bit_len: u64,
}

#[allow(clippy::indexing_slicing)]
impl Sha1 {
    pub(crate) fn new() -> Self {
        Self {
            h: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
            buf: [0; SHA1_BLOCK],
            buf_len: 0,
            bit_len: 0,
        }
    }

    pub(crate) fn update(&mut self, mut data: &[u8]) {
        self.bit_len = self.bit_len.wrapping_add((data.len() as u64) * 8);

        // Drain into the partial block buffer first.
        if self.buf_len > 0 {
            let need = SHA1_BLOCK - self.buf_len;
            let take = need.min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == SHA1_BLOCK {
                let block = self.buf;
                self.process_block(&block);
                self.buf_len = 0;
            }
        }

        // Process full blocks straight from the input slice.
        while data.len() >= SHA1_BLOCK {
            let block: [u8; SHA1_BLOCK] = data[..SHA1_BLOCK].try_into().expect("64 bytes");
            self.process_block(&block);
            data = &data[SHA1_BLOCK..];
        }

        // Stash the trailing partial block.
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    pub(crate) fn finalize(mut self) -> [u8; SHA1_DIGEST] {
        // Append the 0x80 terminator, then zero-pad until length ≡ 56 (mod 64).
        let bit_len = self.bit_len;
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > SHA1_BLOCK - 8 {
            // Not enough room for the 64-bit length — pad to the end of this
            // block and start a new one.
            for b in &mut self.buf[self.buf_len..] {
                *b = 0;
            }
            let block = self.buf;
            self.process_block(&block);
            self.buf_len = 0;
        }
        for b in &mut self.buf[self.buf_len..SHA1_BLOCK - 8] {
            *b = 0;
        }
        self.buf[SHA1_BLOCK - 8..].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buf;
        self.process_block(&block);

        let mut out = [0u8; SHA1_DIGEST];
        for (i, v) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }

    fn process_block(&mut self, block: &[u8; SHA1_BLOCK]) {
        let mut w = [0u32; 80];
        for (i, slot) in w.iter_mut().take(16).enumerate() {
            *slot = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let [mut a, mut b, mut c, mut d, mut e] = self.h;
        #[allow(clippy::needless_range_loop)]
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
    }
}

#[cfg(test)]
mod handshake_tests {
    use super::ws_accept_key;

    // RFC 6455 §1.3 example handshake. The client sends key
    // "dGhlIHNhbXBsZSBub25jZQ==" and the server must answer with
    // "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".
    #[test]
    fn rfc6455_example_accept() {
        let got = ws_accept_key("dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(got, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
}
