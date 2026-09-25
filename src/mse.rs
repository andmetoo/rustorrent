use std::io::{ErrorKind, Read, Write};

use crate::sha1::Sha1;

pub enum CryptoMode {
    Plaintext,
    Rc4,
}

pub type AcceptOutcome = (CryptoMode, Option<CipherState>, [u8; 20], Vec<u8>, Vec<u8>);

/// Largest PadA/PadB/PadC/PadD length allowed by the MSE specification.
const MAX_PAD: usize = 512;
const CRYPTO_PLAINTEXT: u32 = 0x01;
const CRYPTO_RC4: u32 = 0x02;

#[derive(Clone)]
pub struct CipherState {
    enc: Rc4,
    dec: Rc4,
}

impl CipherState {
    #[cfg(test)]
    pub fn new(enc_key: &[u8], dec_key: &[u8]) -> Self {
        Self {
            enc: Rc4::new(enc_key),
            dec: Rc4::new(dec_key),
        }
    }

    pub fn encrypt(&mut self, data: &mut [u8]) {
        self.enc.apply(data);
    }

    pub fn decrypt(&mut self, data: &mut [u8]) {
        self.dec.apply(data);
    }
}

fn io_err(err: std::io::Error) -> String {
    format!("mse: {err}")
}

/// MSE/PE initiator handshake (outbound connection).
///
/// Follows the standard BEP MSE/PE protocol:
///   Step 1: Send Ya (DH public key)
///   Step 2: Read Yb (peer's DH public key)
///   Step 3: Send HASH('req1', S) + XOR'd hash + ENCRYPT(VC, crypto_provide, PadC, len(IA), IA)
///   Step 4: Read peer's ENCRYPT(VC, crypto_select, PadD)
///
/// `initial_payload` is the BT handshake (68 bytes) sent as IA in step 3.
/// After this returns, the peer's BT handshake is the next thing in the encrypted stream.
pub fn initiate<RW: Read + Write>(
    stream: &mut RW,
    info_hash: [u8; 20],
    allow_plain: bool,
    initial_payload: &[u8],
) -> Result<(CryptoMode, Option<CipherState>, Vec<u8>), String> {
    let ia_len =
        u16::try_from(initial_payload.len()).map_err(|_| "mse payload too large".to_string())?;
    // Step 1: Send Ya (96 bytes, no padding)
    let (private_key, public_key) = dh_generate()?;
    stream.write_all(&public_key).map_err(io_err)?;

    // Step 2: Read Yb (96 bytes)
    let mut peer_public = [0u8; 96];
    stream.read_exact(&mut peer_public).map_err(io_err)?;
    let shared = dh_shared(&peer_public, &private_key)?;

    // Initiator encrypts with keyA and decrypts with keyB.
    let mut enc = Rc4::new(&derive_key(b"keyA", &shared, &info_hash));
    let dec_key = derive_key(b"keyB", &shared, &info_hash);

    // Step 3: HASH('req1', S), HASH('req2', SKEY) xor HASH('req3', S), then
    // ENCRYPT(VC, crypto_provide, len(PadC) = 0, len(IA), IA).
    let provide = if allow_plain {
        CRYPTO_RC4 | CRYPTO_PLAINTEXT
    } else {
        CRYPTO_RC4
    };
    let mut message = Vec::with_capacity(40 + 16 + initial_payload.len());
    message.extend_from_slice(&hash2(b"req1", &shared));
    message.extend_from_slice(&xor20(
        &hash2(b"req2", &info_hash),
        &hash2(b"req3", &shared),
    ));
    message.extend_from_slice(&[0u8; 8]);
    message.extend_from_slice(&provide.to_be_bytes());
    message.extend_from_slice(&0u16.to_be_bytes());
    message.extend_from_slice(&ia_len.to_be_bytes());
    message.extend_from_slice(initial_payload);
    enc.apply(&mut message[40..]);
    stream.write_all(&message).map_err(io_err)?;

    // Step 4: the peer may have sent up to 512 bytes of PadB after Yb, so
    // synchronise on the encrypted verification constant.
    let mut dec = Rc4::new(&dec_key);
    let mut vc_pattern = [0u8; 8];
    dec.apply(&mut vc_pattern);
    let mut buffered = sync_to(stream, &vc_pattern)?;

    // crypto_select and len(PadD) are always RC4 encrypted, as is PadD.
    let mut header = [0u8; 6];
    read_buffered(stream, &mut buffered, &mut header)?;
    dec.apply(&mut header);
    let crypto_select = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let pad_d_len = usize::from(u16::from_be_bytes([header[4], header[5]]));
    if pad_d_len > MAX_PAD {
        return Err("mse PadD too large".to_string());
    }
    let mut pad = [0u8; MAX_PAD];
    read_buffered(stream, &mut buffered, &mut pad[..pad_d_len])?;
    dec.apply(&mut pad[..pad_d_len]);

    // Over-read bytes after PadD use the selected mode. PeerStream buffers
    // hold plaintext, so decrypt them (advancing the cipher) only for RC4.
    if crypto_select == CRYPTO_RC4 {
        dec.apply(&mut buffered);
        return Ok((CryptoMode::Rc4, Some(CipherState { enc, dec }), buffered));
    }
    if allow_plain && crypto_select == CRYPTO_PLAINTEXT {
        return Ok((CryptoMode::Plaintext, None, buffered));
    }
    Err("mse crypto selection failed".to_string())
}

