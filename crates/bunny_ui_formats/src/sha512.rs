//! SHA-512 (FIPS 180-4): the hash inside Ed25519.
//!
//! It uses 128-byte blocks and 64-bit words, so it does not fit the
//! 64-byte block engine of [`crate::sha`]. It has its own engine here.

/// The round constants: the first 64 bits of the fractional parts of
/// the cube roots of the first 80 primes.
const K: [u64; 80] = [
    0x428a_2f98_d728_ae22, 0x7137_4491_23ef_65cd, 0xb5c0_fbcf_ec4d_3b2f, 0xe9b5_dba5_8189_dbbc,
    0x3956_c25b_f348_b538, 0x59f1_11f1_b605_d019, 0x923f_82a4_af19_4f9b, 0xab1c_5ed5_da6d_8118,
    0xd807_aa98_a303_0242, 0x1283_5b01_4570_6fbe, 0x2431_85be_4ee4_b28c, 0x550c_7dc3_d5ff_b4e2,
    0x72be_5d74_f27b_896f, 0x80de_b1fe_3b16_96b1, 0x9bdc_06a7_25c7_1235, 0xc19b_f174_cf69_2694,
    0xe49b_69c1_9ef1_4ad2, 0xefbe_4786_384f_25e3, 0x0fc1_9dc6_8b8c_d5b5, 0x240c_a1cc_77ac_9c65,
    0x2de9_2c6f_592b_0275, 0x4a74_84aa_6ea6_e483, 0x5cb0_a9dc_bd41_fbd4, 0x76f9_88da_8311_53b5,
    0x983e_5152_ee66_dfab, 0xa831_c66d_2db4_3210, 0xb003_27c8_98fb_213f, 0xbf59_7fc7_beef_0ee4,
    0xc6e0_0bf3_3da8_8fc2, 0xd5a7_9147_930a_a725, 0x06ca_6351_e003_826f, 0x1429_2967_0a0e_6e70,
    0x27b7_0a85_46d2_2ffc, 0x2e1b_2138_5c26_c926, 0x4d2c_6dfc_5ac4_2aed, 0x5338_0d13_9d95_b3df,
    0x650a_7354_8baf_63de, 0x766a_0abb_3c77_b2a8, 0x81c2_c92e_47ed_aee6, 0x9272_2c85_1482_353b,
    0xa2bf_e8a1_4cf1_0364, 0xa81a_664b_bc42_3001, 0xc24b_8b70_d0f8_9791, 0xc76c_51a3_0654_be30,
    0xd192_e819_d6ef_5218, 0xd699_0624_5565_a910, 0xf40e_3585_5771_202a, 0x106a_a070_32bb_d1b8,
    0x19a4_c116_b8d2_d0c8, 0x1e37_6c08_5141_ab53, 0x2748_774c_df8e_eb99, 0x34b0_bcb5_e19b_48a8,
    0x391c_0cb3_c5c9_5a63, 0x4ed8_aa4a_e341_8acb, 0x5b9c_ca4f_7763_e373, 0x682e_6ff3_d6b2_b8a3,
    0x748f_82ee_5def_b2fc, 0x78a5_636f_4317_2f60, 0x84c8_7814_a1f0_ab72, 0x8cc7_0208_1a64_39ec,
    0x90be_fffa_2363_1e28, 0xa450_6ceb_de82_bde9, 0xbef9_a3f7_b2c6_7915, 0xc671_78f2_e372_532b,
    0xca27_3ece_ea26_619c, 0xd186_b8c7_21c0_c207, 0xeada_7dd6_cde0_eb1e, 0xf57d_4f7f_ee6e_d178,
    0x06f0_67aa_7217_6fba, 0x0a63_7dc5_a2c8_98a6, 0x113f_9804_bef9_0dae, 0x1b71_0b35_131c_471b,
    0x28db_77f5_2304_7d84, 0x32ca_ab7b_40c7_2493, 0x3c9e_be0a_15c9_bebc, 0x431d_67c4_9c10_0d4c,
    0x4cc5_d4be_cb3e_42b6, 0x597f_299c_fc65_7e2a, 0x5fcb_6fab_3ad6_faec, 0x6c44_198c_4a47_5817,
];

