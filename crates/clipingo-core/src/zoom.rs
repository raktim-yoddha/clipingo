//! Smart zoom: edits like a person would. It reads the clip sentence by sentence and only pushes in where it helps
//! (the hook, a question, a line with a strong word), keeps zooms apart, comes in three sizes, never starts right
//! after a camera cut or runs across one, and gives long still stretches one slow push-in. Rules follow Clippo's
//! auto zoom. Times are on the clip's timeline.

use crate::Word;
use crate::emphasis::strength;

const SUBTLE: f64 = 0.08;
const MEDIUM: f64 = 0.14;
const STRONG: f64 = 0.21;
/// Least time between two zoom starts; a very strong moment may come at 60 % of it.
const GAP_S: f64 = 5.0;
/// A still stretch this long gets one slow push-in.
const STILL_S: f64 = 12.0;

/// One push-in: `amount` extra zoom between `t` and `end`, easing in and back out after `end`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Zoom {
    pub t: f64,
    pub end: f64,
    pub amount: f64,
    /// True for the slow variety push-in; false for a punch on a word.
    pub slow: bool,
    /// The word it lands on.
    pub word: String,
}

impl Zoom {
    /// Seconds to ease in (bigger zooms move a little slower).
    fn ease_in(&self) -> f64 {
        if self.slow { self.end - self.t } else { 0.3 + self.amount * 2.0 }
    }
    fn ease_out(&self) -> f64 {
        0.45 + self.amount * 2.0
    }
    /// When it has fully eased out.
    pub fn until(&self) -> f64 {
        self.end + self.ease_out()
    }
}

struct Moment {
    t: f64,
    end: f64,
    score: f64,
    word: String,
}

fn ends_sentence(w: &str) -> bool {
    w.trim_end_matches(['"', '\'', '”', '’', ')']).ends_with(['.', '!', '?', '।', '॥', '…'])
}

/// Sentences of the clip's words as index ranges: broken at sentence ends, pauses and 28 words.
fn sentences(words: &[Word]) -> Vec<&[Word]> {
    let (mut out, mut begin) = (Vec::new(), 0);
    for i in 0..words.len() {
        let pause = i + 1 < words.len() && words[i + 1].start - words[i].end > 0.9;
        if ends_sentence(&words[i].text) || pause || i + 1 - begin >= 28 || i + 1 == words.len() {
            out.push(&words[begin..=i]);
            begin = i + 1;
        }
    }
    out
}

/// How much a zoom would add at each sentence.
fn moments(words: &[Word], duration: f64) -> Vec<Moment> {
    let sents = sentences(words);
    sents
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let best = s.iter().max_by(|a, b| strength(&a.text).total_cmp(&strength(&b.text))).expect("non-empty");
            let peak = strength(&best.text);
            let mut score = peak;
            if i == 0 && s[0].start < 3.0 {
                score += 0.55; // the hook
            }
            let tail = s[s.len() - 1].text.trim_end_matches(['"', '\'', '”', '’', ')']);
            if tail.ends_with(['!', '?']) {
                score += 0.35;
            }
            if i > 0 && s[0].start - sents[i - 1][sents[i - 1].len() - 1].end > 0.7 {
                score += 0.25; // a new thought after a pause
            }
            if s.len() <= 2 && i > 0 {
                score -= 0.35; // fragments are rarely worth it
            }
            let at = if peak >= 1.3 { best } else { &s[0] };
            let t = (at.start - 0.08).max(0.0);
            let end = (s[s.len() - 1].end + 0.15).min(at.start + 3.2).max(at.start + 1.1).min(duration - 0.1);
            Moment { t, end, score, word: at.text.clone() }
        })
        .collect()
}

