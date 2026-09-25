use std::cell::Cell;
use std::fmt;
use std::io::{Read, Write};

const PSTR: &str = "BitTorrent protocol";
const PSTR_LEN: usize = 19;
const HANDSHAKE_LEN: usize = 49 + PSTR_LEN;
const MAX_MESSAGE_LEN: usize = 2 * 1024 * 1024;
const MIN_READ: usize = 4 * 1024;
const MAX_READ: usize = 64 * 1024;
const REUSED_ENCODE_BUFFER_LIMIT: usize = 64 * 1024;
const EXTENSION_PROTOCOL_BIT: u8 = 0x10;
const HYBRID_V2_UPGRADE_BIT: u8 = 0x10;
const HASH_REQUEST_PAYLOAD_LEN: usize = 48;
const MAX_HASH_REQUEST_LENGTH: u32 = 512;
const MAX_HASH_TREE_LAYERS: u32 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub reserved: [u8; 8],
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
}

impl Handshake {
    pub fn supports_extensions(&self) -> bool {
        self.reserved[5] & EXTENSION_PROTOCOL_BIT != 0
    }

    /// BEP 52's upgrade signal: the fourth most-significant bit in the final
    /// reserved byte of a v1 hybrid-torrent handshake.
    pub fn supports_hybrid_v2_upgrade(&self) -> bool {
        self.reserved[7] & HYBRID_V2_UPGRADE_BIT != 0
    }
}