/// MSE/PE responder handshake (inbound connection).
///
/// `first_byte` is the first byte already read from the stream (to distinguish
/// plaintext vs MSE). Returns the matched info_hash and the peer's initial
/// payload (BT handshake). After this returns, the caller should send their
/// BT handshake directly through the encrypted stream.
pub fn accept<RW: Read + Write>(
    stream: &mut RW,
    info_hashes: &[[u8; 20]],
    first_byte: u8,
    allow_plain: bool,
) -> Result<AcceptOutcome, String> {
    // Read Ya (first byte already consumed)
    let mut peer_public = [0u8; 96];
    peer_public[0] = first_byte;
    stream.read_exact(&mut peer_public[1..]).map_err(io_err)?;
    let (private_key, public_key) = dh_generate()?;
    let shared = dh_shared(&peer_public, &private_key)?;
    // Send Yb (no padding)
    stream.write_all(&public_key).map_err(io_err)?;

    // Skip PadA by synchronising on HASH('req1', S).
    let mut buffered = sync_to(stream, &hash2(b"req1", &shared))?;

    let mut obfuscated = [0u8; 20];
    read_buffered(stream, &mut buffered, &mut obfuscated)?;
    let req2 = xor20(&obfuscated, &hash2(b"req3", &shared));
    let info_hash = *info_hashes
        .iter()
        .find(|hash| hash2(b"req2", hash.as_slice()) == req2)
        .ok_or_else(|| "mse unknown info hash".to_string())?;

    // The responder decrypts with keyA and encrypts with keyB.
    let mut dec = Rc4::new(&derive_key(b"keyA", &shared, &info_hash));
    let mut enc = Rc4::new(&derive_key(b"keyB", &shared, &info_hash));

    // ENCRYPT(VC, crypto_provide, len(PadC), PadC, len(IA)), ENCRYPT(IA)
    let mut header = [0u8; 14];
    read_buffered(stream, &mut buffered, &mut header)?;
    dec.apply(&mut header);
    if header[..8] != [0u8; 8] {
        return Err("mse vc verification failed".to_string());
    }
    let crypto_provide = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
    let pad_c_len = usize::from(u16::from_be_bytes([header[12], header[13]]));
    if pad_c_len > MAX_PAD {
        return Err("mse PadC too large".to_string());
    }
    let mut pad = [0u8; MAX_PAD + 2];
    read_buffered(stream, &mut buffered, &mut pad[..pad_c_len + 2])?;
    dec.apply(&mut pad[..pad_c_len + 2]);
    let ia_len = usize::from(u16::from_be_bytes([pad[pad_c_len], pad[pad_c_len + 1]]));
    let mut ia = vec![0u8; ia_len];
    read_buffered(stream, &mut buffered, &mut ia)?;
    dec.apply(&mut ia);

    let crypto_select = if crypto_provide & CRYPTO_RC4 != 0 {
        CRYPTO_RC4
    } else if allow_plain && crypto_provide & CRYPTO_PLAINTEXT != 0 {
        CRYPTO_PLAINTEXT
    } else {
        return Err("mse no compatible crypto".to_string());
    };

    // ENCRYPT(VC, crypto_select, len(PadD) = 0)
    let mut response = [0u8; 14];
    response[8..12].copy_from_slice(&crypto_select.to_be_bytes());
    enc.apply(&mut response);
    stream.write_all(&response).map_err(io_err)?;

    if crypto_select == CRYPTO_RC4 {
        // PeerStream buffers hold plaintext. Only an RC4-selected stream has
        // encrypted bytes following IA; plaintext-selected bytes must remain
        // exactly as received.
        dec.apply(&mut buffered);
        let cipher = CipherState { enc, dec };
        return Ok((CryptoMode::Rc4, Some(cipher), info_hash, ia, buffered));
    }
    Ok((CryptoMode::Plaintext, None, info_hash, ia, buffered))
}