/// The first state: the first 64 bits of the fractional parts of the
/// square roots of the first eight primes.
const START: [u64; 8] = [
    0x6a09_e667_f3bc_c908, 0xbb67_ae85_84ca_a73b, 0x3c6e_f372_fe94_f82b, 0xa54f_f53a_5f1d_36f1,
    0x510e_527f_ade6_82d1, 0x9b05_688c_2b3e_6c1f, 0x1f83_d9ab_fb41_bd6b, 0x5be0_cd19_137e_2179,
];

/// A SHA-512 hash that takes its input in pieces.
#[derive(Clone)]
pub struct Sha512 {
    state: [u64; 8],
    buffer: [u8; 128],
    filled: usize,
    length: u128,
}

impl Default for Sha512 {
    fn default() -> Self {
        Self { state: START, buffer: [0; 128], filled: 0, length: 0 }
    }
}

impl Sha512 {
    /// A new hash, with no input.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `data` to the input.
    pub fn update(&mut self, mut data: &[u8]) {
        self.length += data.len() as u128;
        if self.filled > 0 {
            let take = (128 - self.filled).min(data.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled < 128 {
                return;
            }
            let block = self.buffer;
            compress(&mut self.state, &block);
            self.filled = 0;
        }
        let (blocks, rest) = data.as_chunks::<128>();
        for block in blocks {
            compress(&mut self.state, block);
        }
        self.buffer[..rest.len()].copy_from_slice(rest);
        self.filled = rest.len();
    }

    /// Adds `data` and gives the hash back, for a chain of calls.
    #[must_use]
    pub fn chain(mut self, data: &[u8]) -> Self {
        self.update(data);
        self
    }

    /// The 64-byte digest of all the input.
    #[must_use]
    pub fn finish(mut self) -> [u8; 64] {
        let bits = self.length.wrapping_mul(8);
        let mut tail = [0u8; 256];
        tail[..self.filled].copy_from_slice(&self.buffer[..self.filled]);
        tail[self.filled] = 0x80;
        // the length takes the last 16 bytes of the last block
        let end = if self.filled < 112 { 128 } else { 256 };
        tail[end - 16..end].copy_from_slice(&bits.to_be_bytes());
        for block in tail[..end].as_chunks::<128>().0 {
            compress(&mut self.state, block);
        }
        let mut out = [0u8; 64];
        for (place, word) in out.as_chunks_mut::<8>().0.iter_mut().zip(self.state) {
            place.copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

/// The SHA-512 digest of `data`.
#[must_use]
pub fn sha512(data: &[u8]) -> [u8; 64] {
    Sha512::new().chain(data).finish()
}

fn compress(state: &mut [u64; 8], block: &[u8; 128]) {
    let mut w = [0u64; 80];
    for (word, bytes) in w.iter_mut().zip(block.as_chunks::<8>().0) {
        *word = u64::from_be_bytes(*bytes);
    }
    for t in 16..80 {
        let s0 = w[t - 15].rotate_right(1) ^ w[t - 15].rotate_right(8) ^ (w[t - 15] >> 7);
        let s1 = w[t - 2].rotate_right(19) ^ w[t - 2].rotate_right(61) ^ (w[t - 2] >> 6);
        w[t] = w[t - 16].wrapping_add(s0).wrapping_add(w[t - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for t in 0..80 {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let choose = (e & f) ^ (!e & g);
        let t1 = h.wrapping_add(s1).wrapping_add(choose).wrapping_add(K[t]).wrapping_add(w[t]);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(majority);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (word, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *word = word.wrapping_add(add);
    }
}

#[cfg(test)]
#[path = "sha512_tests.rs"]
mod tests;