/// The fixed request tuple shared by BEP 52 hash-request and hash-reject
/// messages. All integer fields are encoded as four-byte big-endian values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashRequest {
    pub pieces_root: [u8; 32],
    pub base_layer: u32,
    pub index: u32,
    pub length: u32,
    pub proof_layers: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        block: Vec<u8>,
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    Port(u16),
    Extended {
        ext_id: u8,
        payload: Vec<u8>,
    },
    // BEP 6 - Fast Extension
    SuggestPiece(u32),
    HaveAll,
    HaveNone,
    RejectRequest {
        index: u32,
        begin: u32,
        length: u32,
    },
    AllowedFast(u32),
    // BEP 52 - v2 Merkle hash exchange.
    HashRequest(HashRequest),
    Hashes {
        request: HashRequest,
        hashes: Vec<[u8; 32]>,
    },
    HashReject(HashRequest),
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    InvalidHandshake,
    InvalidProtocol,
    InvalidMessage,
    InvalidLength,
    UnsupportedMessage(u8),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(err) => write!(f, "io error: {err}"),
            Error::InvalidHandshake => write!(f, "invalid handshake"),
            Error::InvalidProtocol => write!(f, "invalid protocol string"),
            Error::InvalidMessage => write!(f, "invalid message"),
            Error::InvalidLength => write!(f, "invalid message length"),
            Error::UnsupportedMessage(id) => write!(f, "unsupported message id {id}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

pub fn build_handshake(
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    extensions: bool,
) -> [u8; HANDSHAKE_LEN] {
    build_handshake_with_hybrid_upgrade(info_hash, peer_id, extensions, false)
}

pub fn build_handshake_with_hybrid_upgrade(
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    extensions: bool,
    hybrid_v2_upgrade: bool,
) -> [u8; HANDSHAKE_LEN] {
    let mut out = [0u8; HANDSHAKE_LEN];
    out[0] = PSTR_LEN as u8;
    out[1..1 + PSTR_LEN].copy_from_slice(PSTR.as_bytes());
    let reserved_start = 1 + PSTR_LEN;
    if extensions {
        out[reserved_start + 5] |= EXTENSION_PROTOCOL_BIT;
    }
    if hybrid_v2_upgrade {
        out[reserved_start + 7] |= HYBRID_V2_UPGRADE_BIT;
    }
    let info_start = reserved_start + 8;
    let peer_start = info_start + 20;
    out[info_start..peer_start].copy_from_slice(&info_hash);
    out[peer_start..peer_start + 20].copy_from_slice(&peer_id);
    out
}

pub fn parse_handshake(bytes: &[u8]) -> Result<Handshake, Error> {
    if bytes.len() != HANDSHAKE_LEN {
        return Err(Error::InvalidHandshake);
    }
    if bytes[0] as usize != PSTR_LEN {
        return Err(Error::InvalidHandshake);
    }
    if &bytes[1..1 + PSTR_LEN] != PSTR.as_bytes() {
        return Err(Error::InvalidProtocol);
    }
    let reserved_start = 1 + PSTR_LEN;
    let info_start = reserved_start + 8;
    let peer_start = info_start + 20;

    let mut reserved = [0u8; 8];
    reserved.copy_from_slice(&bytes[reserved_start..info_start]);
    let mut info_hash = [0u8; 20];
    info_hash.copy_from_slice(&bytes[info_start..peer_start]);
    let mut peer_id = [0u8; 20];
    peer_id.copy_from_slice(&bytes[peer_start..peer_start + 20]);

    Ok(Handshake {
        reserved,
        info_hash,
        peer_id,
    })
}

pub fn write_handshake<W: Write>(
    writer: &mut W,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    extensions: bool,
) -> Result<(), Error> {
    let data = build_handshake(info_hash, peer_id, extensions);
    writer.write_all(&data)?;
    Ok(())
}

pub fn write_handshake_with_hybrid_upgrade<W: Write>(
    writer: &mut W,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    extensions: bool,
    hybrid_v2_upgrade: bool,
) -> Result<(), Error> {
    let data =
        build_handshake_with_hybrid_upgrade(info_hash, peer_id, extensions, hybrid_v2_upgrade);
    writer.write_all(&data)?;
    Ok(())
}

pub fn read_handshake<R: Read>(reader: &mut R) -> Result<Handshake, Error> {
    let mut buf = [0u8; HANDSHAKE_LEN];
    reader.read_exact(&mut buf)?;
    parse_handshake(&buf)
}

pub fn write_message<W: Write>(writer: &mut W, message: &Message) -> Result<(), Error> {
    if encoded_payload_len(message).ok_or(Error::InvalidLength)? > MAX_MESSAGE_LEN {
        return Err(Error::InvalidLength);
    }
    // Peer threads send many small messages; reuse one encode buffer per
    // thread instead of allocating for every frame.
    thread_local! {
        static ENCODE_BUFFER: Cell<Vec<u8>> = const { Cell::new(Vec::new()) };
    }
    let mut buffer = ENCODE_BUFFER.take();
    buffer.clear();
    encode_into(message, &mut buffer);
    let result = writer.write_all(&buffer);
    if buffer.capacity() <= REUSED_ENCODE_BUFFER_LIMIT {
        ENCODE_BUFFER.set(buffer);
    }
    result.map_err(Error::Io)
}

fn encoded_payload_len(message: &Message) -> Option<usize> {
    match message {
        Message::KeepAlive => Some(0),
        Message::Choke
        | Message::Unchoke
        | Message::Interested
        | Message::NotInterested
        | Message::HaveAll
        | Message::HaveNone => Some(1),
        Message::Have(_) | Message::SuggestPiece(_) | Message::AllowedFast(_) => Some(5),
        Message::Bitfield(bits) => 1usize.checked_add(bits.len()),
        Message::Request { .. } | Message::Cancel { .. } | Message::RejectRequest { .. } => {
            Some(13)
        }
        Message::Piece { block, .. } => 9usize.checked_add(block.len()),
        Message::Port(_) => Some(3),
        Message::Extended { payload, .. } => 2usize.checked_add(payload.len()),
        Message::HashRequest(request) | Message::HashReject(request) => {
            validate_hash_request(request).then_some(1 + HASH_REQUEST_PAYLOAD_LEN)
        }
        Message::Hashes { request, hashes } => {
            if !validate_hashes(request, hashes) {
                return None;
            }
            (1 + HASH_REQUEST_PAYLOAD_LEN).checked_add(hashes.len().checked_mul(32)?)
        }
    }
}

#[allow(dead_code)]
pub fn read_message<R: Read>(reader: &mut R) -> Result<Message, Error> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return Ok(Message::KeepAlive);
    }
    if len > MAX_MESSAGE_LEN {
        return Err(Error::InvalidLength);
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    decode_message(&payload)
}

