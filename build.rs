//! Precompresses the embedded web interface assets and the search runtime
//! with gzip so the binary carries (and the server sends) a fraction of
//! their size. The small
//! DEFLATE encoder below avoids a build dependency.

use std::env;
use std::fs;
use std::path::Path;

const ASSETS: [&str; 2] = ["app.css", "app.js"];
const SEARCH_RUNTIME: [&str; 5] = [
    "helpers.py",
    "nova2.py",
    "nova2dl.py",
    "novaprinter.py",
    "socks.py",
];

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let mut tag = 0u32;
    for name in ASSETS {
        tag = crc32(tag, &compress("assets/ui", name, &out_dir));
    }
    println!("cargo:rustc-env=UI_ASSET_TAG={tag:08x}");
    for name in SEARCH_RUNTIME {
        compress("assets/search_runtime", name, &out_dir);
    }
    println!("cargo:rerun-if-changed=build.rs");
}

/// Writes `<OUT_DIR>/<name>.gz` and returns the compressed bytes.
fn compress(dir: &str, name: &str, out_dir: &str) -> Vec<u8> {
    let source = Path::new(dir).join(name);
    println!("cargo:rerun-if-changed={}", source.display());
    let data = fs::read(&source).unwrap_or_else(|err| panic!("{}: {err}", source.display()));
    let gz = gzip(&data);
    fs::write(Path::new(out_dir).join(format!("{name}.gz")), &gz).expect("write asset");
    gz
}

fn crc32(seed: u32, data: &[u8]) -> u32 {
    let mut crc = !seed;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 255];
    out.extend(deflate(data));
    out.extend(crc32(0, data).to_le_bytes());
    out.extend((data.len() as u32).to_le_bytes());
    out
}

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        self.acc |= u64::from(value) << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// Huffman codes are sent most-significant bit first.
    fn code(&mut self, code: u32, len: u32) {
        self.put(code.reverse_bits() >> (32 - len), len);
    }
}

enum Token {
    Lit(u8),
    Match(usize, usize),
}

