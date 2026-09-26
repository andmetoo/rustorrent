pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    length: u64,
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const INITIAL_STATE: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: INITIAL_STATE,
            buffer: [0u8; 64],
            buffer_len: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        if self.buffer_len > 0 {
            let take = (64 - self.buffer_len).min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];
            if self.buffer_len < 64 {
                return;
            }
            let block = self.buffer;
            compress(&mut self.state, &[block]);
            self.buffer_len = 0;
        }
        // Whole blocks are hashed directly from the caller's slice.
        let (blocks, rest) = data.as_chunks::<64>();
        compress(&mut self.state, blocks);
        self.buffer[..rest.len()].copy_from_slice(rest);
        self.buffer_len = rest.len();
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let mut tail = [0u8; 128];
        let used = self.buffer_len;
        tail[..used].copy_from_slice(&self.buffer[..used]);
        tail[used] = 0x80;
        let end = if used < 56 { 64 } else { 128 };
        tail[end - 8..end].copy_from_slice(&self.length.wrapping_mul(8).to_be_bytes());
        compress(&mut self.state, tail[..end].as_chunks::<64>().0);

        let mut out = [0u8; 32];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *chunk = word.to_be_bytes();
        }
        out
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize()
}

fn compress(state: &mut [u32; 8], blocks: &[[u8; 64]]) {
    if blocks.is_empty() {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("sha") && std::is_x86_feature_detected!("sse4.1") {
        // SAFETY: the required CPU features were detected at runtime.
        unsafe { compress_sha_ni(state, blocks) };
        return;
    }
    compress_soft(state, blocks);
}

fn compress_soft(state: &mut [u32; 8], blocks: &[[u8; 64]]) {
    for block in blocks {
        let mut w = [0u32; 16];
        for (word, bytes) in w.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        // Eight rounds per iteration rotate the working variables by name
        // instead of moving them, which keeps the size-optimised loop fast.
        macro_rules! round {
            ($a:ident, $b:ident, $c:ident, $d:ident, $e:ident, $f:ident, $g:ident, $h:ident, $i:expr) => {
                let i = $i;
                if i >= 16 {
                    let w15 = w[(i + 1) & 15];
                    let w2 = w[(i + 14) & 15];
                    let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
                    let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
                    w[i & 15] = w[i & 15]
                        .wrapping_add(s0)
                        .wrapping_add(w[(i + 9) & 15])
                        .wrapping_add(s1);
                }
                let t1 = $h
                    .wrapping_add($e.rotate_right(6) ^ $e.rotate_right(11) ^ $e.rotate_right(25))
                    .wrapping_add($g ^ ($e & ($f ^ $g)))
                    .wrapping_add(K[i])
                    .wrapping_add(w[i & 15]);
                let t2 = ($a.rotate_right(2) ^ $a.rotate_right(13) ^ $a.rotate_right(22))
                    .wrapping_add(($a & $b) | ($c & ($a | $b)));
                $d = $d.wrapping_add(t1);
                $h = t1.wrapping_add(t2);
            };
        }
        for base in (0..64).step_by(8) {
            round!(a, b, c, d, e, f, g, h, base);
            round!(h, a, b, c, d, e, f, g, base + 1);
            round!(g, h, a, b, c, d, e, f, base + 2);
            round!(f, g, h, a, b, c, d, e, base + 3);
            round!(e, f, g, h, a, b, c, d, base + 4);
            round!(d, e, f, g, h, a, b, c, base + 5);
            round!(c, d, e, f, g, h, a, b, base + 6);
            round!(b, c, d, e, f, g, h, a, base + 7);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
}
/// SHA-256 using the x86 SHA extensions (Intel SHA-NI / AMD Zen).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
unsafe fn compress_sha_ni(state: &mut [u32; 8], blocks: &[[u8; 64]]) {
    use std::arch::x86_64::*;

    macro_rules! rounds4 {
        ($abef:ident, $cdgh:ident, $w:expr, $i:expr) => {{
            let t1 = _mm_add_epi32($w, _mm_loadu_si128(K.as_ptr().add(4 * $i).cast()));
            $cdgh = _mm_sha256rnds2_epu32($cdgh, $abef, t1);
            $abef = _mm_sha256rnds2_epu32($abef, $cdgh, _mm_shuffle_epi32(t1, 0x0e));
        }};
    }
    macro_rules! schedule_rounds4 {
        ($abef:ident, $cdgh:ident, $w0:ident, $w1:ident, $w2:ident, $w3:ident, $w4:ident, $i:expr) => {{
            $w4 = _mm_sha256msg2_epu32(
                _mm_add_epi32(_mm_sha256msg1_epu32($w0, $w1), _mm_alignr_epi8($w3, $w2, 4)),
                $w3,
            );
            rounds4!($abef, $cdgh, $w4, $i);
        }};
    }

    let mask = _mm_set_epi64x(
        0x0c0d_0e0f_0809_0a0bu64 as i64,
        0x0405_0607_0001_0203u64 as i64,
    );
    let state_ptr = state.as_ptr().cast::<__m128i>();
    let cdab = _mm_shuffle_epi32(_mm_loadu_si128(state_ptr), 0xb1);
    let efgh = _mm_shuffle_epi32(_mm_loadu_si128(state_ptr.add(1)), 0x1b);
    let mut abef = _mm_alignr_epi8(cdab, efgh, 8);
    let mut cdgh = _mm_blend_epi16(efgh, cdab, 0xf0);

    for block in blocks {
        let abef_save = abef;
        let cdgh_save = cdgh;
        let ptr = block.as_ptr().cast::<__m128i>();
        let mut w0 = _mm_shuffle_epi8(_mm_loadu_si128(ptr), mask);
        let mut w1 = _mm_shuffle_epi8(_mm_loadu_si128(ptr.add(1)), mask);
        let mut w2 = _mm_shuffle_epi8(_mm_loadu_si128(ptr.add(2)), mask);
        let mut w3 = _mm_shuffle_epi8(_mm_loadu_si128(ptr.add(3)), mask);
        let mut w4;

        rounds4!(abef, cdgh, w0, 0);
        rounds4!(abef, cdgh, w1, 1);
        rounds4!(abef, cdgh, w2, 2);
        rounds4!(abef, cdgh, w3, 3);
        schedule_rounds4!(abef, cdgh, w0, w1, w2, w3, w4, 4);
        schedule_rounds4!(abef, cdgh, w1, w2, w3, w4, w0, 5);
        schedule_rounds4!(abef, cdgh, w2, w3, w4, w0, w1, 6);
        schedule_rounds4!(abef, cdgh, w3, w4, w0, w1, w2, 7);
        schedule_rounds4!(abef, cdgh, w4, w0, w1, w2, w3, 8);
        schedule_rounds4!(abef, cdgh, w0, w1, w2, w3, w4, 9);
        schedule_rounds4!(abef, cdgh, w1, w2, w3, w4, w0, 10);
        schedule_rounds4!(abef, cdgh, w2, w3, w4, w0, w1, 11);
        schedule_rounds4!(abef, cdgh, w3, w4, w0, w1, w2, 12);
        schedule_rounds4!(abef, cdgh, w4, w0, w1, w2, w3, 13);
        schedule_rounds4!(abef, cdgh, w0, w1, w2, w3, w4, 14);
        schedule_rounds4!(abef, cdgh, w1, w2, w3, w4, w0, 15);

        abef = _mm_add_epi32(abef, abef_save);
        cdgh = _mm_add_epi32(cdgh, cdgh_save);
    }

    let feba = _mm_shuffle_epi32(abef, 0x1b);
    let dchg = _mm_shuffle_epi32(cdgh, 0xb1);
    let state_ptr = state.as_mut_ptr().cast::<__m128i>();
    _mm_storeu_si128(state_ptr, _mm_blend_epi16(feba, dchg, 0xf0));
    _mm_storeu_si128(state_ptr.add(1), _mm_alignr_epi8(dchg, feba, 8));
}

/// Hash a BEP 52 logical piece. The v2 piece hash is a Merkle-tree node over
/// 16 KiB block hashes, not SHA-256 over the whole piece when pieces are larger
/// than 16 KiB. Missing blocks in the final piece are represented by zero
/// hashes, as required by BEP 52.
pub fn merkle_piece_root(data: &[u8], piece_length: u32) -> Option<[u8; 32]> {
    const BLOCK_LENGTH: usize = 16 * 1024;

    let piece_length = piece_length as usize;
    if data.is_empty()
        || piece_length < BLOCK_LENGTH
        || !piece_length.is_power_of_two()
        || data.len() > piece_length
    {
        return None;
    }

    let leaf_count = piece_length / BLOCK_LENGTH;
    let mut layer = Vec::with_capacity(leaf_count);
    layer.extend(data.chunks(BLOCK_LENGTH).map(sha256));
    layer.resize(leaf_count, [0u8; 32]);
    reduce_merkle_layer(layer)
}

/// Reconstruct a file's BEP 52 root from its piece layer. `piece_hashes`
/// contains only hashes which cover file data; omitted balancing nodes are
/// supplied using the zero hash for the selected piece layer.
pub fn merkle_root_from_piece_layer(
    piece_hashes: &[[u8; 32]],
    piece_length: u32,
) -> Option<[u8; 32]> {
    const BLOCK_LENGTH: u32 = 16 * 1024;

    if piece_hashes.is_empty() || piece_length < BLOCK_LENGTH || !piece_length.is_power_of_two() {
        return None;
    }

    let mut padding_hash = [0u8; 32];
    let mut covered = BLOCK_LENGTH;
    while covered < piece_length {
        padding_hash = hash_pair(&padding_hash, &padding_hash);
        covered = covered.checked_mul(2)?;
    }

    let width = piece_hashes.len().checked_next_power_of_two()?;
    let mut layer = Vec::with_capacity(width);
    layer.extend_from_slice(piece_hashes);
    layer.resize(width, padding_hash);
    reduce_merkle_layer(layer)
}

fn reduce_merkle_layer(mut layer: Vec<[u8; 32]>) -> Option<[u8; 32]> {
    if layer.is_empty() || !layer.len().is_power_of_two() {
        return None;
    }
    while layer.len() > 1 {
        let mut next = Vec::with_capacity(layer.len() / 2);
        for pair in layer.as_chunks::<2>().0 {
            next.push(hash_pair(&pair[0], &pair[1]));
        }
        layer = next;
    }
    layer.pop()
}

fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut bytes = [0u8; 64];
    bytes[..32].copy_from_slice(left);
    bytes[32..].copy_from_slice(right);
    sha256(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn accelerated_and_portable_compression_agree() {
        let data: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(131) >> 3) as u8)
            .collect();
        for len in [0usize, 1, 55, 56, 63, 64, 65, 119, 120, 128, 1000, 4096] {
            let blocks = data[..len].as_chunks::<64>().0;
            let mut fast = INITIAL_STATE;
            compress(&mut fast, blocks);
            let mut soft = INITIAL_STATE;
            compress_soft(&mut soft, blocks);
            assert_eq!(fast, soft, "length {len}");
        }
        let expected = sha256(&data[..300]);
        for split in 0..300 {
            let mut hasher = Sha256::new();
            hasher.update(&data[..split]);
            hasher.update(&data[split..300]);
            assert_eq!(hasher.finalize(), expected, "split {split}");
        }
    }

    #[test]
    fn sha256_empty() {
        let hash = sha256(b"");
        assert_eq!(
            to_hex(&hash),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_abc() {
        let hash = sha256(b"abc");
        assert_eq!(
            to_hex(&hash),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_448_bits() {
        let hash = sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(
            to_hex(&hash),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_chunked_matches_single_update() {
        let data = b"The quick brown fox jumps over the lazy dog";
        let expected = sha256(data);

        let mut hasher = Sha256::new();
        for chunk in data.chunks(3) {
            hasher.update(chunk);
        }
        let actual = hasher.finalize();
        assert_eq!(actual, expected);
        assert_eq!(
            to_hex(&actual),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
    }

    #[test]
    fn sha256_long_message() {
        // 1 million 'a' characters
        let data = vec![b'a'; 1_000_000];
        let hash = sha256(&data);
        assert_eq!(
            to_hex(&hash),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn bep52_piece_hash_uses_merkle_nodes_above_16k() {
        let data = vec![b'a'; 32 * 1024];
        let left = sha256(&data[..16 * 1024]);
        let right = sha256(&data[16 * 1024..]);
        assert_eq!(
            merkle_piece_root(&data, 32 * 1024),
            Some(hash_pair(&left, &right))
        );
        assert_ne!(merkle_piece_root(&data, 32 * 1024), Some(sha256(&data)));
    }

    #[test]
    fn bep52_short_final_piece_is_zero_hash_padded() {
        let data = vec![b'z'; 16 * 1024 + 7];
        let left = sha256(&data[..16 * 1024]);
        let right = sha256(&data[16 * 1024..]);
        let expected = hash_pair(&left, &right);
        assert_eq!(merkle_piece_root(&data, 32 * 1024), Some(expected));

        let third_piece = sha256(b"third");
        let layer = [expected, third_piece];
        assert_eq!(
            merkle_root_from_piece_layer(&layer, 32 * 1024),
            Some(hash_pair(&expected, &third_piece))
        );
    }
}