pub struct MessageReader {
    buf: Vec<u8>,
    start: usize,
}

impl MessageReader {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(64 * 1024),
            start: 0,
        }
    }

    pub fn read_message<R: Read>(&mut self, reader: &mut R) -> Result<Option<Message>, Error> {
        if let Some(message) = self.try_parse()? {
            return Ok(Some(message));
        }

        // Perform at most one socket read per call. A peer that supplies a
        // partial frame one byte at a time must not keep this function inside
        // an unbounded progress loop and prevent its caller from observing a
        // stop request or an absolute operation deadline. Read directly into
        // the frame buffer, sized to finish a partially received frame (such
        // as a 16 KiB piece) in one call where possible.
        let filled = self.buf.len();
        let want = self.missing_frame_bytes().clamp(MIN_READ, MAX_READ);
        self.buf.resize(filled + want, 0);
        let result = reader.read(&mut self.buf[filled..]);
        self.buf.truncate(filled + *result.as_ref().unwrap_or(&0));
        match result {
            Ok(0) => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "peer closed connection",
            ))),
            Ok(_) => self.try_parse(),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(None)
            }
            Err(err) => Err(Error::Io(err)),
        }
    }

    /// Bytes still needed to complete the frame at the head of the buffer.
    fn missing_frame_bytes(&self) -> usize {
        let pending = &self.buf[self.start..];
        match pending.first_chunk::<4>() {
            Some(header) => (u32::from_be_bytes(*header) as usize)
                .min(MAX_MESSAGE_LEN)
                .saturating_add(4)
                .saturating_sub(pending.len()),
            None => 0,
        }
    }

    fn try_parse(&mut self) -> Result<Option<Message>, Error> {
        let pending = &self.buf[self.start..];
        let Some(header) = pending.first_chunk::<4>() else {
            return Ok(None);
        };
        let len = u32::from_be_bytes(*header) as usize;
        if len > MAX_MESSAGE_LEN {
            return Err(Error::InvalidLength);
        }
        let total = 4 + len;
        if pending.len() < total {
            return Ok(None);
        }
        let message = if len == 0 {
            Ok(Message::KeepAlive)
        } else {
            decode_message(&pending[4..total])
        };
        self.consume(total);
        message.map(Some)
    }

    fn consume(&mut self, amount: usize) {
        self.start += amount;
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start >= 64 * 1024 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
    }
}

#[cfg(test)]
pub fn encode_message(message: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(message, &mut out);
    out
}

fn encode_into(message: &Message, out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    match message {
        Message::KeepAlive => {}
        Message::Choke => out.push(0),
        Message::Unchoke => out.push(1),
        Message::Interested => out.push(2),
        Message::NotInterested => out.push(3),
        Message::Have(index) => put_u32s(out, 4, &[*index]),
        Message::Bitfield(bits) => {
            out.push(5);
            out.extend_from_slice(bits);
        }
        Message::Request {
            index,
            begin,
            length,
        } => put_u32s(out, 6, &[*index, *begin, *length]),
        Message::Piece {
            index,
            begin,
            block,
        } => {
            put_u32s(out, 7, &[*index, *begin]);
            out.extend_from_slice(block);
        }
        Message::Cancel {
            index,
            begin,
            length,
        } => put_u32s(out, 8, &[*index, *begin, *length]),
        Message::Port(port) => {
            out.push(9);
            out.extend_from_slice(&port.to_be_bytes());
        }
        Message::Extended { ext_id, payload } => {
            out.extend_from_slice(&[20, *ext_id]);
            out.extend_from_slice(payload);
        }
        // BEP 6 - Fast Extension
        Message::SuggestPiece(index) => put_u32s(out, 13, &[*index]),
        Message::HaveAll => out.push(14),
        Message::HaveNone => out.push(15),
        Message::RejectRequest {
            index,
            begin,
            length,
        } => put_u32s(out, 16, &[*index, *begin, *length]),
        Message::AllowedFast(index) => put_u32s(out, 17, &[*index]),
        Message::HashRequest(request) => encode_hash_request(out, 21, request),
        Message::Hashes { request, hashes } => {
            encode_hash_request(out, 22, request);
            out.extend_from_slice(hashes.as_flattened());
        }
        Message::HashReject(request) => encode_hash_request(out, 23, request),
    }
    let len = (out.len() - start - 4) as u32;
    out[start..start + 4].copy_from_slice(&len.to_be_bytes());
}