/// Reads until `pattern` begins within the first `MAX_PAD` bytes and returns
/// every byte received after it.
fn sync_to<R: Read>(stream: &mut R, pattern: &[u8]) -> Result<Vec<u8>, String> {
    let mut received = Vec::with_capacity(MAX_PAD + 64);
    let mut chunk = [0u8; MAX_PAD];
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) => return Err("mse sync: unexpected eof".to_string()),
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(io_err(err)),
        };
        // A match may straddle the previous read boundary.
        let search_start = received.len().saturating_sub(pattern.len() - 1);
        received.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_sync_pattern(&received, pattern, search_start, MAX_PAD) {
            return Ok(received.split_off(pos + pattern.len()));
        }
        if received.len() >= MAX_PAD + pattern.len() {
            return Err("mse sync failed".to_string());
        }
    }
}

/// `read_exact` that first consumes bytes already over-read into `buffered`.
fn read_buffered<R: Read>(
    stream: &mut R,
    buffered: &mut Vec<u8>,
    out: &mut [u8],
) -> Result<(), String> {
    let take = out.len().min(buffered.len());
    out[..take].copy_from_slice(&buffered[..take]);
    buffered.drain(..take);
    stream.read_exact(&mut out[take..]).map_err(io_err)
}

fn find_sync_pattern(
    data: &[u8],
    pattern: &[u8],
    search_start: usize,
    max_offset: usize,
) -> Option<usize> {
    let last = data.len().checked_sub(pattern.len())?.min(max_offset);
    (search_start..=last).find(|offset| data[*offset..*offset + pattern.len()] == *pattern)
}

fn hash2(prefix: &[u8], data: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(prefix);
    hasher.update(data);
    hasher.finalize()
}

fn derive_key(label: &[u8; 4], shared: &[u8; 96], info_hash: &[u8; 20]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(label);
    hasher.update(shared);
    hasher.update(info_hash);
    hasher.finalize()
}

fn xor20(a: &[u8; 20], b: &[u8; 20]) -> [u8; 20] {
    let mut out = *a;
    for (byte, other) in out.iter_mut().zip(b) {
        *byte ^= other;
    }
    out
}

/// Generates a 160-bit Diffie-Hellman private exponent (the MSE
/// specification considers anything beyond ~180 bits useless) and the
/// matching public key `2^x mod P`.
fn dh_generate() -> Result<([u8; 20], [u8; 96]), String> {
    let mut private_key = [0u8; 20];
    getrandom::fill(&mut private_key).map_err(|err| format!("mse random: {err}"))?;
    // Keep the exponent full-size so it is never trivially small.
    private_key[0] |= 0x80;
    Ok((private_key, to_be_bytes(&mod_pow(&TWO, &private_key))))
}

/// Validates the peer's public key and returns the 96-byte shared secret.
fn dh_shared(peer_public: &[u8; 96], private_key: &[u8]) -> Result<[u8; 96], String> {
    let peer = from_be_bytes(peer_public);
    // Reject 0, 1, P - 1 and anything >= P: these confine the shared secret
    // to a trivial subgroup or are not field elements at all.
    if less_than(&peer, &TWO) || !less_than(&peer, &P_MINUS_ONE) {
        return Err("mse invalid DH public key".to_string());
    }
    Ok(to_be_bytes(&mod_pow(&peer, private_key)))
}

// Fixed-size 768-bit arithmetic modulo the MSE prime, using Montgomery
// multiplication over little-endian 64-bit limbs. Exponentiation always
// performs both the square and the multiply and selects the result with a
// mask, so its running time does not depend on the secret exponent bits.
const LIMBS: usize = 12;
type U768 = [u64; LIMBS];

