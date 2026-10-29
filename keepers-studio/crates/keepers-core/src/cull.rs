//! Auto-cull: a first pass over a whole shoot, built only from the local
//! scores, bursts and duplicates. The shoot is split into moments (runs of
//! photos close in time and look); technical failures, weaker burst frames
//! and duplicates become reject suggestions, and the strongest frames of each
//! moment become pick suggestions. Everything else is left for the person.

use crate::scoring::{hamming, quality, Scores};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct CullPhoto {
    pub id: i64,
    pub folder_id: i64,
    pub capture_time: Option<i64>,
    pub scores: Option<Scores>,
    pub burst_id: Option<i64>,
    pub burst_best: bool,
    pub dup_of: Option<i64>,
    pub pick: i8,
    pub rating: u8,
    pub edited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Strictness {
    Gentle,
    #[default]
    Balanced,
    Strict,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CullOptions {
    pub strictness: Strictness,
    /// Keep at most this many picks, spread over the moments.
    pub max_picks: Option<usize>,
    /// Also revise photos that already have a pick or reject flag. Starred
    /// and edited photos are always left as they are.
    pub revise_decided: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Pick,
    Reject,
    Leave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// Best frame of its moment.
    TopOfMoment,
    /// Nearly as strong as the best of its moment.
    StrongFrame,
    /// A good frame, but the moment already has its picks.
    Alternate,
    /// Would be a pick, but the pick limit was reached.
    OverLimit,
    /// A weaker frame of a burst.
    BurstFrame,
    /// A near-identical copy of a better photo.
    Duplicate,
    OutOfFocus,
    Blown,
    TooDark,
    /// Already flagged, rated or edited by the person.
    Decided,
    /// Previews and scores aren't ready yet.
    Unscored,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub id: i64,
    pub verdict: Verdict,
    pub reason: Reason,
    pub moment: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CullPlan {
    pub suggestions: Vec<Suggestion>,
    pub moments: u32,
}

struct Limits {
    soft_abs: f32,
    soft_rel: f32,
    blown: f32,
    dark_mean: f32,
    dark_clip: f32,
    /// Quality points behind the moment's best that still count as a pick.
    margin: f32,
    /// Share of a moment's good frames that may become picks.
    share: f32,
    /// Burst frames this close to the chosen frame are left, not rejected.
    burst_spare: f32,
}

fn limits(s: Strictness) -> Limits {
    match s {
        Strictness::Gentle => {
            Limits { soft_abs: 10.0, soft_rel: 0.4, blown: 0.35, dark_mean: 0.03, dark_clip: 0.6, margin: 20.0, share: 0.75, burst_spare: 8.0 }
        }
        Strictness::Balanced => {
            Limits { soft_abs: 16.0, soft_rel: 0.55, blown: 0.22, dark_mean: 0.05, dark_clip: 0.45, margin: 10.0, share: 0.5, burst_spare: 0.0 }
        }
        Strictness::Strict => {
            Limits { soft_abs: 22.0, soft_rel: 0.7, blown: 0.12, dark_mean: 0.08, dark_clip: 0.3, margin: 5.0, share: 0.2, burst_spare: 0.0 }
        }
    }
}

/// A new moment starts after a pause this long...
const MOMENT_GAP_MS: i64 = 90_000;
/// ...or after a shorter pause when the view also changed.
const SCENE_GAP_MS: i64 = 15_000;
const SCENE_CHANGE: u32 = 26;

/// Split into moments; returns a moment index per photo (same order).
fn moments(photos: &[CullPhoto]) -> (Vec<u32>, u32) {
    let mut order: Vec<usize> = (0..photos.len()).collect();
    order.sort_by_key(|&i| (photos[i].folder_id, photos[i].capture_time.is_none(), photos[i].capture_time, photos[i].id));
    let mut out = vec![0u32; photos.len()];
    let mut m = 0u32;
    for (k, &i) in order.iter().enumerate() {
        if k > 0 {
            let (a, b) = (&photos[order[k - 1]], &photos[i]);
            let new = match (a.capture_time, b.capture_time) {
                _ if a.folder_id != b.folder_id => true,
                (Some(ta), Some(tb)) => {
                    let gap = tb - ta;
                    let changed = match (&a.scores, &b.scores) {
                        (Some(x), Some(y)) => hamming(x.dhash, y.dhash) > SCENE_CHANGE,
                        _ => false,
                    };
                    gap > MOMENT_GAP_MS || (gap > SCENE_GAP_MS && changed)
                }
                // Without capture times each photo stands alone.
                _ => true,
            };
            if new {
                m += 1;
            }
        }
        out[i] = m;
    }
    (out, if photos.is_empty() { 0 } else { m + 1 })
}

pub fn plan(photos: &[CullPhoto], opts: &CullOptions) -> CullPlan {
    let lim = limits(opts.strictness);
    let (moment, n_moments) = moments(photos);
    let q: Vec<f32> = photos.iter().map(|p| p.scores.as_ref().map(quality).unwrap_or(f32::MIN)).collect();
    let index: HashMap<i64, usize> = photos.iter().enumerate().map(|(i, p)| (p.id, i)).collect();

    // Focus is judged against the shoot itself, so a soft lens or a dim
    // evening doesn't reject everything.
    let mut sharp: Vec<f32> = photos.iter().filter_map(|p| p.scores.map(|s| s.sharpness)).collect();
    sharp.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sharp.get(sharp.len() / 2).copied().unwrap_or(0.0);
    let soft_cut = lim.soft_abs.min(median * lim.soft_rel);

    let mut verdict: Vec<Option<(Verdict, Reason)>> = vec![None; photos.len()];
    for (i, p) in photos.iter().enumerate() {
        let Some(s) = p.scores else {
            verdict[i] = Some((Verdict::Leave, Reason::Unscored));
            continue;
        };
        verdict[i] = if s.sharpness < soft_cut {
            Some((Verdict::Reject, Reason::OutOfFocus))
        } else if s.clip_hi > lim.blown {
            Some((Verdict::Reject, Reason::Blown))
        } else if s.exposure < lim.dark_mean && s.clip_lo > lim.dark_clip {
            Some((Verdict::Reject, Reason::TooDark))
        } else {
            None
        };
    }

    // Duplicates: within each cluster keep the chosen burst frame, else the
    // best one.
    let mut root: Vec<usize> = (0..photos.len()).collect();
    fn find(r: &mut [usize], mut i: usize) -> usize {
        while r[i] != i {
            r[i] = r[r[i]];
            i = r[i];
        }
        i
    }
    for (i, p) in photos.iter().enumerate() {
        if let Some(&o) = p.dup_of.and_then(|d| index.get(&d)) {
            let (a, b) = (find(&mut root, i), find(&mut root, o));
            root[a] = b;
        }
    }
    let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..photos.len() {
        let r = find(&mut root, i);
        clusters.entry(r).or_default().push(i);
    }
    let rank: Vec<(bool, bool, f32)> = (0..photos.len()).map(|i| (verdict[i].is_none(), photos[i].burst_best, q[i])).collect();
    for members in clusters.values().filter(|m| m.len() > 1) {
        // On a tie keep the original, not the copy.
        let keep = *members
            .iter()
            .max_by(|&&a, &&b| {
                rank[a]
                    .partial_cmp(&rank[b])
                    .unwrap()
                    .then(photos[b].dup_of.is_some().cmp(&photos[a].dup_of.is_some()))
                    .then(photos[b].id.cmp(&photos[a].id))
            })
            .unwrap();
        for &i in members {
            if i != keep && verdict[i].is_none() {
                verdict[i] = Some((Verdict::Reject, Reason::Duplicate));
            }
        }
    }

    // Bursts: the chosen frame stays in the running, the others go.
    let mut chosen: HashMap<i64, f32> = HashMap::new();
    for (i, p) in photos.iter().enumerate() {
        if let (Some(b), true) = (p.burst_id, p.burst_best) {
            chosen.insert(b, q[i]);
        }
    }
    for (i, p) in photos.iter().enumerate() {
        if let (Some(b), false, None) = (p.burst_id, p.burst_best, verdict[i]) {
            let best = chosen.get(&b).copied().unwrap_or(f32::MAX);
            verdict[i] =
                Some(if q[i] >= best - lim.burst_spare { (Verdict::Leave, Reason::Alternate) } else { (Verdict::Reject, Reason::BurstFrame) });
        }
    }

    // Moments: the strongest remaining frames become picks.
    let mut by_moment: HashMap<u32, Vec<usize>> = HashMap::new();
    for i in (0..photos.len()).filter(|&i| verdict[i].is_none()) {
        by_moment.entry(moment[i]).or_default().push(i);
    }
    let mut picks: Vec<(usize, usize)> = Vec::new(); // (rank in moment, photo)
    for list in by_moment.values_mut() {
        list.sort_by(|&a, &b| q[b].partial_cmp(&q[a]).unwrap().then(photos[a].id.cmp(&photos[b].id)));
        let best = q[list[0]];
        let room = ((list.len() as f32 * lim.share).ceil() as usize).max(1);
        for (r, &i) in list.iter().enumerate() {
            if r == 0 {
                verdict[i] = Some((Verdict::Pick, Reason::TopOfMoment));
                picks.push((r, i));
            } else if r < room && q[i] >= best - lim.margin {
                verdict[i] = Some((Verdict::Pick, Reason::StrongFrame));
                picks.push((r, i));
            } else {
                verdict[i] = Some((Verdict::Leave, Reason::Alternate));
            }
        }
    }

    // A pick limit is filled round-robin: every moment's best before any
    // moment's second.
    if let Some(max) = opts.max_picks {
        picks.sort_by(|a, b| a.0.cmp(&b.0).then(q[b.1].partial_cmp(&q[a.1]).unwrap()));
        for &(_, i) in picks.iter().skip(max) {
            verdict[i] = Some((Verdict::Leave, Reason::OverLimit));
        }
    }

    let suggestions = photos
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (mut v, mut reason) = verdict[i].unwrap_or((Verdict::Leave, Reason::Alternate));
            if p.rating > 0 || p.edited || (!opts.revise_decided && p.pick != 0) {
                (v, reason) = (Verdict::Leave, Reason::Decided);
            }
            Suggestion { id: p.id, verdict: v, reason, moment: moment[i] }
        })
        .collect();
    CullPlan { suggestions, moments: n_moments }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: i64, t: i64, sharp: f32, hash: u64) -> CullPhoto {
        CullPhoto {
            id,
            folder_id: 1,
            capture_time: Some(t),
            scores: Some(Scores { sharpness: sharp, exposure: 0.45, dhash: hash, ..Default::default() }),
            burst_id: None,
            burst_best: false,
            dup_of: None,
            pick: 0,
            rating: 0,
            edited: false,
        }
    }

    fn get(plan: &CullPlan, id: i64) -> (Verdict, Reason) {
        let s = plan.suggestions.iter().find(|s| s.id == id).unwrap();
        (s.verdict, s.reason)
    }

    fn shoot() -> Vec<CullPhoto> {
        let mut v = vec![
            // Moment 1: a burst of three, frame 2 chosen.
            photo(1, 0, 50.0, 0),
            photo(2, 400, 70.0, 0),
            photo(3, 800, 45.0, 0),
            // Moment 1 continued: a soft frame.
            photo(4, 5_000, 6.0, 0),
            // Moment 2, five minutes later: two good frames and a copy.
            photo(5, 300_000, 60.0, u64::MAX),
            photo(6, 310_000, 58.0, u64::MAX),
            photo(7, 320_000, 55.0, u64::MAX),
        ];
        for p in &mut v[..3] {
            p.burst_id = Some(1);
        }
        v[1].burst_best = true;
        v[6].dup_of = Some(5);
        v
    }

    #[test]
    fn an_exact_copy_loses_to_the_original() {
        let mut s = vec![photo(1, 0, 50.0, 7), photo(2, 0, 50.0, 7)];
        s[1].dup_of = Some(1);
        let p = plan(&s, &CullOptions::default());
        assert_eq!(get(&p, 1).0, Verdict::Pick);
        assert_eq!(get(&p, 2), (Verdict::Reject, Reason::Duplicate));
    }

    #[test]
    fn suggests_picks_and_rejects() {
        let plan = plan(&shoot(), &CullOptions::default());
        assert_eq!(plan.moments, 2);
        assert_eq!(get(&plan, 2), (Verdict::Pick, Reason::TopOfMoment));
        assert_eq!(get(&plan, 1), (Verdict::Reject, Reason::BurstFrame));
        assert_eq!(get(&plan, 3), (Verdict::Reject, Reason::BurstFrame));
        assert_eq!(get(&plan, 4), (Verdict::Reject, Reason::OutOfFocus));
        assert_eq!(get(&plan, 5), (Verdict::Pick, Reason::TopOfMoment));
        assert_eq!(get(&plan, 7), (Verdict::Reject, Reason::Duplicate));
        // Two good frames in one moment: Balanced picks one, Gentle both.
        assert_eq!(get(&plan, 6), (Verdict::Leave, Reason::Alternate));
    }

    #[test]
    fn gentle_spares_close_burst_frames_and_strict_picks_fewer() {
        let gentle = plan(&shoot(), &CullOptions { strictness: Strictness::Gentle, ..Default::default() });
        assert_eq!(get(&gentle, 3).0, Verdict::Reject);
        let mut s = shoot();
        s[0].scores.as_mut().unwrap().sharpness = 68.0;
        let gentle = plan(&s, &CullOptions { strictness: Strictness::Gentle, ..Default::default() });
        assert_eq!(get(&gentle, 1), (Verdict::Leave, Reason::Alternate));
        assert_eq!(get(&gentle, 6), (Verdict::Pick, Reason::StrongFrame));
        let strict = plan(&shoot(), &CullOptions { strictness: Strictness::Strict, ..Default::default() });
        assert_eq!(get(&strict, 6), (Verdict::Leave, Reason::Alternate));
    }

    #[test]
    fn pick_limit_covers_every_moment_first() {
        let plan = plan(&shoot(), &CullOptions { max_picks: Some(2), strictness: Strictness::Gentle, ..Default::default() });
        let picks: Vec<i64> = plan.suggestions.iter().filter(|s| s.verdict == Verdict::Pick).map(|s| s.id).collect();
        assert_eq!(picks, vec![2, 5]);
        assert_eq!(get(&plan, 6), (Verdict::Leave, Reason::OverLimit));
    }

    #[test]
    fn decided_photos_are_left_alone() {
        let mut s = shoot();
        s[0].pick = 1;
        s[3].rating = 3;
        let p = plan(&s, &CullOptions::default());
        assert_eq!(get(&p, 1), (Verdict::Leave, Reason::Decided));
        assert_eq!(get(&p, 4), (Verdict::Leave, Reason::Decided));
        let p = plan(&s, &CullOptions { revise_decided: true, ..Default::default() });
        assert_eq!(get(&p, 1).0, Verdict::Reject);
        // Stars are never overridden.
        assert_eq!(get(&p, 4), (Verdict::Leave, Reason::Decided));
    }

    #[test]
    fn a_uniformly_soft_shoot_is_not_rejected_wholesale() {
        let s: Vec<CullPhoto> = (0..6).map(|i| photo(i, i * 200_000, 9.0 + i as f32 * 0.2, 0)).collect();
        let p = plan(&s, &CullOptions::default());
        assert!(p.suggestions.iter().all(|s| s.verdict == Verdict::Pick));
    }
}