fn put_u32s(out: &mut Vec<u8>, id: u8, values: &[u32]) {
    out.push(id);
    for value in values {
        out.extend_from_slice(&value.to_be_bytes());
    }
}

pub fn decode_message(payload: &[u8]) -> Result<Message, Error> {
    let (&id, data) = payload.split_first().ok_or(Error::InvalidMessage)?;
    // Fixed-size messages must match their length exactly.
    let exact = |len: usize| {
        if data.len() == len {
            Ok(())
        } else {
            Err(Error::InvalidMessage)
        }
    };
    match id {
        0 => exact(0).map(|_| Message::Choke),
        1 => exact(0).map(|_| Message::Unchoke),
        2 => exact(0).map(|_| Message::Interested),
        3 => exact(0).map(|_| Message::NotInterested),
        4 => exact(4).map(|_| Message::Have(read_u32(data, 0))),
        5 => Ok(Message::Bitfield(data.to_vec())),
        6 | 8 | 16 => {
            exact(12)?;
            let (index, begin, length) = (read_u32(data, 0), read_u32(data, 4), read_u32(data, 8));
            Ok(match id {
                6 => Message::Request {
                    index,
                    begin,
                    length,
                },
                8 => Message::Cancel {
                    index,
                    begin,
                    length,
                },
                _ => Message::RejectRequest {
                    index,
                    begin,
                    length,
                },
            })
        }
        7 => {
            if data.len() < 8 {
                return Err(Error::InvalidMessage);
            }
            Ok(Message::Piece {
                index: read_u32(data, 0),
                begin: read_u32(data, 4),
                block: data[8..].to_vec(),
            })
        }
        9 => exact(2).map(|_| Message::Port(u16::from_be_bytes([data[0], data[1]]))),
        // BEP 6 - Fast Extension
        13 => exact(4).map(|_| Message::SuggestPiece(read_u32(data, 0))),
        14 => exact(0).map(|_| Message::HaveAll),
        15 => exact(0).map(|_| Message::HaveNone),
        17 => exact(4).map(|_| Message::AllowedFast(read_u32(data, 0))),
        20 => {
            let (&ext_id, payload) = data.split_first().ok_or(Error::InvalidMessage)?;
            Ok(Message::Extended {
                ext_id,
                payload: payload.to_vec(),
            })
        }
        21 => Ok(Message::HashRequest(decode_hash_request(data)?)),
        22 => decode_hashes(data),
        23 => Ok(Message::HashReject(decode_hash_request(data)?)),
        other => Err(Error::UnsupportedMessage(other)),
    }
}

fn encode_hash_request(out: &mut Vec<u8>, id: u8, request: &HashRequest) {
    out.push(id);
    out.extend_from_slice(&request.pieces_root);
    for value in [
        request.base_layer,
        request.index,
        request.length,
        request.proof_layers,
    ] {
        out.extend_from_slice(&value.to_be_bytes());
    }
}