/// The 768-bit MSE prime P (little-endian limbs).
const P: U768 = [
    0x0000_0000_0009_0563,
    0xF44C_42E9_A63A_3621,
    0xE485_B576_625E_7EC6,
    0x4FE1_356D_6D51_C245,
    0x302B_0A6D_F25F_1437,
    0xEF95_19B3_CD3A_431B,
    0x514A_0879_8E34_04DD,
    0x020B_BEA6_3B13_9B22,
    0x2902_4E08_8A67_CC74,
    0xC4C6_628B_80DC_1CD1,
    0xC90F_DAA2_2168_C234,
    0xFFFF_FFFF_FFFF_FFFF,
];
const P_MINUS_ONE: U768 = {
    let mut value = P;
    value[0] -= 1;
    value
};
const ONE: U768 = {
    let mut value = [0u64; LIMBS];
    value[0] = 1;
    value
};
const TWO: U768 = {
    let mut value = [0u64; LIMBS];
    value[0] = 2;
    value
};
/// -P^-1 mod 2^64, by Newton iteration (each step doubles the correct bits).
const N0_INV: u64 = {
    let mut inverse = 1u64;
    let mut step = 0;
    while step < 6 {
        inverse = inverse.wrapping_mul(2u64.wrapping_sub(P[0].wrapping_mul(inverse)));
        step += 1;
    }
    inverse.wrapping_neg()
};
/// R^2 mod P with R = 2^768, used to enter the Montgomery domain.
const R2: U768 = {
    // R mod P = 2^768 - P, the two's complement of P.
    let mut value = [0u64; LIMBS];
    let mut carry = 1u64;
    let mut i = 0;
    while i < LIMBS {
        let (sum, overflow) = (!P[i]).overflowing_add(carry);
        value[i] = sum;
        carry = overflow as u64;
        i += 1;
    }
    // Doubling 768 times multiplies by another R.
    let mut doubling = 0;
    while doubling < 768 {
        let mut top = 0u64;
        let mut i = 0;
        while i < LIMBS {
            let next = value[i] >> 63;
            value[i] = (value[i] << 1) | top;
            top = next;
            i += 1;
        }
        if top == 1 || !less_than(&value, &P) {
            let mut borrow = 0u64;
            let mut i = 0;
            while i < LIMBS {
                let (diff, under1) = value[i].overflowing_sub(P[i]);
                let (diff, under2) = diff.overflowing_sub(borrow);
                value[i] = diff;
                borrow = (under1 | under2) as u64;
                i += 1;
            }
        }
        doubling += 1;
    }
    value
};

const fn less_than(a: &U768, b: &U768) -> bool {
    let mut i = LIMBS;
    while i > 0 {
        i -= 1;
        if a[i] != b[i] {
            return a[i] < b[i];
        }
    }
    false
}

fn from_be_bytes(bytes: &[u8; 96]) -> U768 {
    let mut out = [0u64; LIMBS];
    for (limb, chunk) in out.iter_mut().zip(bytes.as_chunks::<8>().0.iter().rev()) {
        *limb = u64::from_be_bytes(*chunk);
    }
    out
}

fn to_be_bytes(value: &U768) -> [u8; 96] {
    let mut out = [0u8; 96];
    for (chunk, limb) in out.as_chunks_mut::<8>().0.iter_mut().rev().zip(value) {
        *chunk = limb.to_be_bytes();
    }
    out
}

/// Montgomery product a * b * R^-1 mod P for a, b < P (CIOS method).
fn mont_mul(a: &U768, b: &U768) -> U768 {
    let mut t = [0u64; LIMBS + 2];
    for &word in b {
        let mut carry = 0u64;
        for (slot, &limb) in t.iter_mut().zip(a) {
            let sum = u128::from(*slot) + u128::from(limb) * u128::from(word) + u128::from(carry);
            *slot = sum as u64;
            carry = (sum >> 64) as u64;
        }
        let sum = u128::from(t[LIMBS]) + u128::from(carry);
        t[LIMBS] = sum as u64;
        t[LIMBS + 1] = (sum >> 64) as u64;

        let m = t[0].wrapping_mul(N0_INV);
        let mut carry = ((u128::from(t[0]) + u128::from(m) * u128::from(P[0])) >> 64) as u64;
        for j in 1..LIMBS {
            let sum = u128::from(t[j]) + u128::from(m) * u128::from(P[j]) + u128::from(carry);
            t[j - 1] = sum as u64;
            carry = (sum >> 64) as u64;
        }
        let sum = u128::from(t[LIMBS]) + u128::from(carry);
        t[LIMBS - 1] = sum as u64;
        t[LIMBS] = t[LIMBS + 1] + (sum >> 64) as u64;
    }
    // t < 2P: subtract P once when t >= P, selecting without branching.
    let mut reduced = [0u64; LIMBS];
    let mut borrow = 0u64;
    for ((out, &limb), &modulus) in reduced.iter_mut().zip(&t[..LIMBS]).zip(&P) {
        let (diff, under1) = limb.overflowing_sub(modulus);
        let (diff, under2) = diff.overflowing_sub(borrow);
        *out = diff;
        borrow = u64::from(under1 | under2);
    }
    let keep_reduced = 0u64.wrapping_sub((t[LIMBS] | (borrow ^ 1)) & 1);
    for (out, &limb) in reduced.iter_mut().zip(&t[..LIMBS]) {
        *out = (*out & keep_reduced) | (limb & !keep_reduced);
    }
    reduced
}

