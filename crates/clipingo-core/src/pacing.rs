//! Tight pacing: which parts of a clip window to keep once dead air and fillers go.
//!
//! Port of BridgeClip's `compute_keep_intervals`. Times are ms inside the window.

/// Filler words cut under tight pacing (English set plus common Hindi/Hinglish fillers).
const FILLERS: &[&str] = &[
    "um", "uh", "uhm", "umm", "uhh", "erm", "er", "hmm", "hm", "mm", "mhm", "matlab", "मतलब",
];
/// Longest pause kept as-is per layout; `None` = never cut (silence on a screen is usually something happening).
pub const MAX_PAUSE_TALKING_HEAD_MS: Option<i64> = Some(700);
pub const MAX_PAUSE_SCREEN_CAM_MS: Option<i64> = Some(2000);
pub const MAX_PAUSE_SCREEN_MS: Option<i64> = None;

const KEEP_PAUSE_MS: i64 = 260;
const LEAD_IN_MS: i64 = 120;
const TAIL_MS: i64 = 380;
const MIN_CUT_MS: i64 = 200;
const MIN_PIECE_MS: i64 = 400;
const FRAME_MS: f64 = 1000.0 / 30.0;
const LONGFORM_MIN_PAUSE_MS: i64 = 1300;
const LONGFORM_KEEP_PAUSE_MS: i64 = 450;

