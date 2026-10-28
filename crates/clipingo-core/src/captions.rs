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
/// Pop-in on each new page (BridgeClip's entrance).
const POP_IN: &str = "\\fscx82\\fscy82\\t(0,90,\\fscx106\\fscy106)\\t(90,170,\\fscx100\\fscy100)";

/// How the active word stands out.
#[derive(Debug, Clone, Copy)]
enum Look {
    /// Active word in the highlight colour.
    Plain,
    /// Active word on a coloured pill (its outline, thickened).
    Pill(&'static str),
    /// Active word with a blurred colour bloom underneath.
    Glow(&'static str),
    /// Whole page on a box (`#AARRGGBB`).
    Boxed(&'static str),
    /// Colour fills each word as it is spoken (karaoke); optionally on a box.
    Sweep(Option<&'static str>),
}

/// One caption look. Ids, names and colours follow BridgeClip's 13 presets.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub font: &'static str,
    pub italic: bool,
    /// `#RRGGBB`.
    pub primary: &'static str,
    pub highlight: &'static str,
    /// Size at 1080 px wide; landscape uses 3/4 of it.
    #[serde(skip)]
    size: u32,
    #[serde(skip)]
    words_per_page: usize,
    #[serde(skip)]
    outline: u32,
    pub upper: bool,
    #[serde(skip)]
    pop: bool,
    #[serde(skip)]
    look: Look,
}

// ponytail: Windows system fonts stand in for BridgeClip's bundled OFL fonts (Montserrat, Anton, Archivo Black,
// Instrument Serif, Poppins); bundle them with the installer for exact looks and for macOS.
#[rustfmt::skip]
pub const PRESETS: &[Preset] = &[
    Preset { id: "pop", name: "Pop", description: "The all-rounder", font: "Segoe UI Black", italic: false, primary: "#FFFFFF", highlight: "#FFE234", size: 82, words_per_page: 3, outline: 6, upper: false, pop: true, look: Look::Plain },
    Preset { id: "spotlight", name: "Spotlight", description: "Word on a pill", font: "Segoe UI Black", italic: false, primary: "#FFFFFF", highlight: "#FFFFFF", size: 78, words_per_page: 3, outline: 5, upper: false, pop: true, look: Look::Pill("#7C5CFF") },
    Preset { id: "impact", name: "Impact", description: "Tall, two words at a time", font: "Impact", italic: false, primary: "#FFFFFF", highlight: "#FFD60A", size: 124, words_per_page: 2, outline: 6, upper: true, pop: true, look: Look::Plain },
    Preset { id: "glow", name: "Glow", description: "Cyan bloom, tech & gaming", font: "Segoe UI Black", italic: false, primary: "#FFFFFF", highlight: "#7DF9FF", size: 80, words_per_page: 3, outline: 4, upper: false, pop: true, look: Look::Glow("#00C8FF") },
    Preset { id: "boxed", name: "Boxed", description: "Readable on any footage", font: "Arial Black", italic: false, primary: "#FFFFFF", highlight: "#FFD23F", size: 68, words_per_page: 3, outline: 0, upper: false, pop: true, look: Look::Boxed("#50101014") },
    Preset { id: "sweep", name: "Sweep", description: "Colour follows the voice", font: "Segoe UI Black", italic: false, primary: "#FFFFFF", highlight: "#FF5FA2", size: 80, words_per_page: 4, outline: 6, upper: false, pop: false, look: Look::Sweep(None) },
    Preset { id: "editorial", name: "Editorial", description: "Serif for podcasts & stories", font: "Georgia", italic: true, primary: "#FFFFFF", highlight: "#FFE6B8", size: 84, words_per_page: 4, outline: 2, upper: false, pop: false, look: Look::Sweep(None) },
    Preset { id: "hype", name: "Hype", description: "Heavy stroke, high energy", font: "Segoe UI Black", italic: false, primary: "#FFFFFF", highlight: "#39FF6A", size: 90, words_per_page: 2, outline: 11, upper: true, pop: true, look: Look::Plain },
    Preset { id: "punch", name: "Punch", description: "One huge word at a time", font: "Impact", italic: false, primary: "#FFFFFF", highlight: "#FFFFFF", size: 150, words_per_page: 1, outline: 8, upper: true, pop: true, look: Look::Plain },
    Preset { id: "neon", name: "Neon", description: "Magenta bloom, music & lifestyle", font: "Segoe UI Black", italic: false, primary: "#FFFFFF", highlight: "#FF9CEB", size: 80, words_per_page: 3, outline: 4, upper: false, pop: true, look: Look::Glow("#FF2EC4") },
    Preset { id: "headline", name: "Headline", description: "Word on a red news tag", font: "Arial Black", italic: false, primary: "#FFFFFF", highlight: "#FFFFFF", size: 70, words_per_page: 3, outline: 5, upper: true, pop: false, look: Look::Pill("#E5202E") },
    Preset { id: "paper", name: "Paper", description: "Dark type on a white card", font: "Segoe UI Black", italic: false, primary: "#111111", highlight: "#6D28D9", size: 70, words_per_page: 4, outline: 0, upper: false, pop: false, look: Look::Sweep(Some("#00FFFFFF")) },
    Preset { id: "subtle", name: "Subtle", description: "Light touch for interviews & vlogs", font: "Segoe UI Semibold", italic: false, primary: "#FFFFFF", highlight: "#C4F1FF", size: 60, words_per_page: 5, outline: 3, upper: false, pop: false, look: Look::Plain },
];

/// The preset with this id, or Pop.
pub fn preset(id: &str) -> &'static Preset {
    PRESETS.iter().find(|p| p.id == id).unwrap_or(&PRESETS[0])
}

/// `#RRGGBB` or `#AARRGGBB` (alpha 00 = opaque, as in ASS) → `&HAABBGGRR&`.
fn ass_color(hex: &str) -> String {
    let h = hex.trim_start_matches('#');
    let (a, rgb) = if h.len() == 8 { h.split_at(2) } else { ("00", h) };
    format!("&H{a}{}{}{}&", &rgb[4..6], &rgb[2..4], &rgb[0..2])
}

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

/// Where captions sit on the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    Top,
    Middle,
    /// Lower third (above the platforms' bottom UI on 9:16).
    #[default]
    Bottom,
}

/// Caption placement: position and a size multiplier (0.6–1.6, 1 = the preset's size).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub position: Position,
    pub scale: f64,
}