/// Keeps a zoom clear of camera cuts: none starts within 0.7 s after one, none runs over one.
fn fit_cuts(t: f64, mut end: f64, cuts: &[f64]) -> Option<f64> {
    for &c in cuts {
        if c > t - 0.7 && c <= t + 0.05 {
            return None;
        }
        if c > t && c < end + 0.5 {
            end = end.min(c - 0.55);
        }
    }
    (end - t >= 0.8).then_some(end)
}

/// The zooms of a clip `duration` seconds long with these words and camera cuts, in time order.
pub fn plan(words: &[Word], cuts: &[f64], duration: f64) -> Vec<Zoom> {
    if words.is_empty() || duration < 3.0 {
        return Vec::new();
    }
    let cap = if duration < 10.0 { 1 } else { (duration / 8.0).round().max(1.0) as usize };
    let mut all = moments(words, duration);
    all.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.t.total_cmp(&b.t)));
    let mut chosen: Vec<(Moment, f64)> = Vec::new();
    for m in all.into_iter().filter(|m| m.score >= 1.6) {
        if chosen.len() >= cap || (duration < 10.0 && m.score < 2.2) {
            continue;
        }
        let Some(end) = fit_cuts(m.t, m.end, cuts) else { continue };
        let need = if m.score >= 3.0 { GAP_S * 0.6 } else { GAP_S };
        let clear = chosen.iter().all(|(p, pe)| (p.t - m.t).abs() >= need && (m.t >= pe + 1.4 || p.t >= end + 1.4));
        if clear {
            chosen.push((m, end));
        }
    }
    chosen.sort_by(|a, b| a.0.t.total_cmp(&b.0.t));
    // Sizes: strong is rare (one per 20 s), and the same size twice in a row is varied.
    let (mut last_strong, mut prev) = (f64::MIN, 0.0);
    let mut out: Vec<Zoom> = Vec::new();
    for (m, end) in chosen {
        let mut amount = if m.score >= 2.9 {
            STRONG
        } else if m.score >= 2.1 {
            MEDIUM
        } else {
            SUBTLE
        };
        if amount == STRONG && m.t - last_strong < 20.0 {
            amount = MEDIUM;
        }
        if amount == prev && amount != STRONG {
            amount = if amount == MEDIUM { SUBTLE } else { MEDIUM };
        }
        if amount == STRONG {
            last_strong = m.t;
        }
        prev = amount;
        out.push(Zoom { t: m.t, end, amount, slow: false, word: m.word });
    }
    // Long still stretches (no zoom, no cut) get one slow push-in, starting on a sentence.
    let mut busy: Vec<(f64, f64)> = out.iter().map(|z| (z.t - 1.0, z.end + 1.0)).collect();
    busy.extend(cuts.iter().map(|&c| (c - 0.5, c + 0.5)));
    busy.sort_by(|a, b| a.0.total_cmp(&b.0));
    busy.push((duration - 1.0, duration));
    let mut from = 1.0;
    let mut slow = Vec::new();
    for (a, b) in busy {
        if a - from >= STILL_S {
            let s =
                sentences(words).into_iter().find(|s| s[0].start >= from + (a - from) * 0.3 && s[0].start < a - 5.0);
            if let Some(s) = s {
                let t = s[0].start - 0.05;
                slow.push(Zoom { t, end: (a - 1.2).min(t + 4.5), amount: SUBTLE, slow: true, word: s[0].text.clone() });
            }
        }
        from = from.max(b);
    }
    out.extend(slow);
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    out
}

/// Zoom factor at `t` (1 = none): the same curve as `expr`, for the editor's preview.
pub fn factor_at(zooms: &[Zoom], t: f64) -> f64 {
    let sine = |k: f64| 0.5 - 0.5 * (std::f64::consts::PI * k.clamp(0.0, 1.0)).cos();
    let mut v: f64 = 0.0;
    for z in zooms.iter().filter(|z| t >= z.t && t <= z.until()) {
        let up = sine((t - z.t) / z.ease_in());
        let x =
            if t <= z.end { up } else { sine((z.end - z.t) / z.ease_in()) * (1.0 - sine((t - z.end) / z.ease_out())) };
        v = v.max(z.amount * x);
    }
    1.0 + v
}

