//! SHA-256 (FIPS 180-4), ~90 lines of safe Rust.
//!
//! Why not a crate: this hash exists for exactly one job — pinning the bundled
//! ViGEmBus installer's identity before `ksx install-drivers` is allowed to
//! execute it. A supply-chain check that pulls a new supply chain to do the
//! checking is not much of a check, and `sha2` + `digest` + `generic-array` +
//! `typenum` is a lot of dependency for one digest. It is also the only way the
//! verification path stays testable on a non-Windows CI runner.
//!
//! Correctness is pinned by the NIST vectors in the tests below plus a
//! multi-block/streaming case; the algorithm has been frozen since 2001.

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

/// Streaming SHA-256 state. Feed with [`update`](Sha256::update), finish with
/// [`finish`](Sha256::finish).
#[derive(Clone)]
pub struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    buffered: usize,
    len_bits: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0u8; 64],
            buffered: 0,
            len_bits: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len_bits = self.len_bits.wrapping_add((data.len() as u64) * 8);
        if self.buffered > 0 {
            let take = (64 - self.buffered).min(data.len());
            self.buf[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buffered = 0;
            } else {
                // `take` was limited by `data.len()`, so reaching here means the
                // input is exhausted and a PARTIAL block is still buffered.
                // Falling through would run the tail below, whose
                // `self.buffered = rest.len()` on an empty remainder resets the
                // count to 0 — silently discarding the buffered bytes and, in
                // `finish`, oscillating `buffered` between 0 and 1 forever.
                return;
            }
        }
        let mut chunks = data.chunks_exact(64);
        for block in &mut chunks {
            let mut b = [0u8; 64];
            b.copy_from_slice(block);
            self.compress(&b);
        }
        let rest = chunks.remainder();
        self.buf[..rest.len()].copy_from_slice(rest);
        self.buffered = rest.len();
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.len_bits;
        self.update_no_len(&[0x80]);
        while self.buffered != 56 {
            self.update_no_len(&[0x00]);
        }
        self.update_no_len(&bits.to_be_bytes());
        debug_assert_eq!(self.buffered, 0);

        let mut out = [0u8; 32];
        for (chunk, word) in out.chunks_exact_mut(4).zip(self.h.iter()) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// Padding bytes must not count toward the length field.
    fn update_no_len(&mut self, data: &[u8]) {
        let saved = self.len_bits;
        self.update(data);
        self.len_bits = saved;
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in self.h.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(v);
        }
    }
}

/// Uppercase hex, the form `docs/DRIVERS.md` records installer hashes in.
pub fn hex_upper(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02X}");
    }
    out
}

/// Hash a file in 64 KiB chunks — the ViGEmBus installer is ~1 MB today, but
/// streaming means a bad `--installer` path can never balloon memory.
pub fn hash_file(path: &std::path::Path) -> std::io::Result<[u8; 32]> {
    use std::io::Read as _;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finish())
}

/// Constant-time-ish comparison of a computed digest against a recorded hex
/// string. Case-insensitive; whitespace-tolerant.
pub fn digest_matches(digest: &[u8; 32], expected_hex: &str) -> bool {
    let want = expected_hex.trim();
    if want.len() != 64 {
        return false;
    }
    hex_upper(digest).eq_ignore_ascii_case(want)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex_upper(&h.finish())
    }

    /// FIPS 180-4 / NIST CAVP vectors.
    #[test]
    fn nist_vectors() {
        assert_eq!(
            hash(b""),
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        );
        assert_eq!(
            hash(b"abc"),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
        assert_eq!(
            hash(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248D6A61D20638B8E5C026930C3E6039A33CE45964FF2167F6ECEDD419DB06C1"
        );
        // 1,000,000 'a' — exercises many blocks and the 64-bit length field.
        let mut h = Sha256::new();
        for _ in 0..1000 {
            h.update(&[b'a'; 1000]);
        }
        assert_eq!(
            hex_upper(&h.finish()),
            "CDC76E5C9914FB9281A1C7E284D73E67F1809A48A497200E046D39CCC7112CD0"
        );
    }

    /// Streaming in odd-sized pieces must equal hashing in one shot: the
    /// installer is read in 64 KiB chunks, so a buffering bug here would let a
    /// tampered file pass with a "matching" hash.
    #[test]
    fn chunked_updates_match_one_shot() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let want = hash(&data);
        for step in [1usize, 7, 63, 64, 65, 127, 128, 1000] {
            let mut h = Sha256::new();
            for chunk in data.chunks(step) {
                h.update(chunk);
            }
            assert_eq!(hex_upper(&h.finish()), want, "step={step}");
        }
    }

    #[test]
    fn digest_matches_is_case_insensitive_and_length_checked() {
        let mut h = Sha256::new();
        h.update(b"abc");
        let d = h.finish();
        assert!(digest_matches(
            &d,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        ));
        assert!(digest_matches(
            &d,
            "  BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD  "
        ));
        assert!(!digest_matches(&d, "BA7816BF"), "a prefix must never match");
        assert!(!digest_matches(&d, ""));
    }

    #[test]
    fn hash_file_matches_the_in_memory_digest() {
        let dir = std::env::temp_dir().join(format!("ksx-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("blob.bin");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 256) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        assert_eq!(hex_upper(&hash_file(&path).unwrap()), hash(&data));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
