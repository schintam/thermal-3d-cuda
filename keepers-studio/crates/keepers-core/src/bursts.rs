//! Burst grouping and near-duplicate detection.

use crate::scoring::{hamming, hamming_fp, quality, Scores};
use rayon::prelude::*;

#[derive(Debug, Clone)]
pub struct Shot {
    pub id: i64,
    pub folder_id: i64,
    pub capture_time: Option<i64>,
    pub scores: Option<Scores>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Grouping {
    /// Id of the burst's first frame, if the shot belongs to a burst of 2+.
    pub burst_id: Option<i64>,
    /// Suggested best frame of its burst.
    pub best: bool,
    /// An earlier, near-identical shot.
    pub dup_of: Option<i64>,
}

/// Frames closer together than this belong to the same burst...
pub const BURST_GAP_MS: i64 = 2000;
/// ...provided they also look alike (dHash bits differing).
const BURST_MAX_DIST: u32 = 24;
/// Near-duplicate thresholds: coarse pre-filter, then the 256-bit fingerprint
/// and a matching overall brightness.
const DUP_COARSE: u32 = 8;
const DUP_FINE: u32 = 16;
const DUP_EXPOSURE: f32 = 0.03;
const DUP_IDENTICAL: u32 = 2;

/// `shots` in any order; returns one grouping per shot, same order.
pub fn group(shots: &[Shot]) -> Vec<Grouping> {
    let mut out = vec![Grouping::default(); shots.len()];

    let mut order: Vec<usize> = (0..shots.len()).filter(|&i| shots[i].capture_time.is_some()).collect();
    order.sort_by_key(|&i| (shots[i].folder_id, shots[i].capture_time, shots[i].id));

    let mut start = 0;
    while start < order.len() {
        let mut end = start + 1;
        while end < order.len() {
            let (a, b) = (&shots[order[end - 1]], &shots[order[end]]);
            let close = a.folder_id == b.folder_id && b.capture_time.unwrap() - a.capture_time.unwrap() <= BURST_GAP_MS;
            let alike = match (&a.scores, &b.scores) {
                (Some(x), Some(y)) => hamming(x.dhash, y.dhash) <= BURST_MAX_DIST,
                // Unreadable files never join a burst.
                _ => false,
            };
            if !(close && alike) {
                break;
            }
            end += 1;
        }
        if end - start >= 2 {
            let members = &order[start..end];
            let burst_id = shots[members[0]].id;
            let best = members
                .iter()
                .copied()
                .max_by(|&x, &y| {
                    let q = |i: usize| shots[i].scores.as_ref().map(quality).unwrap_or(f32::MIN);
                    // On ties prefer the earlier frame.
                    q(x).partial_cmp(&q(y)).unwrap().then(shots[y].id.cmp(&shots[x].id))
                })
                .unwrap();
            for &i in members {
                out[i].burst_id = Some(burst_id);
                out[i].best = i == best;
            }
        }
        start = end;
    }

    // Near-duplicates: compare every pair of hashes (64-bit popcounts are cheap
    // enough even at 50k photos when spread over all cores).
    let hashed: Vec<(usize, i64, Scores)> =
        shots.iter().enumerate().filter_map(|(i, s)| s.scores.map(|sc| (i, s.id, sc))).collect();
    let dups: Vec<(usize, i64, u32)> = hashed
        .par_iter()
        .filter_map(|&(i, id, a)| {
            hashed
                .iter()
                .filter(|&&(_, oid, b)| {
                    oid < id
                        && hamming(a.dhash, b.dhash) <= DUP_COARSE
                        && (a.exposure - b.exposure).abs() <= DUP_EXPOSURE
                })
                .map(|&(_, oid, b)| (hamming_fp(&a.fp, &b.fp), oid))
                .filter(|&(d, _)| d <= DUP_FINE)
                .min()
                .map(|(d, o)| (i, o, d))
        })
        .collect();
    let burst_of: std::collections::HashMap<i64, Option<i64>> =
        shots.iter().zip(&out).map(|(s, g)| (s.id, g.burst_id)).collect();
    for (i, o, d) in dups {
        // Frames of one burst already look alike; within a burst only flag
        // frames that are essentially identical (e.g. a copied file).
        if out[i].burst_id.is_none() || out[i].burst_id != burst_of[&o] || d <= DUP_IDENTICAL {
            out[i].dup_of = Some(o);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(id: i64, t: Option<i64>, sharp: f32, hash: u64) -> Shot {
        Shot {
            id,
            folder_id: 1,
            capture_time: t,
            scores: Some(Scores { sharpness: sharp, exposure: 0.45, clip_hi: 0.0, clip_lo: 0.0, dhash: hash, fp: [hash; 8] }),
        }
    }

    #[test]
    fn groups_close_frames_and_picks_sharpest() {
        let shots = vec![
            shot(1, Some(0), 40.0, 0xF0F0),
            shot(2, Some(300), 70.0, 0xF0F1),
            shot(3, Some(600), 50.0, 0xF0F3),
            shot(4, Some(10_000), 90.0, 0x1234_5678_9abc_def0),
            shot(5, None, 10.0, 0xF0F0),
        ];
        let g = group(&shots);
        assert_eq!(g[0].burst_id, Some(1));
        assert_eq!(g[1].burst_id, Some(1));
        assert_eq!(g[2].burst_id, Some(1));
        assert!(g[1].best && !g[0].best && !g[2].best);
        assert_eq!(g[3].burst_id, None);
        assert_eq!(g[4].burst_id, None);
        // 5 is a pixel-identical copy of 1.
        assert_eq!(g[4].dup_of, Some(1));
        assert_eq!(g[0].dup_of, None);
    }

    #[test]
    fn different_content_breaks_burst() {
        let shots = vec![shot(1, Some(0), 40.0, 0), shot(2, Some(100), 40.0, u64::MAX)];
        let g = group(&shots);
        assert_eq!(g[0].burst_id, None);
        assert_eq!(g[1].burst_id, None);
    }
}
