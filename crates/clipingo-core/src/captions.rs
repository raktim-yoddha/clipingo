//! Captions for a rendered clip: word-by-word highlighted ASS (burned in by libass) and SRT (for Premiere).
//! Times are on the clip's output timeline, in seconds.

use crate::Word;

/// Longest a caption stays up after its last word.
const MAX_LINGER_S: f64 = 0.7;
/// A pause this long starts a new caption page / SRT cue.
const PAUSE_BREAK_S: f64 = 0.6;
const SRT_LINE_CHARS: usize = 42;
const SRT_MAX_CUE_S: f64 = 6.0;
const SRT_LINGER_S: f64 = 0.4;
/// Active word colour (ASS is &HBBGGRR): yellow.
const HIGHLIGHT: &str = "&H0000E5FF&";

fn is_devanagari(s: &str) -> bool {
    s.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c))
}

fn ends_sentence(w: &str) -> bool {
    w.trim_end_matches(['"', '\'', '”', '’', ')']).ends_with(['.', '!', '?', '।', '॥'])
}

/// Groups of at most `per_page` words, broken at sentence ends and pauses.
fn pages(words: &[Word], per_page: usize) -> Vec<&[Word]> {
    let mut out = Vec::new();
    let mut begin = 0;
    for i in 0..words.len() {
        let last = i + 1 == words.len();
        let full = i + 1 - begin >= per_page;
        let pause = !last && words[i + 1].start - words[i].end >= PAUSE_BREAK_S;
        if last || full || pause || ends_sentence(&words[i].text) {
            out.push(&words[begin..=i]);
            begin = i + 1;
        }
    }
    out
}

fn ass_time(t: f64) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{}:{:02}:{:02}.{:02}", cs / 360_000, cs / 6000 % 60, cs / 100 % 60, cs % 100)
}

/// Text is data, never ASS markup.
fn ass_escape(s: &str) -> String {
    s.replace('\\', "\u{29F5}").replace('{', "(").replace('}', ")")
}