/// base^exponent mod P for base < P; `exponent` is big-endian.
fn mod_pow(base: &U768, exponent: &[u8]) -> U768 {
    let base = mont_mul(base, &R2);
    let mut acc = mont_mul(&ONE, &R2);
    for byte in exponent {
        for bit in (0..8).rev() {
            acc = mont_mul(&acc, &acc);
            let product = mont_mul(&acc, &base);
            let mask = 0u64.wrapping_sub(u64::from((byte >> bit) & 1));
            for (slot, value) in acc.iter_mut().zip(product) {
                *slot = (value & mask) | (*slot & !mask);
            }
        }
    }
    mont_mul(&acc, &ONE)
}

#[derive(Clone)]
struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    /// RC4 keyed for MSE, with the mandatory first 1024 bytes discarded.
    fn new(key: &[u8]) -> Self {
        let mut s = [0u8; 256];
        for (i, slot) in s.iter_mut().enumerate() {
            *slot = i as u8;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, usize::from(j));
        }
        let mut rc4 = Self { s, i: 0, j: 0 };
        let mut discard = [0u8; 256];
        for _ in 0..4 {
            rc4.apply(&mut discard);
        }
        rc4
    }

    fn apply(&mut self, data: &mut [u8]) {
        let (mut i, mut j) = (self.i, self.j);
        for byte in data {
            i = i.wrapping_add(1);
            let si = self.s[usize::from(i)];
            j = j.wrapping_add(si);
            let sj = self.s[usize::from(j)];
            self.s[usize::from(i)] = sj;
            self.s[usize::from(j)] = si;
            *byte ^= self.s[usize::from(si.wrapping_add(sj))];
        }
        self.i = i;
        self.j = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    use crate::peer_stream::PeerStream;

    /// Makes selected reads deterministic so a test can guarantee that the
    /// MSE scanner over-reads negotiation and post-negotiation bytes together.
    struct ExactReadTcp {
        inner: TcpStream,
        read_sizes: VecDeque<usize>,
    }

    impl ExactReadTcp {
        fn new(inner: TcpStream, read_sizes: impl IntoIterator<Item = usize>) -> Self {
            Self {
                inner,
                read_sizes: read_sizes.into_iter().collect(),
            }
        }
    }

    impl Read for ExactReadTcp {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let Some(read_len) = self.read_sizes.pop_front() else {
                return self.inner.read(buf);
            };
            if read_len > buf.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "scripted read is larger than destination",
                ));
            }
            self.inner.read_exact(&mut buf[..read_len])?;
            Ok(read_len)
        }
    }

    impl Write for ExactReadTcp {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    fn hex96(hex: &str) -> [u8; 96] {
        let mut out = [0u8; 96];
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap();
        }
        out
    }

    fn hex_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect()
    }

    /// Reference results computed with Python's `pow(base, exponent, P)`.
    #[test]
    fn modular_exponentiation_matches_reference_values() {
        assert_eq!(
            to_be_bytes(&P),
            hex96(
                "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD1\
                 29024E088A67CC74020BBEA63B139B22514A08798E3404DD\
                 EF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245\
                 E485B576625E7EC6F44C42E9A63A36210000000000090563"
            )
        );
        assert_eq!(P[0].wrapping_mul(N0_INV), u64::MAX);

        let x1 = hex_bytes("0102030405060708090a0b0c0d0e0f1011121314");
        assert_eq!(
            to_be_bytes(&mod_pow(&TWO, &x1)),
            hex96(
                "96e112dab29e8c5272accb9b17b26887ce54a144a4e3b697c7d159b7a817e556\
                 b0918db2b4c658e02a87f7e5fb14b18a553e084cbf3dad2d30f16596ccb982d4\
                 06258c61b30c5c1dae2ddc60bdbd48d79896312aad63238c39e1a633821eb693"
            )
        );

        let y2 = hex96(
            "0b30557a9fc4e90e33587da2c7ec11365b80a5caef14395e83a8cdf2173c6186\
             abd0f51a3f6489aed3f81d42678cb1d6fb20456a8fb4d9fe23486d92b7dc0126\
             4b7095badf04294e7398bde2072c51769bc0e50a2f54799ec3e80d32577ca1c6",
        );
        let x2 = hex_bytes("c8237ed9348fea45a0fb56b10c67c21d78d32e89");
        assert_eq!(
            dh_shared(&y2, &x2).unwrap(),
            hex96(
                "0a9fd0881d48ee35ad96cca9b6fe73090dec4ebe4db2b2976c79843d923cdbaf\
                 c4f56fd201aef742239723e779119bc2cc22b0a03681d0bb294c303af896edbd\
                 a478437790c43ec8ff11e426ad5a9af0b824d138560ba53e6678d9fe58734888"
            )
        );

        let mut p_minus_two = P;
        p_minus_two[0] -= 2;
        assert_eq!(
            dh_shared(&to_be_bytes(&p_minus_two), &[0xff; 20]).unwrap(),
            hex96(
                "016553ebd09b52b26d455c7162c744d1fdbc83ed348587ee7683c199b242bbb8\
                 aa768ed9578430d1cc909edacae9019f1df43ddeafca1bcf30ea5c24b3523169\
                 92a4780d17885acb65cea84c6d8591cb31e9efd2fe8c1c13c299fa0fbf9d57ea"
            )
        );

        let x4 = hex96(
            "0714212e3b4855626f7c8996a3b0bdcad7e4f1fe0b1825323f4c596673808d9a\
             a7b4c1cedbe8f5020f1c293643505d6a7784919eabb8c5d2dfecf90613202d3a\
             4754616e7b8895a2afbcc9d6e3f0fd0a1724313e4b5865727f8c99a6b3c0cdda",
        );
        assert_eq!(
            dh_shared(&y2, &x4).unwrap(),
            hex96(
                "44c67145c1aa11ee5c0dd7a6546445f5c0dfc71a3b26b8ffa31a2dc94607ba40\
                 f5fdac93170fb03dc647528d97944ebf917032a515f837a1a8210b633e00cfac\
                 df2f5d7275e442074c6ed44bafceec0b2210e8f356a986daec540ffd1fd25b53"
            )
        );
    }

    #[test]
    fn diffie_hellman_agrees_and_rejects_degenerate_public_keys() {
        let (private_a, public_a) = dh_generate().unwrap();
        let (private_b, public_b) = dh_generate().unwrap();
        assert_ne!(public_a, public_b);
        assert_eq!(
            dh_shared(&public_b, &private_a).unwrap(),
            dh_shared(&public_a, &private_b).unwrap()
        );

        let mut value = [0u8; 96];
        assert!(dh_shared(&value, &private_a).is_err());
        value[95] = 1;
        assert!(dh_shared(&value, &private_a).is_err());
        value[95] = 2;
        assert!(dh_shared(&value, &private_a).is_ok());
        let p_minus_one = to_be_bytes(&P_MINUS_ONE);
        assert!(dh_shared(&p_minus_one, &private_a).is_err());
        assert!(dh_shared(&to_be_bytes(&P), &private_a).is_err());
        assert!(dh_shared(&[0xff; 96], &private_a).is_err());
        let mut p_minus_two = p_minus_one;
        p_minus_two[95] -= 1;
        assert!(dh_shared(&p_minus_two, &private_a).is_ok());
    }

    #[test]
    fn xor_and_key_derivation_are_consistent() {
        assert_eq!(xor20(&[0xAAu8; 20], &[0x0Fu8; 20]), [0xA5u8; 20]);

        let shared = [3u8; 96];
        let info_hash = [9u8; 20];
        let mut expected = b"keyA".to_vec();
        expected.extend_from_slice(&shared);
        expected.extend_from_slice(&info_hash);
        assert_eq!(
            derive_key(b"keyA", &shared, &info_hash),
            crate::sha1::sha1(&expected)
        );
        assert_ne!(
            derive_key(b"keyA", &shared, &info_hash),
            derive_key(b"keyB", &shared, &info_hash)
        );
    }

    #[test]
    fn synchronization_pattern_cannot_bypass_padding_limit() {
        let pattern = b"marker";
        let mut data = vec![0u8; 520];
        data[512..518].copy_from_slice(pattern);
        assert_eq!(find_sync_pattern(&data, pattern, 0, 512), Some(512));

        let mut too_late = vec![0u8; 521];
        too_late[513..519].copy_from_slice(pattern);
        assert_eq!(find_sync_pattern(&too_late, pattern, 0, 512), None);
    }

    #[test]
    fn sync_finds_patterns_straddling_reads_and_bounds_padding() {
        let mut data = vec![0x55u8; 300];
        data.extend_from_slice(b"PATTERN!");
        data.extend_from_slice(b"rest");
        for chunk in [1, 5, 7, 64, 512] {
            let mut reader = SlowReader {
                data: data.clone(),
                chunk,
            };
            // Over-read bytes are returned; the rest stays in the stream.
            let mut after = sync_to(&mut reader, b"PATTERN!").unwrap();
            after.extend_from_slice(&reader.data);
            assert_eq!(after, b"rest", "chunk {chunk}");
        }

        let mut reader = SlowReader {
            data: vec![0u8; 2000],
            chunk: 97,
        };
        assert!(sync_to(&mut reader, b"PATTERN!").is_err());
    }

    struct SlowReader {
        data: Vec<u8>,
        chunk: usize,
    }

    impl Read for SlowReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.chunk.min(buf.len()).min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data.drain(..n);
            Ok(n)
        }
    }

    #[test]
    fn cipher_state_roundtrip() {
        let mut cipher = CipherState::new(b"key", b"key");
        let mut data = b"hello world".to_vec();
        let original = data.clone();
        cipher.encrypt(&mut data);
        assert_ne!(data, original);
        cipher.decrypt(&mut data);
        assert_eq!(data, original);
    }

    #[test]
    fn mse_initiate_accept_roundtrip_over_tcp() {
        let info_hash = [5u8; 20];
        let initial_payload = b"bt-handshake";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut first = [0u8; 1];
            stream.read_exact(&mut first).unwrap();
            let (mode, mut cipher, matched_hash, ia, buffered) =
                accept(&mut stream, &[info_hash], first[0], false).unwrap();
            assert!(matches!(mode, CryptoMode::Rc4));
            assert_eq!(matched_hash, info_hash);
            assert_eq!(ia, initial_payload);
            assert!(buffered.is_empty());

            let mut outbound = b"pong".to_vec();
            if let Some(c) = cipher.as_mut() {
                c.encrypt(&mut outbound);
            }
            stream.write_all(&outbound).unwrap();

            let mut inbound = [0u8; 4];
            stream.read_exact(&mut inbound).unwrap();
            if let Some(c) = cipher.as_mut() {
                c.decrypt(&mut inbound);
            }
            assert_eq!(&inbound, b"ping");
        });

        let mut client = PeerStream::tcp(TcpStream::connect(addr).unwrap());
        let (mode, cipher, buffered) =
            initiate(&mut client, info_hash, false, initial_payload).unwrap();
        assert!(matches!(mode, CryptoMode::Rc4));
        if let Some(cipher) = cipher {
            client.enable_encryption(cipher);
        }
        client.prepend_read_buffer(buffered);

        let mut inbound = [0u8; 4];
        client.read_exact(&mut inbound).unwrap();
        assert_eq!(&inbound, b"pong");

        client.write_all(b"ping").unwrap();
        server.join().unwrap();
    }

    #[test]
    fn initiate_preserves_plaintext_overread_after_pad_d() {
        let info_hash = [0x31u8; 20];
        let initial_payload = b"initiator-ia";
        let post_pad = b"plaintext-peer-handshake";
        let pad_d = [0xA1, 0xB2, 0xC3];
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let responder = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            let mut ya = [0u8; 96];
            stream.read_exact(&mut ya).unwrap();
            let (private, public) = dh_generate().unwrap();
            stream.write_all(&public).unwrap();
            let shared = dh_shared(&ya, &private).unwrap();

            let mut req1 = [0u8; 20];
            let mut obfuscated_req2 = [0u8; 20];
            stream.read_exact(&mut req1).unwrap();
            stream.read_exact(&mut obfuscated_req2).unwrap();
            assert_eq!(req1, hash2(b"req1", &shared));
            assert_eq!(
                obfuscated_req2,
                xor20(&hash2(b"req2", &info_hash), &hash2(b"req3", &shared),)
            );

            let initiator_key = derive_key(b"keyA", &shared, &info_hash);
            let responder_key = derive_key(b"keyB", &shared, &info_hash);
            let mut dec = Rc4::new(&initiator_key);

            let mut header = [0u8; 14];
            stream.read_exact(&mut header).unwrap();
            dec.apply(&mut header);
            assert_eq!(&header[..8], &[0u8; 8]);
            let offered = u32::from_be_bytes(header[8..12].try_into().unwrap());
            assert_ne!(offered & 0x01, 0);
            let pad_c_len = u16::from_be_bytes(header[12..14].try_into().unwrap()) as usize;
            let mut pad_c = vec![0u8; pad_c_len];
            stream.read_exact(&mut pad_c).unwrap();
            dec.apply(&mut pad_c);

            let mut ia_len = [0u8; 2];
            stream.read_exact(&mut ia_len).unwrap();
            dec.apply(&mut ia_len);
            let mut ia = vec![0u8; u16::from_be_bytes(ia_len) as usize];
            stream.read_exact(&mut ia).unwrap();
            dec.apply(&mut ia);
            assert_eq!(ia, initial_payload);

            let mut response = Vec::new();
            response.extend_from_slice(&[0u8; 8]);
            response.extend_from_slice(&1u32.to_be_bytes());
            response.extend_from_slice(&(pad_d.len() as u16).to_be_bytes());
            response.extend_from_slice(&pad_d);
            let mut enc = Rc4::new(&responder_key);
            enc.apply(&mut response);
            response.extend_from_slice(post_pad);
            stream.write_all(&response).unwrap();
        });

        let response_len = 8 + 4 + 2 + pad_d.len() + post_pad.len();
        let stream = TcpStream::connect(addr).unwrap();
        let mut stream = ExactReadTcp::new(stream, [96, response_len]);
        let (mode, cipher, buffered) =
            initiate(&mut stream, info_hash, true, initial_payload).unwrap();
        assert!(matches!(mode, CryptoMode::Plaintext));
        assert!(cipher.is_none());
        assert_eq!(buffered, post_pad);
        responder.join().unwrap();
    }

    #[test]
    fn accept_preserves_plaintext_overread_after_initial_payload() {
        let info_hash = [0x52u8; 20];
        let initial_payload = b"plaintext-ia";
        let post_ia = b"plaintext-message-after-ia";
        let pad_c = [0x19, 0x28, 0x37, 0x46];
        let transcript_len =
            20 + 20 + 8 + 4 + 2 + pad_c.len() + 2 + initial_payload.len() + post_ia.len();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let responder = thread::spawn(move || {
            let (mut raw_stream, _) = listener.accept().unwrap();
            let mut first = [0u8; 1];
            raw_stream.read_exact(&mut first).unwrap();
            let mut stream = ExactReadTcp::new(raw_stream, [95, transcript_len]);
            let (mode, cipher, matched_hash, ia, buffered) =
                accept(&mut stream, &[info_hash], first[0], true).unwrap();
            assert!(matches!(mode, CryptoMode::Plaintext));
            assert!(cipher.is_none());
            assert_eq!(matched_hash, info_hash);
            assert_eq!(ia, initial_payload);
            assert_eq!(buffered, post_ia);
        });

        let mut stream = TcpStream::connect(addr).unwrap();
        let (private, public) = dh_generate().unwrap();
        stream.write_all(&public).unwrap();

        let mut yb = [0u8; 96];
        stream.read_exact(&mut yb).unwrap();
        let shared = dh_shared(&yb, &private).unwrap();
        let initiator_key = derive_key(b"keyA", &shared, &info_hash);
        let responder_key = derive_key(b"keyB", &shared, &info_hash);

        let mut encrypted = Vec::new();
        encrypted.extend_from_slice(&[0u8; 8]);
        encrypted.extend_from_slice(&1u32.to_be_bytes());
        encrypted.extend_from_slice(&(pad_c.len() as u16).to_be_bytes());
        encrypted.extend_from_slice(&pad_c);
        encrypted.extend_from_slice(&(initial_payload.len() as u16).to_be_bytes());
        encrypted.extend_from_slice(initial_payload);
        let mut enc = Rc4::new(&initiator_key);
        enc.apply(&mut encrypted);

        let mut transcript = Vec::with_capacity(transcript_len);
        transcript.extend_from_slice(&hash2(b"req1", &shared));
        transcript.extend_from_slice(&xor20(
            &hash2(b"req2", &info_hash),
            &hash2(b"req3", &shared),
        ));
        transcript.extend_from_slice(&encrypted);
        transcript.extend_from_slice(post_ia);
        assert_eq!(transcript.len(), transcript_len);
        stream.write_all(&transcript).unwrap();

        let mut response = [0u8; 14];
        stream.read_exact(&mut response).unwrap();
        let mut dec = Rc4::new(&responder_key);
        dec.apply(&mut response);
        assert_eq!(&response[..8], &[0u8; 8]);
        assert_eq!(u32::from_be_bytes(response[8..12].try_into().unwrap()), 1);
        assert_eq!(u16::from_be_bytes(response[12..14].try_into().unwrap()), 0);

        responder.join().unwrap();
    }
}
