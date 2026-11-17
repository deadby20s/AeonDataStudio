//! Deduplication: fingerprints per document, and finding which documents repeat earlier ones.
//!
//! Each document reaching a dedup step gets one hash per *band* (exact dedup: one band holding
//! a hash of the text; near dedup: MinHash LSH bands). Two documents are duplicates when they
//! share a hash in any band; the one with the lower input row is kept.
//!
//! During a run, `(band hash, row)` pairs are appended to one file per band. Resolving sorts
//! one band at a time (16 bytes per document in memory), so memory stays bounded while the
//! index itself lives on disk.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use aeon_shared::{ExactDedup, NearDedup};
use xxhash_rust::xxh3::{xxh3_64, xxh3_64_with_seed};

use crate::error::{AppError, AppResult};

/// 2^61 − 1, the modulus for MinHash's universal hash functions.
const MERSENNE_61: u64 = (1 << 61) - 1;
/// Fixed, so a recipe always finds the same duplicates.
const MINHASH_SEED: u64 = 0x5EED_AE0D_A7A5_EED1;

pub const EXACT_RULE: &str = "exact_duplicate";
pub const NEAR_RULE: &str = "near_duplicate";

/// Duplicate row → the earliest row it duplicates.
pub type Duplicates = HashMap<u64, u64>;

pub enum DedupMethod {
    Exact { normalize: bool },
    Near(MinHasher),
}

impl DedupMethod {
    pub fn exact(c: &ExactDedup) -> Self {
        DedupMethod::Exact { normalize: c.normalize }
    }

    pub fn near(c: &NearDedup) -> AppResult<Self> {
        if !(1..=20).contains(&c.ngram_size) {
            return Err(AppError::InvalidInput("Words per n-gram must be between 1 and 20".into()));
        }
        if !(1..=64).contains(&c.bands) {
            return Err(AppError::InvalidInput("Bands must be between 1 and 64".into()));
        }
        if !(1..=32).contains(&c.rows_per_band) {
            return Err(AppError::InvalidInput("Rows per band must be between 1 and 32".into()));
        }
        Ok(DedupMethod::Near(MinHasher::new(c.ngram_size as usize, c.bands as usize, c.rows_per_band as usize)))
    }

    pub fn bands(&self) -> usize {
        match self {
            DedupMethod::Exact { .. } => 1,
            DedupMethod::Near(m) => m.bands,
        }
    }

    pub fn rule(&self) -> &'static str {
        match self {
            DedupMethod::Exact { .. } => EXACT_RULE,
            DedupMethod::Near(_) => NEAR_RULE,
        }
    }

    /// One hash per band, or `None` for documents with no words (never treated as duplicates).
    pub fn fingerprint(&self, text: &str) -> Option<Vec<u64>> {
        match self {
            DedupMethod::Exact { normalize } => {
                if text.trim().is_empty() {
                    return None;
                }
                Some(vec![if *normalize {
                    let lower = text.to_lowercase();
                    xxh3_64(lower.split_whitespace().collect::<Vec<_>>().join(" ").as_bytes())
                } else {
                    xxh3_64(text.as_bytes())
                }])
            }
            DedupMethod::Near(m) => m.band_hashes(text),
        }
    }
}

/// Splitmix64, to derive the permutation coefficients from one seed.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub struct MinHasher {
    ngram: usize,
    bands: usize,
    rows: usize,
    /// Coefficients of `h_i(x) = (a_i·x + b_i) mod p`.
    a: Vec<u64>,
    b: Vec<u64>,
}

impl MinHasher {
    pub fn new(ngram: usize, bands: usize, rows: usize) -> Self {
        let mut state = MINHASH_SEED;
        let n = bands * rows;
        let a = (0..n).map(|_| splitmix64(&mut state) % (MERSENNE_61 - 1) + 1).collect();
        let b = (0..n).map(|_| splitmix64(&mut state) % MERSENNE_61).collect();
        Self { ngram, bands, rows, a, b }
    }

    /// Lowercased words with punctuation removed, as hashes.
    fn word_hashes(text: &str) -> Vec<u64> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| xxh3_64(w.to_lowercase().as_bytes()))
            .collect()
    }

    /// The MinHash signature: for each hash function, the minimum over the document's n-grams.
    pub fn signature(&self, text: &str) -> Option<Vec<u64>> {
        let words = Self::word_hashes(text);
        if words.is_empty() {
            return None;
        }
        let n = self.ngram.min(words.len());
        let mut mins = vec![u64::MAX; self.a.len()];
        let mut buf = Vec::with_capacity(n * 8);
        for gram in words.windows(n) {
            buf.clear();
            gram.iter().for_each(|w| buf.extend_from_slice(&w.to_le_bytes()));
            let x = xxh3_64(&buf) % MERSENNE_61;
            for (i, m) in mins.iter_mut().enumerate() {
                let h = ((self.a[i] as u128 * x as u128 + self.b[i] as u128) % MERSENNE_61 as u128) as u64;
                if h < *m {
                    *m = h;
                }
            }
        }
        Some(mins)
    }

    pub fn band_hashes(&self, text: &str) -> Option<Vec<u64>> {
        let sig = self.signature(text)?;
        Some(
            sig.chunks(self.rows)
                .enumerate()
                .map(|(band, rows)| {
                    let bytes: Vec<u8> = rows.iter().flat_map(|v| v.to_le_bytes()).collect();
                    xxh3_64_with_seed(&bytes, band as u64)
                })
                .collect(),
        )
    }
}