/// Bold word-by-word captions: each page shows its words, the one being spoken pops in yellow.
pub fn ass(words: &[Word], width: u32, height: u32) -> String {
    let vertical = height > width;
    let (size, margin_v, per_page) = if vertical { (82, 560, 3) } else { (60, 70, 6) };
    let font = if words.iter().any(|w| is_devanagari(&w.text)) { "Nirmala UI" } else { "Segoe UI Black" };
    let mut out = format!(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: {width}\nPlayResY: {height}\nWrapStyle: 0\nScaledBorderAndShadow: yes\n\n\
         [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Main,{font},{size},&H00FFFFFF,&H00FFFFFF,&H00000000,&H80000000,-1,0,0,0,100,100,0,0,1,6,2,2,70,70,{margin_v},1\n\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
    );
    let all_pages = pages(words, per_page);
    for (p, page) in all_pages.iter().enumerate() {
        let next_page_start = all_pages.get(p + 1).map(|n| n[0].start);
        for (i, w) in page.iter().enumerate() {
            let start = w.start;
            let end = match page.get(i + 1) {
                Some(next) => next.start,
                None => {
                    let linger = page[i].end + MAX_LINGER_S;
                    next_page_start.map_or(linger, |n| n.min(linger))
                }
            };
            let text: Vec<String> = page
                .iter()
                .enumerate()
                .map(|(j, x)| {
                    let t = ass_escape(&x.text);
                    if j == i { format!("{{\\c{HIGHLIGHT}\\fscx108\\fscy108}}{t}{{\\r}}") } else { t }
                })
                .collect();
            out.push_str(&format!("Dialogue: 0,{},{},Main,,0,0,0,,{}\n", ass_time(start), ass_time(end.max(start + 0.05)), text.join(" ")));
        }
    }
    out
}

fn srt_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// Two balanced lines when a cue is longer than one line.
fn wrap(text: &str) -> String {
    if text.chars().count() <= SRT_LINE_CHARS {
        return text.to_string();
    }
    let words: Vec<&str> = text.split(' ').collect();
    let total = text.chars().count();
    let mut acc = 0;
    for (i, w) in words.iter().enumerate() {
        acc += w.chars().count() + 1;
        if acc >= total / 2 {
            return format!("{}\n{}", words[..=i].join(" "), words[i + 1..].join(" "));
        }
    }
    text.to_string()
}

/// SRT cues of at most two 42-character lines and 6 s, broken at pauses.
pub fn srt(words: &[Word]) -> String {
    let mut cues: Vec<(f64, f64, String)> = Vec::new();
    let mut cur: Vec<&Word> = Vec::new();
    let flush = |cur: &mut Vec<&Word>, cues: &mut Vec<(f64, f64, String)>| {
        if let (Some(f), Some(l)) = (cur.first(), cur.last()) {
            let text = cur.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
            cues.push((f.start, l.end, text));
        }
        cur.clear();
    };
    for w in words {
        if let Some(last) = cur.last() {
            let len: usize = cur.iter().map(|x| x.text.chars().count() + 1).sum::<usize>() + w.text.chars().count();
            if len > SRT_LINE_CHARS * 2 || w.end - cur[0].start > SRT_MAX_CUE_S || w.start - last.end >= PAUSE_BREAK_S + 0.1 {
                flush(&mut cur, &mut cues);
            }
        }
        cur.push(w);
        if ends_sentence(&w.text) && cur.len() > 3 {
            flush(&mut cur, &mut cues);
        }
    }
    flush(&mut cur, &mut cues);
    let mut out = String::new();
    for (i, (s, e, text)) in cues.iter().enumerate() {
        let next = cues.get(i + 1).map_or(f64::INFINITY, |c| c.0);
        let end = (e + SRT_LINGER_S).min(next);
        out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(*s), srt_time(end), wrap(text)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: &str, s: f64, e: f64) -> Word {
        Word { text: t.into(), start: s, end: e }
    }

    #[test]
    fn pages_break_at_size_sentence_and_pause() {
        let words = [w("one", 0.0, 0.2), w("two", 0.3, 0.5), w("three.", 0.6, 0.8), w("four", 0.9, 1.0), w("five", 2.0, 2.2)];
        let p = pages(&words, 3);
        let texts: Vec<Vec<&str>> = p.iter().map(|pg| pg.iter().map(|x| x.text.as_str()).collect()).collect();
        assert_eq!(texts, vec![vec!["one", "two", "three."], vec!["four"], vec!["five"]]);
    }

    #[test]
    fn ass_highlights_each_word_and_escapes_markup() {
        let words = [w("hello", 0.0, 0.4), w("{evil}", 0.5, 0.9)];
        let a = ass(&words, 1080, 1920);
        assert!(a.contains("PlayResY: 1920") && a.contains("Segoe UI Black"));
        let events: Vec<&str> = a.lines().filter(|l| l.starts_with("Dialogue")).collect();
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("0:00:00.00,0:00:00.50") && events[0].contains("}hello{\\r}"));
        assert!(events[1].contains("(evil)") && !events[1].contains("{evil}"));
    }

    #[test]
    fn devanagari_switches_font() {
        assert!(ass(&[w("क्या", 0.0, 0.5)], 1080, 1920).contains("Nirmala UI"));
    }

    #[test]
    fn srt_cues_wrap_and_break_on_pause() {
        let mut words: Vec<Word> = (0..12).map(|i| w("longword", i as f64 * 0.4, i as f64 * 0.4 + 0.3)).collect();
        words.push(w("after", 10.0, 10.3));
        let s = srt(&words);
        assert!(s.starts_with("1\n00:00:00,000 --> "));
        assert!(s.contains("\n2\n") && s.contains("00:00:10,000"), "{s}");
        assert!(s.lines().all(|l| l.chars().count() <= SRT_LINE_CHARS + 10));
    }
}
