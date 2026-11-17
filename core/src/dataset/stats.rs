use aeon_shared::{HistogramBin, TextStats};

/// Lower edges of the document-length histogram, in characters. The last bin is open-ended.
const LENGTH_BIN_EDGES: [u64; 13] = [0, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072];

#[derive(Debug, Clone)]
pub struct StatsBuilder {
    stats: TextStats,
    bins: [u64; LENGTH_BIN_EDGES.len()],
}

impl Default for StatsBuilder {
    fn default() -> Self {
        Self { stats: TextStats::default(), bins: [0; LENGTH_BIN_EDGES.len()] }
    }
}

impl StatsBuilder {
    pub fn add(&mut self, text: &str) {
        let chars = text.chars().count() as u64;
        let s = &mut self.stats;
        s.doc_count += 1;
        s.char_count += chars;
        s.text_bytes += text.len() as u64;
        s.min_chars = Some(s.min_chars.map_or(chars, |m| m.min(chars)));
        s.max_chars = Some(s.max_chars.map_or(chars, |m| m.max(chars)));
        let bin = LENGTH_BIN_EDGES.partition_point(|&edge| edge <= chars) - 1;
        self.bins[bin] += 1;
    }

    pub fn finish(mut self) -> TextStats {
        self.stats.length_histogram = LENGTH_BIN_EDGES
            .iter()
            .enumerate()
            .map(|(i, &lo)| HistogramBin { lo, hi: LENGTH_BIN_EDGES.get(i + 1).copied(), count: self.bins[i] })
            .collect();
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_chars_not_bytes_and_bins_by_length() {
        let mut b = StatsBuilder::default();
        b.add("héllo"); // 5 chars, 6 bytes
        b.add(&"x".repeat(64));
        b.add(&"x".repeat(200_000));
        let s = b.finish();
        assert_eq!(s.doc_count, 3);
        assert_eq!(s.char_count, 5 + 64 + 200_000);
        assert_eq!(s.text_bytes, 6 + 64 + 200_000);
        assert_eq!(s.min_chars, Some(5));
        assert_eq!(s.max_chars, Some(200_000));
        let h = &s.length_histogram;
        assert_eq!(h[0], HistogramBin { lo: 0, hi: Some(64), count: 1 });
        assert_eq!(h[1], HistogramBin { lo: 64, hi: Some(128), count: 1 });
        assert_eq!(h.last().unwrap(), &HistogramBin { lo: 131072, hi: None, count: 1 });
        assert_eq!(h.iter().map(|b| b.count).sum::<u64>(), 3);
    }
}