fn decode_hash_request(data: &[u8]) -> Result<HashRequest, Error> {
    if data.len() != HASH_REQUEST_PAYLOAD_LEN {
        return Err(Error::InvalidMessage);
    }
    let mut pieces_root = [0u8; 32];
    pieces_root.copy_from_slice(&data[..32]);
    let request = HashRequest {
        pieces_root,
        base_layer: read_u32(data, 32),
        index: read_u32(data, 36),
        length: read_u32(data, 40),
        proof_layers: read_u32(data, 44),
    };
    if !validate_hash_request(&request) {
        return Err(Error::InvalidMessage);
    }
    Ok(request)
}

fn decode_hashes(data: &[u8]) -> Result<Message, Error> {
    if data.len() < HASH_REQUEST_PAYLOAD_LEN {
        return Err(Error::InvalidMessage);
    }
    let (header, body) = data.split_at(HASH_REQUEST_PAYLOAD_LEN);
    let (hashes, rest) = body.as_chunks::<32>();
    if !rest.is_empty() {
        return Err(Error::InvalidMessage);
    }
    let request = decode_hash_request(header)?;
    if !validate_hashes(&request, hashes) {
        return Err(Error::InvalidMessage);
    }
    Ok(Message::Hashes {
        request,
        hashes: hashes.to_vec(),
    })
}

fn validate_hash_request(request: &HashRequest) -> bool {
    request.base_layer <= MAX_HASH_TREE_LAYERS
        && request.proof_layers <= MAX_HASH_TREE_LAYERS
        && request.length >= 2
        && request.length <= MAX_HASH_REQUEST_LENGTH
        && request.length.is_power_of_two()
        && request.index.is_multiple_of(request.length)
        && request.index.checked_add(request.length).is_some()
}

fn validate_hashes(request: &HashRequest, hashes: &[[u8; 32]]) -> bool {
    // Both counts are bounded by validate_hash_request (<= 512 and <= 64).
    let base_hashes = request.length as usize;
    validate_hash_request(request)
        && hashes.len() >= base_hashes
        && hashes.len() <= base_hashes + request.proof_layers as usize
}