/// A word inside the window.
#[derive(Debug, Clone)]
pub struct WindowWord {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

pub fn is_filler(word: &str) -> bool {
    let core: String = word.chars().filter(|c| c.is_alphabetic() || ('\u{0900}'..='\u{097F}').contains(c)).collect();
    FILLERS.contains(&core.to_lowercase().as_str())
}

fn snap(t: i64) -> i64 {
    ((t as f64 / FRAME_MS).round() * FRAME_MS).round() as i64
}

/// Union `protected` into `keeps`, rounding protection outward to whole frames.
fn preserve(keeps: Vec<(i64, i64)>, protected: &[(i64, i64)], window_ms: i64) -> Vec<(i64, i64)> {
    let mut all: Vec<(i64, i64)> = keeps;
    for &(a, b) in protected {
        let a = ((a as f64 / FRAME_MS).floor() * FRAME_MS) as i64;
        let b = ((b as f64 / FRAME_MS).ceil() * FRAME_MS) as i64;
        all.push((a.max(0), b.min(window_ms)));
    }
    all.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (a, b) in all {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Intervals to keep. `max_pause_ms` is the layout's limit (see the `MAX_PAUSE_*` constants);
/// `protected` intervals (reactions, laughter) are never cut.
pub fn keep_intervals(
    words: &[WindowWord],
    window_ms: i64,
    max_pause_ms: Option<i64>,
    protected: &[(i64, i64)],
    longform: bool,
) -> Vec<(i64, i64)> {
    let protected = preserve(Vec::new(), protected, window_ms);
    let keep_pause = if longform { LONGFORM_KEEP_PAUSE_MS } else { KEEP_PAUSE_MS };
    let limit = max_pause_ms.map(|t| if longform { t.max(LONGFORM_MIN_PAUSE_MS) } else { t });

    let spoken: Vec<&WindowWord> = words.iter().filter(|w| !is_filler(&w.text)).collect();
    let (Some(first), Some(last)) = (spoken.first(), spoken.last()) else {
        return vec![(0, window_ms)];
    };

    let mut cuts: Vec<(i64, i64)> = Vec::new();
    if limit.is_some() && first.start_ms > LEAD_IN_MS + MIN_CUT_MS {
        cuts.push((0, first.start_ms - LEAD_IN_MS));
    }
    if limit.is_some() && window_ms - last.end_ms > TAIL_MS + MIN_CUT_MS {
        cuts.push((last.end_ms + TAIL_MS, window_ms));
    }

    // A gap that swallowed a filler is always compressed; plain pauses only past the limit.
    for pair in spoken.windows(2) {
        let (prev, next) = (pair[0], pair[1]);
        let fillers: Vec<&WindowWord> = words
            .iter()
            .filter(|w| prev.end_ms <= w.start_ms && w.end_ms <= next.start_ms && is_filler(&w.text))
            .collect();
        let threshold = if fillers.is_empty() { limit } else { limit.map(|_| 0) };
        let Some(threshold) = threshold else { continue };
        if next.start_ms - prev.end_ms <= threshold.max(keep_pause) {
            continue;
        }
        let (mut a, mut b) = (prev.end_ms + keep_pause / 2, next.start_ms - keep_pause / 2);
        if let (Some(f0), Some(f1)) = (fillers.first(), fillers.last()) {
            // Remove each filler in full so no syllable survives frame rounding.
            a = a.min(f0.start_ms - FRAME_MS as i64);
            b = b.max(f1.end_ms + FRAME_MS as i64);
        }
        cuts.push((a, b));
    }

    // Never cut into protected moments; drop cuts too short to matter.
    cuts.sort_unstable();
    let mut trimmed = Vec::new();
    for (mut start, end) in cuts {
        for &(ps, pe) in &protected {
            if start < pe && end > ps {
                if ps - start >= MIN_CUT_MS {
                    trimmed.push((start, ps));
                }
                start = start.max(pe);
                if start >= end {
                    break;
                }
            }
        }
        if end - start >= MIN_CUT_MS {
            trimmed.push((start, end));
        }
    }

    // Snap to frames and invert cuts into keeps.
    trimmed.sort_unstable();
    let mut keeps = Vec::new();
    let mut cursor = 0;
    for (start, end) in trimmed {
        let start = snap(start.max(cursor));
        let end = if end >= window_ms { window_ms } else { snap(end) };
        if end - start < MIN_CUT_MS {
            continue;
        }
        if start > cursor {
            keeps.push((cursor, start));
        }
        cursor = end;
    }
    if cursor < window_ms {
        keeps.push((cursor, window_ms));
    }

    // Glue slivers back to their neighbour instead of flashing them.
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for piece in keeps {
        match merged.last_mut() {
            Some(last) if piece.1 - piece.0 < MIN_PIECE_MS => last.1 = piece.1,
            _ => merged.push(piece),
        }
    }
    if merged.len() > 1 && merged[0].1 - merged[0].0 < MIN_PIECE_MS {
        merged[1].0 = merged[0].0;
        merged.remove(0);
    }
    if merged.is_empty() {
        merged.push((0, window_ms));
    }
    preserve(merged, &protected, window_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, s: i64, e: i64) -> WindowWord {
        WindowWord { text: text.into(), start_ms: s, end_ms: e }
    }

    #[test]
    fn long_pause_on_talking_head_is_cut_short_one_kept() {
        let words = [w("hello", 0, 400), w("there", 600, 1000), w("friend", 3000, 3400)];
        let keeps = keep_intervals(&words, 3800, MAX_PAUSE_TALKING_HEAD_MS, &[], false);
        assert_eq!(keeps.len(), 2, "one cut in the 2 s gap: {keeps:?}");
        let removed: i64 = 3800 - keeps.iter().map(|k| k.1 - k.0).sum::<i64>();
        assert!((1600..=1800).contains(&removed), "removed {removed} ms");
    }

    #[test]
    fn screen_silence_is_never_cut() {
        let words = [w("look", 0, 400), w("now", 5000, 5400)];
        assert_eq!(keep_intervals(&words, 5400, MAX_PAUSE_SCREEN_MS, &[], false), vec![(0, 5400)]);
    }

    #[test]
    fn filler_in_gap_is_removed_even_when_short() {
        let words = [w("so", 0, 300), w("um", 500, 900), w("yes", 1100, 1500)];
        let keeps = keep_intervals(&words, 1500, MAX_PAUSE_TALKING_HEAD_MS, &[], false);
        let kept = |t: i64| keeps.iter().any(|k| k.0 <= t && t < k.1);
        assert!(!kept(700), "filler must be cut: {keeps:?}");
    }

    #[test]
    fn protected_reaction_survives() {
        let words = [w("joke", 0, 400), w("next", 4000, 4400)];
        let keeps = keep_intervals(&words, 4400, MAX_PAUSE_TALKING_HEAD_MS, &[(1000, 2500)], false);
        let kept = |t: i64| keeps.iter().any(|k| k.0 <= t && t < k.1);
        assert!(kept(1500) && kept(2400), "laughter must stay: {keeps:?}");
    }
}