/// Groups `(hash, row)` pairs of one band: every row sharing a hash with a lower row is a
/// duplicate of the lowest row of its group.
fn resolve_band(entries: &mut [(u64, u64)], dups: &mut Duplicates) {
    entries.sort_unstable();
    let mut i = 0;
    while i < entries.len() {
        let (hash, first) = entries[i];
        let mut j = i + 1;
        while j < entries.len() && entries[j].0 == hash {
            let row = entries[j].1;
            dups.entry(row).and_modify(|r| *r = (*r).min(first)).or_insert(first);
            j += 1;
        }
        i = j;
    }
}

/// In memory, for previews on a sample. `fingerprints` are `(row, band hashes)`.
pub fn resolve_in_memory(bands: usize, fingerprints: &[(u64, Vec<u64>)]) -> Duplicates {
    let mut dups = Duplicates::new();
    for band in 0..bands {
        let mut entries: Vec<(u64, u64)> = fingerprints.iter().map(|(row, h)| (h[band], *row)).collect();
        resolve_band(&mut entries, &mut dups);
    }
    dups
}

/// On-disk band files for a full run.
pub struct BandFiles {
    dir: PathBuf,
    writers: Vec<BufWriter<File>>,
}

impl BandFiles {
    pub fn create(dir: &Path, bands: usize) -> AppResult<Self> {
        std::fs::create_dir_all(dir)?;
        let writers = (0..bands)
            .map(|b| Ok(BufWriter::with_capacity(1 << 20, File::create(dir.join(format!("band-{b:02}.bin")))?)))
            .collect::<AppResult<Vec<_>>>()?;
        Ok(Self { dir: dir.to_path_buf(), writers })
    }

    pub fn push(&mut self, row: u64, hashes: &[u64]) -> AppResult<()> {
        for (w, h) in self.writers.iter_mut().zip(hashes) {
            w.write_all(&h.to_le_bytes())?;
            w.write_all(&row.to_le_bytes())?;
        }
        Ok(())
    }

    /// Finds the duplicates and deletes the band files.
    pub fn resolve(self) -> AppResult<Duplicates> {
        let bands = self.writers.len();
        for mut w in self.writers {
            w.flush()?;
        }
        let mut dups = Duplicates::new();
        for b in 0..bands {
            let path = self.dir.join(format!("band-{b:02}.bin"));
            let mut bytes = Vec::new();
            BufReader::new(File::open(&path)?).read_to_end(&mut bytes)?;
            let mut entries: Vec<(u64, u64)> = bytes
                .as_chunks::<16>().0.iter()
                .map(|c| {
                    let hash = u64::from_le_bytes(c[..8].try_into().expect("8 bytes"));
                    let row = u64::from_le_bytes(c[8..].try_into().expect("8 bytes"));
                    (hash, row)
                })
                .collect();
            drop(bytes);
            resolve_band(&mut entries, &mut dups);
            std::fs::remove_file(&path)?;
        }
        let _ = std::fs::remove_dir(&self.dir);
        Ok(dups)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jaccard_estimate(m: &MinHasher, a: &str, b: &str) -> f64 {
        let (sa, sb) = (m.signature(a).unwrap(), m.signature(b).unwrap());
        sa.iter().zip(&sb).filter(|(x, y)| x == y).count() as f64 / sa.len() as f64
    }

    #[test]
    fn minhash_estimates_similarity() {
        let m = MinHasher::new(3, 32, 4);
        let base: Vec<String> = (0..200).map(|i| format!("w{i}")).collect();
        let a = base.join(" ");
        // Change 10 of 200 words: most 3-grams survive.
        let mut close = base.clone();
        for i in (0..200).step_by(20) {
            close[i] = format!("x{i}");
        }
        let far: Vec<String> = (0..200).map(|i| format!("z{i}")).collect();
        assert!(jaccard_estimate(&m, &a, &close.join(" ")) > 0.6);
        assert!(jaccard_estimate(&m, &a, &far.join(" ")) < 0.05);
        // Case and punctuation are ignored.
        assert_eq!(m.signature("Hello, World! again").unwrap(), m.signature("hello world again").unwrap());
    }

    #[test]
    fn exact_normalization() {
        let e = DedupMethod::Exact { normalize: true };
        assert_eq!(e.fingerprint("Hello   World\n"), e.fingerprint("hello world"));
        let raw = DedupMethod::Exact { normalize: false };
        assert_ne!(raw.fingerprint("Hello World"), raw.fingerprint("hello world"));
        assert_eq!(e.fingerprint("   "), None);
    }

    #[test]
    fn resolution_keeps_the_lowest_row() {
        // Rows 1, 5, 9 share band 0; row 7 matches row 5 in band 1 only.
        let fp = vec![
            (1, vec![10, 100]),
            (3, vec![30, 300]),
            (5, vec![10, 500]),
            (7, vec![70, 500]),
            (9, vec![10, 900]),
        ];
        let dups = resolve_in_memory(2, &fp);
        assert_eq!(dups.get(&5), Some(&1));
        assert_eq!(dups.get(&9), Some(&1));
        assert_eq!(dups.get(&7), Some(&5));
        assert!(!dups.contains_key(&1) && !dups.contains_key(&3));
    }

    #[test]
    fn on_disk_matches_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let fp: Vec<(u64, Vec<u64>)> = (0..5000u64).map(|r| (r, vec![r % 997, r % 1009, r / 3])).collect();
        let mut files = BandFiles::create(&dir.path().join("d"), 3).unwrap();
        for (row, h) in &fp {
            files.push(*row, h).unwrap();
        }
        assert_eq!(files.resolve().unwrap(), resolve_in_memory(3, &fp));
        assert!(!dir.path().join("d").exists(), "band files are cleaned up");
    }

    #[test]
    fn invalid_settings() {
        assert!(DedupMethod::near(&NearDedup { ngram_size: 0, ..Default::default() }).is_err());
        assert!(DedupMethod::near(&NearDedup { bands: 100, ..Default::default() }).is_err());
    }
}