impl Default for Placement {
    fn default() -> Self {
        Self { position: Position::Bottom, scale: 1.0 }
    }
}

/// Word-by-word captions in `style` (see `PRESETS`), plus an optional title `(text, clip length)` at the top.
/// Pass no words for a title-only overlay.
pub fn ass(
    words: &[Word],
    width: u32,
    height: u32,
    style: &Preset,
    place: Placement,
    title: Option<(&str, f64)>,
) -> String {
    let vertical = height > width;
    let devanagari = words.iter().any(|w| is_devanagari(&w.text));
    let font = if devanagari { "Nirmala UI" } else { style.font };
    let italic = if style.italic && !devanagari { -1 } else { 0 };
    let base_size = if vertical { style.size } else { style.size * 3 / 4 };
    let size = (base_size as f64 * place.scale.clamp(0.6, 1.6)).round() as u32;
    let per_page = if vertical { style.words_per_page } else { (style.words_per_page * 2).min(6) };
    // ASS numpad alignment: 2 bottom centre, 5 middle, 8 top (below the title on 9:16).
    let (alignment, margin_v) = match (place.position, vertical) {
        (Position::Bottom, true) => (2, 560),
        (Position::Bottom, false) => (2, 70),
        (Position::Middle, _) => (5, 0),
        (Position::Top, true) => (8, 330),
        (Position::Top, false) => (8, 130),
    };
    let (base, hl, black) = (ass_color(style.primary), ass_color(style.highlight), "&H00000000&");
    // Sweep fills from SecondaryColour to PrimaryColour, so the highlight is the primary there.
    let (primary, secondary) = if matches!(style.look, Look::Sweep(_)) { (&hl, &base) } else { (&base, &base) };
    let (border_style, outline_colour, outline) = match style.look {
        Look::Boxed(c) | Look::Sweep(Some(c)) => (3, ass_color(c), 14),
        _ => (1, black.to_string(), style.outline),
    };
    let title_font = if title.is_some_and(|(t, _)| is_devanagari(t)) { "Nirmala UI" } else { "Segoe UI Black" };
    let (title_size, title_margin) = if vertical { (58, 150) } else { (46, 40) };
    let mut out = format!(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: {width}\nPlayResY: {height}\nWrapStyle: 0\nScaledBorderAndShadow: yes\n\n\
         [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Main,{font},{size},{primary},{secondary},{outline_colour},&H80000000,-1,{italic},0,0,100,100,0,0,{border_style},{outline},2,{alignment},70,70,{margin_v},1\n\
         Style: Title,{title_font},{title_size},&H00FFFFFF,&H00FFFFFF,&H40000000,&H00000000,-1,0,0,0,100,100,0,0,3,18,0,8,90,90,{title_margin},1\n\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
    );
    if let Some((text, len)) = title {
        out.push_str(&format!("Dialogue: 2,{},{},Title,,0,0,0,,{}\n", ass_time(0.0), ass_time(len), ass_escape(text)));
    }
    let shown = |w: &Word| ass_escape(&if style.upper { w.text.to_uppercase() } else { w.text.clone() });
    let event = |layer: u8, start: f64, end: f64, text: &str| {
        format!("Dialogue: {layer},{},{},Main,,0,0,0,,{text}\n", ass_time(start), ass_time(end.max(start + 0.05)))
    };

    let all_pages = pages(words, per_page);
    for (p, page) in all_pages.iter().enumerate() {
        let next_page_start = all_pages.get(p + 1).map(|n| n[0].start);
        let linger = |last: &Word| next_page_start.map_or(last.end + MAX_LINGER_S, |n| n.min(last.end + MAX_LINGER_S));
        let pop = if style.pop { format!("{{{POP_IN}}}") } else { String::new() };

        if let Look::Sweep(_) = style.look {
            // One event per page; each word's \kf runs until the next word starts.
            let text: Vec<String> = page
                .iter()
                .enumerate()
                .map(|(j, w)| {
                    let until = page.get(j + 1).map_or(w.end, |n| n.start);
                    format!("{{\\kf{}}}{}", ((until - w.start) * 100.0).round().max(1.0) as u64, shown(w))
                })
                .collect();
            out.push_str(&event(0, page[0].start, linger(&page[page.len() - 1]), &text.join(" ")));
            continue;
        }

        for (i, w) in page.iter().enumerate() {
            let end = page.get(i + 1).map_or_else(|| linger(w), |next| next.start);
            let entrance = if i == 0 { pop.as_str() } else { "" };
            let line = |active: &dyn Fn(&str) -> String, other: &dyn Fn(&str) -> String| {
                let words: Vec<String> = page
                    .iter()
                    .enumerate()
                    .map(|(j, x)| if j == i { active(&shown(x)) } else { other(&shown(x)) })
                    .collect();
                format!("{entrance}{}", words.join(" "))
            };
            // Explicit resets instead of \r, so the page's pop-in keeps scaling the other words.
            let main = match style.look {
                Look::Pill(c) => line(
                    &|t| format!("{{\\c{hl}\\3c{}\\bord16}}{t}{{\\c{base}\\3c{black}\\bord{outline}}}", ass_color(c)),
                    &|t| t.to_string(),
                ),
                _ => line(&|t| format!("{{\\c{hl}}}{t}{{\\c{base}}}"), &|t| t.to_string()),
            };
            if let Look::Glow(c) = style.look {
                // Same text one layer down: only the active word's blurred outline shows.
                let glow = line(
                    &|t| format!("{{\\1a&HFF&\\3a&H30&\\3c{}\\bord14\\blur12}}{t}{{\\alpha&HFF&}}", ass_color(c)),
                    &|t| format!("{{\\alpha&HFF&}}{t}"),
                );
                out.push_str(&event(0, w.start, end, &glow));
            }
            out.push_str(&event(1, w.start, end, &main));
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
            if len > SRT_LINE_CHARS * 2
                || w.end - cur[0].start > SRT_MAX_CUE_S
                || w.start - last.end >= PAUSE_BREAK_S + 0.1
            {
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
        let words =
            [w("one", 0.0, 0.2), w("two", 0.3, 0.5), w("three.", 0.6, 0.8), w("four", 0.9, 1.0), w("five", 2.0, 2.2)];
        let p = pages(&words, 3);
        let texts: Vec<Vec<&str>> = p.iter().map(|pg| pg.iter().map(|x| x.text.as_str()).collect()).collect();
        assert_eq!(texts, vec![vec!["one", "two", "three."], vec!["four"], vec!["five"]]);
    }

    fn events(a: &str) -> Vec<&str> {
        a.lines().filter(|l| l.starts_with("Dialogue")).collect()
    }

    #[test]
    fn ass_highlights_each_word_and_escapes_markup() {
        let words = [w("hello", 0.0, 0.4), w("{evil}", 0.5, 0.9)];
        let a = ass(&words, 1080, 1920, preset("pop"), Placement::default(), None);
        assert!(a.contains("PlayResY: 1920") && a.contains("Segoe UI Black"));
        let ev = events(&a);
        assert_eq!(ev.len(), 2);
        assert!(ev[0].contains("0:00:00.00,0:00:00.50") && ev[0].contains("\\fscx82"), "page pops in: {}", ev[0]);
        assert!(ev[0].contains("{\\c&H0034E2FF&}hello{\\c&H00FFFFFF&}"), "active word in #FFE234: {}", ev[0]);
        assert!(ev[1].contains("(evil)") && !ev[1].contains("{evil}"));
    }

    #[test]
    fn presets_change_the_look() {
        let words = [w("one", 0.0, 0.4), w("two", 0.5, 0.9)];
        assert_eq!(PRESETS.len(), 13);
        assert_eq!(preset("nope").id, "pop");

        let sweep = ass(&words, 1080, 1920, preset("sweep"), Placement::default(), None);
        let ev = events(&sweep);
        assert_eq!(ev.len(), 1, "one karaoke event per page");
        assert!(ev[0].contains("{\\kf50}one {\\kf40}two"), "{}", ev[0]);

        let glow = ass(&words, 1080, 1920, preset("glow"), Placement::default(), None);
        assert_eq!(events(&glow).len(), 4, "bloom layer under each word event");
        assert!(glow.contains("\\blur12") && glow.contains("&H00FFC800&"), "cyan #00C8FF bloom");

        let punch = ass(&words, 1080, 1920, preset("punch"), Placement::default(), None);
        assert!(punch.contains("ONE") && events(&punch).len() == 2, "one upper-case word per page");

        let paper = ass(&words, 1080, 1920, preset("paper"), Placement::default(), None);
        assert!(paper.contains(",3,14,") && paper.contains("&H00FFFFFF&,&H80000000"), "white card box");
    }

    #[test]
    fn title_sits_on_top_for_the_whole_clip() {
        let a = ass(&[], 1080, 1920, preset("pop"), Placement::default(), Some(("Why {this} works", 31.5)));
        let ev = events(&a);
        assert_eq!(ev.len(), 1);
        assert!(ev[0].starts_with("Dialogue: 2,0:00:00.00,0:00:31.50,Title,") && ev[0].ends_with("Why (this) works"));
    }

    #[test]
    fn devanagari_switches_font() {
        assert!(
            ass(&[w("क्या", 0.0, 0.5)], 1080, 1920, preset("editorial"), Placement::default(), None)
                .contains("Main,Nirmala UI")
        );
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

    #[test]
    fn placement_moves_and_scales_the_main_style() {
        let words = [w("hi", 0.0, 0.5)];
        let main = |place| ass(&words, 1080, 1920, preset("pop"), place, None);
        assert!(main(Placement::default()).contains("Style: Main,Segoe UI Black,82,"));
        let top = main(Placement { position: Position::Top, scale: 1.5 });
        assert!(top.contains("Style: Main,Segoe UI Black,123,") && top.contains(",2,8,70,70,330,1"), "{top}");
        let mid = main(Placement { position: Position::Middle, scale: 9.0 });
        assert!(mid.contains(",131,") && mid.contains(",2,5,70,70,0,1"), "size clamped to 1.6×: {mid}");
    }
}