/// The zoom factor as an ffmpeg expression of `t`, on a timeline starting at `origin`.
pub fn expr(zooms: &[Zoom], origin: f64) -> String {
    let mut e = String::from("1");
    for z in zooms.iter().rev() {
        let (t0, end, until) = (z.t - origin, z.end - origin, z.until() - origin);
        let (ein, eout, a) = (z.ease_in(), z.ease_out(), z.amount);
        let up = |t: String| format!("(0.5-0.5*cos(PI*min(1,({t})/{ein:.3})))");
        let held = up(format!("{:.3}", end - t0));
        let rise = format!("1+{a:.3}*{}", up(format!("t-{t0:.3}")));
        let fall = format!("1+{a:.3}*{held}*(0.5+0.5*cos(PI*min(1,(t-{end:.3})/{eout:.3})))");
        e = format!("if(between(t,{t0:.3},{end:.3}),{rise},if(between(t,{end:.3},{until:.3}),{fall},{e}))");
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, start: f64) -> Vec<Word> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, w)| Word { text: w.into(), start: start + i as f64 * 0.4, end: start + i as f64 * 0.4 + 0.35 })
            .collect()
    }

    fn talk() -> Vec<Word> {
        let mut w = Vec::new();
        for (i, l) in [
            "Why did nobody tell me this?",
            "We moved the folder over there.",
            "That cost us a million dollars!",
            "Then we opened the file again.",
        ]
        .iter()
        .cycle()
        .take(12)
        .enumerate()
        {
            w.extend(line(l, i as f64 * 4.0));
        }
        w
    }

    #[test]
    fn zooms_are_spaced_and_land_on_strong_lines() {
        let z = plan(&talk(), &[], 48.0);
        assert!(!z.is_empty() && z.len() <= 7, "{z:?}");
        for p in z.windows(2).filter(|p| !p[0].slow && !p[1].slow) {
            assert!(p[1].t - p[0].t >= GAP_S * 0.6 - 1e-9, "{p:?}");
            assert!(p[1].t >= p[0].end, "never overlap: {p:?}");
        }
        assert_eq!(z[0].word, "Why", "opens on the hook");
    }

    #[test]
    fn zooms_keep_clear_of_camera_cuts() {
        let cuts = [0.3, 8.0, 16.0, 24.0];
        for z in plan(&talk(), &cuts, 30.0) {
            assert!(cuts.iter().all(|&c| !(c > z.t - 0.7 && c <= z.t + 0.05)), "starts just after a cut: {z:?}");
            assert!(cuts.iter().all(|&c| !(c > z.t && c < z.end)), "runs over a cut: {z:?}");
        }
    }

    #[test]
    fn nothing_to_zoom_on_a_flat_line_but_still_stretches_get_a_slow_push() {
        let flat: Vec<Word> = (0..8).flat_map(|i| line("we moved the folder over there.", i as f64 * 5.0)).collect();
        let z = plan(&flat, &[], 40.0);
        assert_eq!(z.iter().filter(|z| !z.slow).count(), 0, "{z:?}");
        assert!(z.iter().any(|z| z.slow), "long still stretch: {z:?}");
    }

    #[test]
    fn curve_and_expression_agree() {
        let z = vec![Zoom { t: 2.0, end: 4.0, amount: 0.14, slow: false, word: "x".into() }];
        assert_eq!(factor_at(&z, 1.0), 1.0);
        assert!((factor_at(&z, 3.5) - 1.14).abs() < 1e-6, "held");
        assert_eq!(factor_at(&z, z[0].until() + 0.1), 1.0, "eased out");
        let e = expr(&z, 1.0);
        assert!(e.contains("between(t,1.000,3.000)") && e.ends_with(",1))"), "{e}");
    }
}