/// Reads a big-endian u32 at `offset`; callers have checked the length.
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_be_bytes(word)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    struct OneByteReader {
        bytes: Vec<u8>,
        offset: usize,
        reads: usize,
    }

    impl Read for OneByteReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            if self.offset == self.bytes.len() {
                return Ok(0);
            }
            buf[0] = self.bytes[self.offset];
            self.offset += 1;
            Ok(1)
        }
    }

    struct SingleChunkReader {
        bytes: Option<Vec<u8>>,
        reads: usize,
    }

    impl Read for SingleChunkReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            let bytes = self
                .bytes
                .take()
                .expect("buffered message parsing performed an extra read");
            buf[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }

    fn hash_request_payload(id: u8, request: &HashRequest) -> Vec<u8> {
        let mut out = Vec::new();
        encode_hash_request(&mut out, id, request);
        out
    }

    #[test]
    fn message_reader_completes_a_piece_frame_in_one_read() {
        let piece = Message::Piece {
            index: 3,
            begin: 16384,
            block: vec![0xAB; 16 * 1024],
        };
        let mut bytes = encode_message(&piece);
        let tail = bytes.split_off(100);
        let mut reader = MessageReader::new();
        let mut head = SingleChunkReader {
            bytes: Some(bytes),
            reads: 0,
        };
        assert_eq!(reader.read_message(&mut head).unwrap(), None);
        let mut rest = SingleChunkReader {
            bytes: Some(tail),
            reads: 0,
        };
        assert_eq!(reader.read_message(&mut rest).unwrap(), Some(piece));
        assert_eq!(rest.reads, 1);
    }

    #[test]
    fn write_message_matches_encoding_and_reuses_buffer_safely() {
        let messages = [
            Message::Have(9),
            Message::Piece {
                index: 1,
                begin: 2,
                block: vec![7; 100_000],
            },
            Message::Request {
                index: 1,
                begin: 2,
                length: 3,
            },
        ];
        for message in &messages {
            let mut out = Vec::new();
            write_message(&mut out, message).unwrap();
            assert_eq!(out, encode_message(message));
            assert_eq!(decode_message(&out[4..]).unwrap(), *message);
        }
    }

    #[test]
    fn decoder_rejects_wrong_fixed_lengths_and_empty_extended() {
        for payload in [
            &[4u8, 0, 0, 0][..],
            &[6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            &[7, 0, 0, 0, 0, 0, 0, 0],
            &[9, 0],
            &[20],
            &[0, 1],
            &[],
        ] {
            assert!(decode_message(payload).is_err(), "{payload:?}");
        }
        assert_eq!(
            decode_message(&[16, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3]).unwrap(),
            Message::RejectRequest {
                index: 1,
                begin: 2,
                length: 3
            }
        );
    }

    #[test]
    fn handshake_roundtrip() {
        let info_hash = [1u8; 20];
        let peer_id = [2u8; 20];
        let bytes = build_handshake(info_hash, peer_id, true);
        let parsed = parse_handshake(&bytes).unwrap();
        assert_eq!(parsed.info_hash, info_hash);
        assert_eq!(parsed.peer_id, peer_id);
        assert!(parsed.supports_extensions());
        assert!(!parsed.supports_hybrid_v2_upgrade());
    }

    #[test]
    fn hybrid_upgrade_handshake_sets_only_the_bep52_reserved_bit() {
        let bytes = build_handshake_with_hybrid_upgrade([1u8; 20], [2u8; 20], true, true);
        let parsed = parse_handshake(&bytes).unwrap();
        assert!(parsed.supports_extensions());
        assert!(parsed.supports_hybrid_v2_upgrade());
        assert_eq!(parsed.reserved, [0, 0, 0, 0, 0, 0x10, 0, 0x10]);
    }

    #[test]
    fn message_roundtrip() {
        let hash_request = HashRequest {
            pieces_root: [9u8; 32],
            base_layer: 2,
            index: 0,
            length: 2,
            proof_layers: 3,
        };
        let messages = vec![
            Message::KeepAlive,
            Message::Choke,
            Message::Interested,
            Message::Have(42),
            Message::Request {
                index: 1,
                begin: 2,
                length: 3,
            },
            Message::Piece {
                index: 4,
                begin: 8,
                block: vec![1, 2, 3, 4],
            },
            Message::Extended {
                ext_id: 2,
                payload: b"hello".to_vec(),
            },
            Message::HashRequest(hash_request),
            Message::Hashes {
                request: hash_request,
                hashes: vec![[1u8; 32], [2u8; 32], [3u8; 32]],
            },
            Message::HashReject(hash_request),
        ];

        for msg in messages {
            let data = encode_message(&msg);
            let mut cursor = Cursor::new(data);
            let decoded = read_message(&mut cursor).unwrap();
            assert_eq!(decoded, msg);
        }
    }

    #[test]
    fn parse_handshake_rejects_wrong_protocol() {
        let mut bytes = build_handshake([1u8; 20], [2u8; 20], false);
        bytes[1] = b'X';
        assert!(matches!(
            parse_handshake(&bytes),
            Err(Error::InvalidProtocol)
        ));
    }

    #[test]
    fn read_message_rejects_oversized_length() {
        let len = (MAX_MESSAGE_LEN as u32).saturating_add(1);
        let data = len.to_be_bytes();
        let mut cursor = Cursor::new(data);
        assert!(matches!(
            read_message(&mut cursor),
            Err(Error::InvalidLength)
        ));
    }

    #[test]
    fn write_message_rejects_oversized_payload() {
        let message = Message::Extended {
            ext_id: 1,
            payload: vec![0; MAX_MESSAGE_LEN],
        };
        let mut out = Vec::new();
        assert!(matches!(
            write_message(&mut out, &message),
            Err(Error::InvalidLength)
        ));
        assert!(out.is_empty());
    }

    #[test]
    fn hash_messages_use_bep52_ids_and_fixed_header() {
        let request = HashRequest {
            pieces_root: [0xabu8; 32],
            base_layer: 4,
            index: 8,
            length: 4,
            proof_layers: 5,
        };
        let request_bytes = encode_message(&Message::HashRequest(request));
        assert_eq!(
            u32::from_be_bytes(request_bytes[..4].try_into().unwrap()),
            49
        );
        assert_eq!(request_bytes[4], 21);
        assert_eq!(&request_bytes[5..37], &[0xabu8; 32]);

        let hashes = vec![[1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]];
        let hashes_bytes = encode_message(&Message::Hashes {
            request,
            hashes: hashes.clone(),
        });
        assert_eq!(hashes_bytes[4], 22);
        assert_eq!(hashes_bytes.len(), 4 + 49 + hashes.len() * 32);

        let reject_bytes = encode_message(&Message::HashReject(request));
        assert_eq!(reject_bytes[4], 23);
    }

    #[test]
    fn hash_message_decoder_enforces_request_and_response_bounds() {
        let request = HashRequest {
            pieces_root: [7u8; 32],
            base_layer: 0,
            index: 0,
            length: 2,
            proof_layers: 1,
        };

        let mut invalid_length = hash_request_payload(21, &request);
        invalid_length[41..45].copy_from_slice(&513u32.to_be_bytes());
        assert!(matches!(
            decode_message(&invalid_length),
            Err(Error::InvalidMessage)
        ));

        let mut misaligned = hash_request_payload(21, &request);
        misaligned[37..41].copy_from_slice(&1u32.to_be_bytes());
        assert!(matches!(
            decode_message(&misaligned),
            Err(Error::InvalidMessage)
        ));

        let too_few_hashes = hash_request_payload(22, &request);
        assert!(matches!(
            decode_message(&too_few_hashes),
            Err(Error::InvalidMessage)
        ));

        let too_many_hashes = Message::Hashes {
            request,
            hashes: vec![[0u8; 32]; 4],
        };
        let mut out = Vec::new();
        assert!(matches!(
            write_message(&mut out, &too_many_hashes),
            Err(Error::InvalidLength)
        ));
        assert!(out.is_empty());
    }

    #[test]
    fn decode_message_rejects_unsupported_id() {
        assert!(matches!(
            decode_message(&[99]),
            Err(Error::UnsupportedMessage(99))
        ));
    }

    #[test]
    fn message_reader_parses_incremental_frames() {
        let mut reader = MessageReader::new();
        reader
            .buf
            .extend_from_slice(&encode_message(&Message::KeepAlive));
        reader
            .buf
            .extend_from_slice(&encode_message(&Message::Have(7)));

        let first = reader.try_parse().unwrap();
        let second = reader.try_parse().unwrap();
        let third = reader.try_parse().unwrap();

        assert_eq!(first, Some(Message::KeepAlive));
        assert_eq!(second, Some(Message::Have(7)));
        assert_eq!(third, None);
    }

    #[test]
    fn message_reader_returns_after_each_slow_trickle_read() {
        let mut source = OneByteReader {
            bytes: encode_message(&Message::Interested),
            offset: 0,
            reads: 0,
        };
        let mut reader = MessageReader::new();

        for expected_reads in 1..=4 {
            assert_eq!(reader.read_message(&mut source).unwrap(), None);
            assert_eq!(source.reads, expected_reads);
        }
        assert_eq!(
            reader.read_message(&mut source).unwrap(),
            Some(Message::Interested)
        );
        assert_eq!(source.reads, 5);
    }

    #[test]
    fn message_reader_parses_buffered_followup_without_another_read() {
        let mut bytes = encode_message(&Message::KeepAlive);
        bytes.extend_from_slice(&encode_message(&Message::Have(11)));
        let mut source = SingleChunkReader {
            bytes: Some(bytes),
            reads: 0,
        };
        let mut reader = MessageReader::new();

        assert_eq!(
            reader.read_message(&mut source).unwrap(),
            Some(Message::KeepAlive)
        );
        assert_eq!(
            reader.read_message(&mut source).unwrap(),
            Some(Message::Have(11))
        );
        assert_eq!(source.reads, 1);
    }
}