const LEN_BASE: [usize; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [usize; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn bucket(table: &[usize], value: usize) -> usize {
    table.iter().rposition(|&base| base <= value).unwrap_or(0)
}

/// Greedy LZ77 with hash chains and one step of lazy matching.
fn tokenize(data: &[u8]) -> Vec<Token> {
    const WINDOW: usize = 32768;
    let hash = |i: usize| {
        ((usize::from(data[i]) << 10) ^ (usize::from(data[i + 1]) << 5) ^ usize::from(data[i + 2]))
            & 0x7fff
    };
    let mut head = vec![usize::MAX; 0x8000];
    let mut prev = vec![usize::MAX; data.len()];
    let longest = |head: &[usize], prev: &[usize], i: usize| -> (usize, usize) {
        let mut best = (0, 0);
        if i + 3 > data.len() {
            return best;
        }
        let mut candidate = head[hash(i)];
        let mut chain = 1024;
        while candidate != usize::MAX && i - candidate <= WINDOW && chain > 0 {
            let max = (data.len() - i).min(258);
            let mut len = 0;
            while len < max && data[candidate + len] == data[i + len] {
                len += 1;
            }
            if len > best.0 {
                best = (len, i - candidate);
                if len == max {
                    break;
                }
            }
            candidate = prev[candidate];
            chain -= 1;
        }
        best
    };
    let insert = |head: &mut [usize], prev: &mut [usize], i: usize| {
        if i + 3 <= data.len() {
            let h = hash(i);
            prev[i] = head[h];
            head[h] = i;
        }
    };
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let (len, dist) = longest(&head, &prev, i);
        insert(&mut head, &mut prev, i);
        if len >= 3 {
            let (next_len, _) = longest(&head, &prev, i + 1);
            if next_len > len + 1 {
                tokens.push(Token::Lit(data[i]));
                i += 1;
                continue;
            }
            tokens.push(Token::Match(len, dist));
            for j in i + 1..i + len {
                insert(&mut head, &mut prev, j);
            }
            i += len;
        } else {
            tokens.push(Token::Lit(data[i]));
            i += 1;
        }
    }
    tokens
}

/// Huffman code lengths limited to `limit` bits (frequencies are flattened
/// until the tree fits).
fn code_lengths(freq: &[u32], limit: u8) -> Vec<u8> {
    let mut scaled: Vec<u64> = freq.iter().map(|&f| u64::from(f)).collect();
    loop {
        let mut lengths = vec![0u8; freq.len()];
        let mut nodes: Vec<(u64, Vec<usize>)> = scaled
            .iter()
            .enumerate()
            .filter(|(_, &f)| f > 0)
            .map(|(symbol, &f)| (f, vec![symbol]))
            .collect();
        if nodes.len() == 1 {
            lengths[nodes[0].1[0]] = 1;
            return lengths;
        }
        while nodes.len() > 1 {
            nodes.sort_by_key(|node| std::cmp::Reverse(node.0));
            let (fa, sa) = nodes.pop().expect("node");
            let (fb, sb) = nodes.pop().expect("node");
            for &symbol in sa.iter().chain(&sb) {
                lengths[symbol] += 1;
            }
            nodes.push((fa + fb, [sa, sb].concat()));
        }
        if lengths.iter().all(|&l| l <= limit) {
            return lengths;
        }
        for f in scaled.iter_mut().filter(|f| **f > 0) {
            *f = (*f >> 1).max(1);
        }
    }
}

fn canonical(lengths: &[u8]) -> Vec<u32> {
    let mut count = [0u32; 16];
    for &l in lengths {
        count[usize::from(l)] += 1;
    }
    count[0] = 0;
    let mut next = [0u32; 16];
    let mut code = 0;
    for bits in 1..16 {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    lengths
        .iter()
        .map(|&l| {
            if l == 0 {
                0
            } else {
                let c = next[usize::from(l)];
                next[usize::from(l)] += 1;
                c
            }
        })
        .collect()
}

/// Run-length encodes code lengths with symbols 16, 17 and 18.
fn rle(lengths: &[u8]) -> Vec<(u8, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lengths.len() {
        let l = lengths[i];
        let mut run = 1;
        while i + run < lengths.len() && lengths[i + run] == l {
            run += 1;
        }
        i += run;
        if l == 0 {
            while run >= 11 {
                let n = run.min(138);
                out.push((18, (n - 11) as u32));
                run -= n;
            }
            if run >= 3 {
                out.push((17, (run - 3) as u32));
                run = 0;
            }
        } else {
            out.push((l, 0));
            run -= 1;
            while run >= 3 {
                let n = run.min(6);
                out.push((16, (n - 3) as u32));
                run -= n;
            }
        }
        for _ in 0..run {
            out.push((l, 0));
        }
    }
    out
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let tokens = tokenize(data);
    let mut lit_freq = [0u32; 286];
    let mut dist_freq = [0u32; 30];
    for token in &tokens {
        match *token {
            Token::Lit(b) => lit_freq[usize::from(b)] += 1,
            Token::Match(len, dist) => {
                lit_freq[257 + bucket(&LEN_BASE, len)] += 1;
                dist_freq[bucket(&DIST_BASE, dist)] += 1;
            }
        }
    }
    lit_freq[256] = 1;
    if dist_freq.iter().all(|&f| f == 0) {
        dist_freq[0] = 1;
    }
    let lit_len = code_lengths(&lit_freq, 15);
    let dist_len = code_lengths(&dist_freq, 15);
    let lit_code = canonical(&lit_len);
    let dist_code = canonical(&dist_len);
    let hlit = lit_len
        .iter()
        .rposition(|&l| l > 0)
        .map_or(257, |p| (p + 1).max(257));
    let hdist = dist_len.iter().rposition(|&l| l > 0).map_or(1, |p| p + 1);
    let all: Vec<u8> = lit_len[..hlit]
        .iter()
        .chain(&dist_len[..hdist])
        .copied()
        .collect();
    let runs = rle(&all);
    let mut cl_freq = [0u32; 19];
    for &(symbol, _) in &runs {
        cl_freq[usize::from(symbol)] += 1;
    }
    let cl_len = code_lengths(&cl_freq, 7);
    let cl_code = canonical(&cl_len);
    let hclen = CL_ORDER
        .iter()
        .rposition(|&s| cl_len[s] > 0)
        .map_or(4, |p| (p + 1).max(4));

    let mut bits = Bits {
        out: Vec::new(),
        acc: 0,
        n: 0,
    };
    bits.put(1, 1); // final block
    bits.put(2, 2); // dynamic Huffman
    bits.put((hlit - 257) as u32, 5);
    bits.put((hdist - 1) as u32, 5);
    bits.put((hclen - 4) as u32, 4);
    for &symbol in &CL_ORDER[..hclen] {
        bits.put(u32::from(cl_len[symbol]), 3);
    }
    for &(symbol, extra) in &runs {
        let s = usize::from(symbol);
        bits.code(cl_code[s], u32::from(cl_len[s]));
        match symbol {
            16 => bits.put(extra, 2),
            17 => bits.put(extra, 3),
            18 => bits.put(extra, 7),
            _ => {}
        }
    }
    for token in &tokens {
        match *token {
            Token::Lit(b) => {
                let s = usize::from(b);
                bits.code(lit_code[s], u32::from(lit_len[s]));
            }
            Token::Match(len, dist) => {
                let l = bucket(&LEN_BASE, len);
                bits.code(lit_code[257 + l], u32::from(lit_len[257 + l]));
                bits.put((len - LEN_BASE[l]) as u32, LEN_EXTRA[l]);
                let d = bucket(&DIST_BASE, dist);
                bits.code(dist_code[d], u32::from(dist_len[d]));
                bits.put((dist - DIST_BASE[d]) as u32, DIST_EXTRA[d]);
            }
        }
    }
    bits.code(lit_code[256], u32::from(lit_len[256]));
    if bits.n > 0 {
        bits.put(0, 8 - bits.n);
    }
    bits.out
}
