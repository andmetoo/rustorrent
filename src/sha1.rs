pub struct Sha1 {
    state: [u32; 5],
    buffer: [u8; 64],
    buffer_len: usize,
    length: u64,
}

const INITIAL_STATE: [u32; 5] = [
    0x6745_2301,
    0xefcd_ab89,
    0x98ba_dcfe,
    0x1032_5476,
    0xc3d2_e1f0,
];

impl Sha1 {
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

    pub fn finalize(mut self) -> [u8; 20] {
        let mut tail = [0u8; 128];
        let used = self.buffer_len;
        tail[..used].copy_from_slice(&self.buffer[..used]);
        tail[used] = 0x80;
        let end = if used < 56 { 64 } else { 128 };
        tail[end - 8..end].copy_from_slice(&self.length.wrapping_mul(8).to_be_bytes());
        compress(&mut self.state, tail[..end].as_chunks::<64>().0);

        let mut out = [0u8; 20];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *chunk = word.to_be_bytes();
        }
        out
    }
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher.finalize()
}

fn compress(state: &mut [u32; 5], blocks: &[[u8; 64]]) {
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

fn compress_soft(state: &mut [u32; 5], blocks: &[[u8; 64]]) {
    for block in blocks {
        let mut w = [0u32; 16];
        for (word, bytes) in w.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        let [mut a, mut b, mut c, mut d, mut e] = *state;
        // Five rounds per iteration rotate the working variables by name
        // instead of moving them, which keeps the size-optimised loop fast.
        macro_rules! round {
            ($a:ident, $b:ident, $c:ident, $d:ident, $e:ident, $i:expr, $f:expr, $k:expr) => {
                let i = $i;
                if i >= 16 {
                    let next = w[(i + 13) & 15] ^ w[(i + 8) & 15] ^ w[(i + 2) & 15] ^ w[i & 15];
                    w[i & 15] = next.rotate_left(1);
                }
                $e = $e
                    .wrapping_add($a.rotate_left(5))
                    .wrapping_add($f($b, $c, $d))
                    .wrapping_add($k)
                    .wrapping_add(w[i & 15]);
                $b = $b.rotate_left(30);
            };
        }
        macro_rules! rounds20 {
            ($start:expr, $f:expr, $k:expr) => {
                for base in ($start..$start + 20).step_by(5) {
                    round!(a, b, c, d, e, base, $f, $k);
                    round!(e, a, b, c, d, base + 1, $f, $k);
                    round!(d, e, a, b, c, base + 2, $f, $k);
                    round!(c, d, e, a, b, base + 3, $f, $k);
                    round!(b, c, d, e, a, base + 4, $f, $k);
                }
            };
        }
        rounds20!(
            0,
            |b: u32, c: u32, d: u32| d ^ (b & (c ^ d)),
            0x5a82_7999u32
        );
        rounds20!(20, |b: u32, c: u32, d: u32| b ^ c ^ d, 0x6ed9_eba1u32);
        rounds20!(
            40,
            |b: u32, c: u32, d: u32| (b & c) | (d & (b | c)),
            0x8f1b_bcdcu32
        );
        rounds20!(60, |b: u32, c: u32, d: u32| b ^ c ^ d, 0xca62_c1d6u32);
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }
}

/// SHA-1 using the x86 SHA extensions (Intel SHA-NI / AMD Zen).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
unsafe fn compress_sha_ni(state: &mut [u32; 5], blocks: &[[u8; 64]]) {
    use std::arch::x86_64::*;

    macro_rules! rounds4 {
        ($h0:ident, $h1:ident, $wk:expr, $i:expr) => {
            _mm_sha1rnds4_epu32($h0, _mm_sha1nexte_epu32($h1, $wk), $i)
        };
    }
    macro_rules! schedule_rounds4 {
        ($h0:ident, $h1:ident, $w0:ident, $w1:ident, $w2:ident, $w3:ident, $w4:ident, $i:expr) => {
            $w4 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32($w0, $w1), $w2), $w3);
            $h1 = rounds4!($h0, $h1, $w4, $i);
        };
    }

    let mask = _mm_set_epi64x(0x0001_0203_0405_0607, 0x0809_0a0b_0c0d_0e0f);
    let mut abcd = _mm_set_epi32(
        state[0] as i32,
        state[1] as i32,
        state[2] as i32,
        state[3] as i32,
    );
    let mut e = _mm_set_epi32(state[4] as i32, 0, 0, 0);

    for block in blocks {
        let ptr = block.as_ptr().cast::<__m128i>();
        let mut w0 = _mm_shuffle_epi8(_mm_loadu_si128(ptr), mask);
        let mut w1 = _mm_shuffle_epi8(_mm_loadu_si128(ptr.add(1)), mask);
        let mut w2 = _mm_shuffle_epi8(_mm_loadu_si128(ptr.add(2)), mask);
        let mut w3 = _mm_shuffle_epi8(_mm_loadu_si128(ptr.add(3)), mask);
        let mut w4;

        let mut h0 = abcd;
        let mut h1 = _mm_add_epi32(e, w0);

        h1 = _mm_sha1rnds4_epu32(h0, h1, 0);
        h0 = rounds4!(h1, h0, w1, 0);
        h1 = rounds4!(h0, h1, w2, 0);
        h0 = rounds4!(h1, h0, w3, 0);
        schedule_rounds4!(h0, h1, w0, w1, w2, w3, w4, 0);

        schedule_rounds4!(h1, h0, w1, w2, w3, w4, w0, 1);
        schedule_rounds4!(h0, h1, w2, w3, w4, w0, w1, 1);
        schedule_rounds4!(h1, h0, w3, w4, w0, w1, w2, 1);
        schedule_rounds4!(h0, h1, w4, w0, w1, w2, w3, 1);
        schedule_rounds4!(h1, h0, w0, w1, w2, w3, w4, 1);

        schedule_rounds4!(h0, h1, w1, w2, w3, w4, w0, 2);
        schedule_rounds4!(h1, h0, w2, w3, w4, w0, w1, 2);
        schedule_rounds4!(h0, h1, w3, w4, w0, w1, w2, 2);
        schedule_rounds4!(h1, h0, w4, w0, w1, w2, w3, 2);
        schedule_rounds4!(h0, h1, w0, w1, w2, w3, w4, 2);

        schedule_rounds4!(h1, h0, w1, w2, w3, w4, w0, 3);
        schedule_rounds4!(h0, h1, w2, w3, w4, w0, w1, 3);
        schedule_rounds4!(h1, h0, w3, w4, w0, w1, w2, 3);
        schedule_rounds4!(h0, h1, w4, w0, w1, w2, w3, 3);
        schedule_rounds4!(h1, h0, w0, w1, w2, w3, w4, 3);

        abcd = _mm_add_epi32(abcd, h0);
        e = _mm_sha1nexte_epu32(h1, e);
    }

    state[0] = _mm_extract_epi32(abcd, 3) as u32;
    state[1] = _mm_extract_epi32(abcd, 2) as u32;
    state[2] = _mm_extract_epi32(abcd, 1) as u32;
    state[3] = _mm_extract_epi32(abcd, 0) as u32;
    state[4] = _mm_extract_epi32(e, 3) as u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha1_abc() {
        let hash = sha1(b"abc");
        assert_eq!(to_hex(&hash), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sha1_empty() {
        let hash = sha1(b"");
        assert_eq!(to_hex(&hash), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn sha1_two_block_padding_boundary() {
        assert_eq!(
            to_hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha1_chunked_matches_single_update() {
        let data = b"The quick brown fox jumps over the lazy dog";
        let expected = sha1(data);

        let mut hasher = Sha1::new();
        for chunk in data.chunks(3) {
            hasher.update(chunk);
        }
        let actual = hasher.finalize();
        assert_eq!(actual, expected);
        assert_eq!(to_hex(&actual), "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12");
    }

    #[test]
    fn sha1_long_message() {
        let mut hasher = Sha1::default();
        let chunk = [b'a'; 1_000];
        for _ in 0..1_000 {
            hasher.update(&chunk);
        }
        assert_eq!(
            to_hex(&hasher.finalize()),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
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
        // Split updates across every buffering boundary.
        let expected = sha1(&data[..300]);
        for split in 0..300 {
            let mut hasher = Sha1::new();
            hasher.update(&data[..split]);
            hasher.update(&data[split..300]);
            assert_eq!(hasher.finalize(), expected, "split {split}");
        }
    }
}
