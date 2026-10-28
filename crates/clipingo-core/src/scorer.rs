//! Offline viral-moment scorer over a transcript (English, Hindi, Hinglish).
//!
//! Scores sentence-aligned windows on hook strength, emotion, story/insight/debate cues,
//! vocal energy, pace, completeness and topic focus. Follows the approach of Clippo's
//! clip finder (same feature weights and selection rules); lexicons are our own.

use std::collections::{HashMap, HashSet};

use crate::Word;

const SENTENCE_GAP_S: f64 = 1.2;
const MAX_SENTENCE_WORDS: usize = 45;

fn list(s: &str) -> Vec<&str> {
    s.split('|').collect()
}

struct Lexicon {
    hook: Vec<&'static str>,
    question: Vec<&'static str>,
    you: Vec<&'static str>,
    emotion: Vec<&'static str>,
    story: Vec<&'static str>,
    insight: Vec<&'static str>,
    debate: Vec<&'static str>,
    cont: Vec<&'static str>,
    number: Vec<&'static str>,
    laugh: Vec<&'static str>,
    filler: Vec<&'static str>,
    anaphor: Vec<&'static str>,
    stop: HashSet<&'static str>,
}

fn lexicon() -> &'static Lexicon {
    static LEX: std::sync::OnceLock<Lexicon> = std::sync::OnceLock::new();
    LEX.get_or_init(|| Lexicon {
        hook: list("the truth|nobody|no one|never|secret|biggest|mistake|stop|here's why|the reason|most people|imagine|the problem|the worst|you need|crazy|insane|million|money|failed|wrong|honestly|i realized|what if|did you know|hard truth|unpopular|warning|watch this|look at this|it works|it broke|sach|galti|raaz|kabhi nahi|sabse|paisa|crore|lakh|asli|suno|dekho|ek baat|सच|गलती|राज़|कभी नहीं|सबसे|पैसा|करोड़|लाख|असली|सुनो|देखो|एक बात"),
        question: list("what|why|how|who|when|where|which|would you|have you|do you|can you|is it|kya|kyun|kaise|kab|kaun|kahan|kitna|क्या|क्यों|कैसे|कब|कौन|कहाँ|कितना"),
        you: list("you|your|you're|yourself|aap|aapko|aapka|tum|tumhe|tu|tera|आप|आपको|आपका|तुम|तुम्हें|तू|तेरा"),
        emotion: list("love|hate|scared|fear|angry|cried|amazing|incredible|shocking|terrible|painful|lost|happy|sad|stress|panic|excited|proud|hilarious|funny|wow|omg|damn|unbelievable|brutal|legend|genius|stupid|disaster|nightmare|pyaar|darr|gussa|pagal|kamaal|zabardast|bekaar|bakwas|dard|khushi|mazaa|tension|प्यार|डर|गुस्सा|पागल|कमाल|ज़बरदस्त|बेकार|बकवास|दर्द|खुशी|मज़ा"),
        story: list("i remember|when i was|one day|my dad|my mom|my friend|story|years ago|first time|the day|turns out|and then|ek baar|ek din|jab main|mere dost|kahani|us din|pehli baar|bachpan|एक बार|एक दिन|जब मैं|कहानी|उस दिन|पहली बार|बचपन"),
        insight: list("the key|the lesson|what i learned|here's the thing|the point is|remember this|the rule|if you want|the only way|step one|baat ye hai|asli baat|seekha|yaad rakho|tarika|agar aap|बात ये है|असली बात|सीखा|याद रखो|तरीका|अगर आप"),
        debate: list("disagree|i agree|myth|overrated|underrated|nobody talks about|unpopular opinion|versus|controversial|no way|not true|fake|scam|galat|jhooth|bilkul nahi|faltu|गलत|झूठ|बिल्कुल नहीं|फालतू"),
        cont: list("and|so|but|because|or|also|then|like|um|uh|which|that|yeah|okay|ok|right|aur|toh|to|lekin|par|kyunki|phir|matlab|haan|achha|और|तो|लेकिन|पर|क्योंकि|फिर|मतलब|हाँ|अच्छा"),
        number: list("one|two|three|four|five|ten|hundred|thousand|million|billion|percent|ek|do|teen|char|paanch|das|sau|hazaar|lakh|crore|एक|दो|तीन|चार|पांच|दस|सौ|हज़ार|लाख|करोड़"),
        laugh: list("haha|hahaha|lol|laughter|laughs|😂|🤣"),
        filler: list("um|uh|umm|hmm|you know|i mean|kind of|sort of|matlab|basically|मतलब"),
        anaphor: list("that|this|it|they|he|she|those|these|there|wo|woh|ye|yeh|वो|ये"),
        stop: list("the|a|an|and|or|but|so|to|of|in|on|at|for|with|is|are|was|were|be|been|it|its|this|that|these|those|i|me|my|we|our|you|your|he|she|they|them|his|her|their|what|which|who|do|does|did|have|has|had|not|no|yes|just|like|really|very|can|could|would|should|will|there|here|then|than|about|from|as|if|when|because|all|any|some|more|most|um|uh|yeah|okay|gonna|wanna|know|think|mean|right|get|got|go|going|one|thing|things|people|lot|kind|actually|hai|hain|ho|tha|thi|mein|main|ki|ke|ka|ko|se|ne|toh|bhi|hi|aur|ya|kya|kuch|nahi|na|haan|jo|ab|liye|saath|है|हैं|हो|था|थी|में|मैं|की|के|का|को|से|ने|पर|तो|भी|ही|और|या|क्या|कुछ|नहीं|ना|जो|अब|लिए|साथ|i'm|that's|it's|don't|you're|can't|didn't|there's|let's")
            .into_iter()
            .collect(),
    })
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || ('\u{0900}'..='\u{097F}').contains(&c) || ('\u{0300}'..='\u{036F}').contains(&c) || c == '\''
}

/// Lowercased, punctuation-free text padded with spaces so phrases match on word boundaries.
fn norm(s: &str) -> String {
    let cleaned: String =
        s.to_lowercase().replace('’', "'").chars().map(|c| if is_word_char(c) { c } else { ' ' }).collect();
    format!(" {} ", cleaned.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn count(n: &str, phrases: &[&str]) -> f64 {
    let mut c = 0;
    for p in phrases {
        let needle = format!(" {p} ");
        let mut from = 0;
        while let Some(i) = n[from..].find(&needle) {
            c += 1;
            from += i + needle.len() - 1; // the trailing space may start the next match
        }
    }
    c as f64
}

fn ends_sentence(text: &str) -> bool {
    let t = text.trim_end_matches(['"', '\'', '”', '’', ')']);
    t.ends_with(['.', '!', '?', '।', '॥', '…'])
}

struct Sentence<'a> {
    start: f64,
    end: f64,
    text: String,
    words: &'a [Word],
}

fn sentences(words: &[Word]) -> Vec<Sentence<'_>> {
    let mut out = Vec::new();
    let mut begin = 0;
    for i in 0..words.len() {
        let gap_before = i > begin && words[i].start - words[i - 1].end > SENTENCE_GAP_S;
        if gap_before {
            out.push(make_sentence(&words[begin..i]));
            begin = i;
        }
        if ends_sentence(&words[i].text) || i + 1 - begin >= MAX_SENTENCE_WORDS {
            out.push(make_sentence(&words[begin..=i]));
            begin = i + 1;
        }
    }
    if begin < words.len() {
        out.push(make_sentence(&words[begin..]));
    }
    out
}

fn make_sentence(words: &[Word]) -> Sentence<'_> {
    Sentence {
        start: words[0].start,
        end: words[words.len() - 1].end,
        text: words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
        words,
    }
}

/// Optional loudness envelope (linear RMS) sampled at `rate` values per second.
pub struct Loudness<'a> {
    pub rms: &'a [f32],
    pub rate: f64,
}

struct Features {
    is_q: bool,
    hook_hits: f64,
    emotion: f64,
    story: f64,
    insight: f64,
    debate: f64,
    laugh: f64,
    numbers: f64,
    starts_cont: bool,
    words: f64,
    complete: bool,
    terms: Vec<String>,
    rms: Option<(f64, f64, f64)>, // mean, std, sample count
    hook: f64,
}

fn features(s: &Sentence, loud: Option<&Loudness>) -> Features {
    let lex = lexicon();
    let n = norm(&s.text);
    let words = s.words.len().max(1) as f64;
    let first = n.split_whitespace().next().unwrap_or("");
    let is_q =
        s.text.trim_end().ends_with(['?', '？']) || lex.question.iter().any(|q| n.starts_with(&format!(" {q} ")));
    let numbers =
        count(&n, &lex.number) + s.text.split(|c: char| !c.is_ascii_digit()).filter(|t| !t.is_empty()).count() as f64;
    let terms = n
        .split_whitespace()
        .filter(|w| w.chars().count() > 2 && !lex.stop.contains(w) && !w.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string)
        .collect();
    let rms = loud.and_then(|l| {
        let a = ((s.start * l.rate).floor().max(0.0) as usize).min(l.rms.len());
        let b = ((s.end * l.rate).ceil() as usize).min(l.rms.len());
        (b > a).then(|| {
            let v = &l.rms[a..b];
            let mean = v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64;
            let var = v.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / v.len() as f64;
            (mean, var.sqrt(), v.len() as f64)
        })
    });
    let mut f = Features {
        is_q,
        hook_hits: count(&n, &lex.hook),
        emotion: count(&n, &lex.emotion),
        story: count(&n, &lex.story),
        insight: count(&n, &lex.insight),
        debate: count(&n, &lex.debate),
        laugh: count(&n, &lex.laugh) + s.text.matches(['!', '！']).count() as f64 * 0.5,
        numbers,
        starts_cont: lex.cont.contains(&first),
        words,
        complete: ends_sentence(s.text.trim()),
        terms,
        rms,
        hook: 0.0,
    };
    let fillers = count(&n, &lex.filler);
    let you = count(&n, &lex.you);
    let short_ref = words <= 6.0 && count(&n, &lex.anaphor) > 0.0;
    let mut hook = 0.0;
    if f.is_q {
        hook += 0.34;
    }
    if f.story > 0.0 {
        hook += 0.22;
    }
    if f.insight > 0.0 {
        hook += 0.16;
    }
    hook -= (fillers * 0.14).min(0.3);
    if short_ref {
        hook -= 0.22;
    }
    hook += (f.hook_hits * 0.12).min(0.3);
    if you > 0.0 {
        hook += 0.12;
    }
    if f.numbers > 0.0 {
        hook += 0.08;
    }
    if words <= 12.0 {
        hook += 0.1;
    } else if words > 30.0 {
        hook -= 0.12;
    }
    if f.starts_cont && !f.is_q {
        hook -= 0.3;
    }
    if f.emotion > 0.0 {
        hook += 0.06;
    }
    f.hook = hook.clamp(-0.4, 1.0);
    f
}

/// What the scorer is asked for.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct FindOptions {
    /// Clip length when `lengths` is empty, seconds.
    pub min_len: f64,
    pub max_len: f64,
    /// Accepted clip lengths `(min, max)` in seconds, any of them; empty = `min_len..max_len`.
    pub lengths: Vec<(f64, f64)>,
    /// `None` = let the scorer decide how many clips the video can carry.
    pub count: Option<usize>,
    /// Preferred part of the source `(start, end)`: a clip's middle must fall inside.
    pub within: Option<(f64, f64)>,
    /// "What to clip": only moments that mention it are kept.
    pub request: Option<String>,
}

impl Default for FindOptions {
    fn default() -> Self {
        Self { min_len: 20.0, max_len: 60.0, lengths: Vec::new(), count: None, within: None, request: None }
    }
}

/// Content words of a "what to clip" request, as prefixes so "pricing" also matches "price".
// ponytail: lexical match; the local-LLM planner replaces this when it lands.
fn request_terms(request: Option<&str>) -> Vec<String> {
    let lex = lexicon();
    request
        .map(norm)
        .unwrap_or_default()
        .split_whitespace()
        .filter(|w| w.chars().count() > 2 && !lex.stop.contains(w))
        .map(|w| w.chars().take(4.max(w.chars().count().saturating_sub(3))).collect())
        .collect()
}

fn request_hits(normalized: &str, terms: &[String]) -> f64 {
    normalized.split_whitespace().filter(|w| terms.iter().any(|t| w.starts_with(t.as_str()))).count() as f64
}

/// A proposed clip.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Candidate {
    pub start: f64,
    pub end: f64,
    /// 35–99, calibrated against the candidate pool.
    pub score: u32,
    pub rank: usize,
    pub title: String,
    pub reasons: Vec<String>,
    pub keywords: Vec<String>,
}

struct Window {
    i: usize,
    j: usize,
    start: f64,
    end: f64,
    raw: f64,
    ok: bool,
    parts: [f64; 7], // hook, emotion, energy, pace, complete, topic, laugh
    question: bool,
    story: f64,
    insight: f64,
    debate: f64,
    top: Vec<String>,
}

pub fn find(words: &[Word], opts: &FindOptions, loud: Option<&Loudness>) -> Vec<Candidate> {
    let lex = lexicon();
    let sents = sentences(words);
    if sents.is_empty() {
        return Vec::new();
    }
    let feats: Vec<Features> = sents.iter().map(|s| features(s, loud)).collect();
    let n_sent = sents.len() as f64;

    let mut df: HashMap<&str, f64> = HashMap::new();
    for f in &feats {
        for t in f.terms.iter().map(String::as_str).collect::<HashSet<_>>() {
            *df.entry(t).or_default() += 1.0;
        }
    }
    let idf = |t: &str| ((n_sent + 1.0) / (df.get(t).copied().unwrap_or(0.0) + 1.0)).ln();

    // Global loudness of speech for energy normalisation.
    let (gm, gs) = loud
        .map(|l| {
            let sp: Vec<f64> = l.rms.iter().map(|&v| v as f64).filter(|&v| v > 0.004).collect();
            if sp.is_empty() {
                return (0.0, 1.0);
            }
            let m = sp.iter().sum::<f64>() / sp.len() as f64;
            (m, (sp.iter().map(|v| (v - m).powi(2)).sum::<f64>() / sp.len() as f64).sqrt())
        })
        .unwrap_or((0.0, 0.0));
    let total_dur = (words[words.len() - 1].end - words[0].start).max(1.0);
    let median_wps = words.len() as f64 / total_dur;
    let sig = |x: f64| 1.0 / (1.0 + (-x).exp());
    let ranges = if opts.lengths.is_empty() { vec![(opts.min_len, opts.max_len)] } else { opts.lengths.clone() };
    let longest = ranges.iter().map(|r| r.1).fold(0.0, f64::max);
    let terms = request_terms(opts.request.as_deref());
    let req: Vec<f64> = sents.iter().map(|s| request_hits(&norm(&s.text), &terms)).collect();

    let mut cands: Vec<Window> = Vec::new();
    for i in 0..sents.len() {
        for j in i..sents.len() {
            let (s0, e1) = (sents[i].start, sents[j].end);
            let dur = e1 - s0;
            if dur > longest + 4.0 {
                break;
            }
            let Some(&(lo, hi)) = ranges.iter().find(|(lo, hi)| dur >= lo - 2.0 && dur <= hi + 4.0) else {
                continue;
            };
            let target = (lo + hi) / 2.0;
            if opts.within.is_some_and(|(a, b)| !(a..=b).contains(&((s0 + e1) / 2.0))) {
                continue;
            }
            let asked: f64 = req[i..=j].iter().sum();
            if !terms.is_empty() && asked == 0.0 {
                continue;
            }
            let (mut emo, mut story, mut ins, mut deb, mut laugh, mut wc, mut gaps) =
                (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
            let (mut rms_sum, mut rms_n, mut rms_var) = (0.0, 0.0, 0.0);
            let mut tf: HashMap<&str, f64> = HashMap::new();
            for k in i..=j {
                let f = &feats[k];
                emo += f.emotion;
                story += f.story;
                ins += f.insight;
                deb += f.debate;
                laugh += f.laugh;
                wc += f.words;
                if k > i && sents[k].start - sents[k - 1].end > 1.6 {
                    gaps += 1.0;
                }
                if let Some((m, sd, c)) = f.rms {
                    rms_sum += m * c;
                    rms_var += sd * c;
                    rms_n += c;
                }
                for t in &f.terms {
                    *tf.entry(t).or_default() += 1.0;
                }
            }
            let fi = &feats[i];
            let mut hook = fi.hook;
            if j > i && sents[i + 1].start - s0 < 6.0 {
                hook = hook.max(feats[i + 1].hook * 0.75 + if fi.starts_cont { -0.15 } else { 0.0 });
            }
            let emotion = (emo / wc * 12.0).clamp(0.0, 1.0);
            let (story_s, insight_s, debate_s) = ((story * 0.45).min(1.0), (ins * 0.35).min(1.0), (deb * 0.4).min(1.0));
            let laugh_s = (laugh * 0.35).min(1.0);
            let (mut energy, mut dynamics) = (0.5, 0.5);
            if rms_n > 0.0 && gs > 0.0 {
                energy = sig((rms_sum / rms_n - gm) / gs * 1.6);
                dynamics = sig(((rms_var / rms_n) / gs - 0.8) * 2.0);
            }
            let pace = (1.0 - ((wc / dur.max(1.0)) / median_wps.max(0.5) - 1.12).abs() * 1.4).clamp(0.0, 1.0);
            let last = &feats[j];
            let ends_clean =
                last.complete && sents.get(j + 1).is_none_or(|nx| nx.start - e1 > 0.45 || !feats[j + 1].starts_cont);
            let first_word = norm(&sents[i].text).split_whitespace().next().unwrap_or("").to_string();
            let starts_ref = !fi.is_q && lex.anaphor.contains(&first_word.as_str());
            let mut complete = if last.complete { 0.55 } else { 0.1 }
                + 0.45 * (1.0 - (dur - target).abs() / target.max(10.0)).clamp(0.0, 1.0);
            if last.words <= 12.0 && (last.emotion > 0.0 || last.numbers > 0.0 || last.hook_hits > 0.0) {
                complete += 0.12;
            }
            let mut scored: Vec<(&str, f64)> = tf.iter().map(|(t, c)| (*t, c * idf(t))).collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
            let topic = (scored.iter().take(5).map(|x| x.1).sum::<f64>() / (8.0 + wc * 0.12)).clamp(0.0, 1.0);
            let repeat_top = if scored.first().is_some_and(|x| tf[x.0] >= 3.0) { 0.1 } else { 0.0 };
            let raw = 0.3 * hook.clamp(0.0, 1.0)
                + 0.12 * emotion
                + 0.1 * energy
                + 0.05 * dynamics
                + 0.08 * pace
                + 0.15 * complete.clamp(0.0, 1.0)
                + 0.08 * (topic + repeat_top)
                + 0.07 * story_s.max(insight_s)
                + 0.05 * debate_s
                + 0.03 * laugh_s
                - 0.06 * gaps
                - if fi.starts_cont && !fi.is_q { 0.05 } else { 0.0 }
                + if ends_clean { 0.03 } else { -0.05 }
                - if starts_ref { 0.07 } else { 0.0 }
                + 0.15 * (asked / 3.0).min(1.0);
            cands.push(Window {
                i,
                j,
                start: s0,
                end: e1,
                raw,
                ok: ends_clean && (fi.is_q || !fi.starts_cont) && !starts_ref && hook >= 0.0,
                parts: [hook.clamp(0.0, 1.0), emotion, energy, pace, complete.clamp(0.0, 1.0), topic, laugh_s],
                question: fi.is_q || (j > i && feats[i + 1].is_q && sents[i + 1].start - s0 < 6.0),
                story: story_s,
                insight: insight_s,
                debate: debate_s,
                top: scored.iter().take(8).map(|x| x.0.to_string()).collect(),
            });
        }
    }
    if cands.is_empty() {
        return Vec::new();
    }
    cands.sort_by(|a, b| b.raw.total_cmp(&a.raw));

    // How many: only moments close to the best one, no two about the same thing,
    // never more than the video can carry (about one per 50 s, at most 20).
    let auto = opts.count.is_none();
    let best = cands[0].raw;
    let max_n = opts.count.unwrap_or_else(|| ((total_dur / 50.0).ceil() as usize + 1).clamp(1, 20));
    let floor = if auto { (best * 0.72).max(0.36) } else { f64::NEG_INFINITY };
    let jaccard = |a: &[String], b: &[String]| {
        let (a, b): (HashSet<_>, HashSet<_>) = (a.iter().collect(), b.iter().collect());
        a.intersection(&b).count() as f64 / (a.union(&b).count().max(1)) as f64
    };
    let mut picked: Vec<&Window> = Vec::new();
    for c in &cands {
        if picked.len() >= max_n || c.raw < floor {
            break;
        }
        if auto && !c.ok {
            continue;
        }
        let clash = picked.iter().any(|p| {
            let overlap = p.end.min(c.end) - p.start.max(c.start);
            overlap > 0.3 * (p.end - p.start).min(c.end - c.start) || (p.start - c.start).abs() < 3.0
        });
        if clash || (auto && picked.iter().any(|p| jaccard(&p.top, &c.top) > 0.45)) {
            continue;
        }
        picked.push(c);
    }
    if picked.is_empty() {
        picked.push(cands.iter().find(|c| c.ok).unwrap_or(&cands[0]));
    }

    let mut raws: Vec<f64> = cands.iter().map(|c| c.raw).collect();
    raws.sort_by(f64::total_cmp);
    let (p50, max) = (raws[raws.len() / 2], raws[raws.len() - 1]);
    picked
        .iter()
        .enumerate()
        .map(|(k, c)| {
            let last_word = sents[c.j].words.last().expect("sentence has words");
            let next_start = words.iter().find(|w| w.start >= last_word.end).map(|w| w.start);
            Candidate {
                start: (sents[c.i].words[0].start - 0.12).max(0.0),
                end: next_start.map_or(last_word.end + 0.5, |s| s - 0.03).min(last_word.end + 0.4),
                score: (62.0 + 37.0 * (c.raw - p50) / (max - p50).max(1e-6)).clamp(35.0, 99.0).round() as u32,
                rank: k + 1,
                title: title_for(&sents, &feats, c),
                reasons: reasons_for(c),
                keywords: c.top.iter().filter(|t| t.chars().count() > 3).take(4).cloned().collect(),
            }
        })
        .collect()
}

fn reasons_for(c: &Window) -> Vec<String> {
    let [hook, emotion, energy, pace, complete, topic, laugh] = c.parts;
    let mut r: Vec<(String, f64)> = Vec::new();
    if c.question {
        r.push(("Question hook".into(), hook));
    } else if hook > 0.4 {
        r.push(("Strong opener".into(), hook));
    }
    if emotion > 0.35 {
        r.push(("Emotional".into(), emotion));
    }
    if c.story > 0.3 {
        r.push(("Story".into(), c.story));
    }
    if c.insight > 0.3 {
        r.push(("Takeaway".into(), c.insight));
    }
    if c.debate > 0.35 {
        r.push(("Hot take".into(), c.debate));
    }
    if energy > 0.65 {
        r.push(("High energy".into(), energy));
    }
    if laugh > 0.6 {
        r.push(("Funny".into(), laugh));
    }
    if complete > 0.7 {
        r.push(("Complete thought".into(), complete * 0.6));
    }
    if pace > 0.75 {
        r.push(("Fast-paced".into(), pace * 0.5));
    }
    if let (true, Some(t)) = (topic > 0.5, c.top.first()) {
        r.push((format!("Topic: {t}"), topic * 0.5));
    }
    r.sort_by(|a, b| b.1.total_cmp(&a.1));
    r.into_iter().take(3).map(|x| x.0).collect()
}

fn title_for(sents: &[Sentence], feats: &[Features], c: &Window) -> String {
    let mut best = c.i;
    if c.j > c.i && feats[c.i + 1].hook - 0.1 > feats[c.i].hook {
        best = c.i + 1;
    }
    let text = sents[best].text.trim();
    let words: Vec<&str> = text.split_whitespace().collect();
    let max_w = if text.ends_with('?') { 12 } else { 10 };
    let mut t = if words.len() > max_w {
        let clause: Vec<&str> =
            text.split([',', ';', ':', '—', '–']).next().unwrap_or(text).split_whitespace().collect();
        let picked = if (4..=max_w).contains(&clause.len()) { clause } else { words[..max_w - 1].to_vec() };
        format!("{}…", picked.join(" "))
    } else {
        text.to_string()
    };
    let lead = list("and|so|but|because|okay|ok|yeah|um|uh|aur|toh|lekin|और|तो|लेकिन");
    loop {
        let first = t.split_whitespace().next().unwrap_or("").trim_end_matches(',').to_lowercase();
        if !lead.contains(&first.as_str()) || t.split_whitespace().count() < 2 {
            break;
        }
        t = t.split_once(char::is_whitespace).map(|x| x.1.trim_start().to_string()).unwrap_or_default();
    }
    let t = t.trim_end_matches(['.', ',', ';', ':']);
    let mut chars = t.chars();
    chars.next().map(|f| f.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(lines: &[&str], words_per_s: f64) -> Vec<Word> {
        let mut out = Vec::new();
        let mut t = 0.0;
        for line in lines {
            for w in line.split_whitespace() {
                out.push(Word { text: w.into(), start: t, end: t + 0.8 / words_per_s });
                t += 1.0 / words_per_s;
            }
            t += 0.6;
        }
        out
    }

    #[test]
    fn finds_hooky_moment_and_calibrates_score() {
        let filler = "and then we kind of just moved the file over to the other folder and it was fine.";
        let mut lines = vec![filler; 6];
        lines.extend([
            "Why does nobody tell you this about money?",
            "I remember the first time I lost a lakh rupees in one day.",
            "The lesson is simple, never trade what you do not understand!",
            "It was the worst mistake of my life and honestly I cried.",
        ]);
        lines.extend(vec![filler; 6]);
        let words = transcript(&lines, 2.5);
        let found = find(&words, &FindOptions { min_len: 15.0, max_len: 40.0, ..Default::default() }, None);
        assert!(!found.is_empty());
        let top = &found[0];
        assert!((35..=99).contains(&top.score));
        let hook_start = words.iter().find(|w| w.text == "Why").unwrap().start;
        assert!((top.start - hook_start).abs() < 1.0, "top clip should open on the question: {top:?}");
        assert!(top.reasons.iter().any(|r| r == "Question hook"));
        assert!(top.title.starts_with("Why"));
    }

    #[test]
    fn request_lengths_and_source_range_narrow_the_search() {
        let filler = "and then we kind of just moved the file over to the other folder and it was fine.";
        let mut lines = vec![filler; 8];
        lines.extend(["Why does nobody talk about the pricing of this plan?", "Prices went up twice this year!"]);
        lines.extend(vec![filler; 8]);
        let words = transcript(&lines, 2.5);
        let at = |w: &str| words.iter().find(|x| x.text == w).unwrap().start;

        let asked = FindOptions { request: Some("the price".into()), ..Default::default() };
        let found = find(&words, &asked, None);
        assert!(
            !found.is_empty() && found.iter().all(|c| c.start <= at("Prices") && c.end >= at("pricing")),
            "{found:?}"
        );
        let none = find(&words, &FindOptions { request: Some("dragons".into()), ..Default::default() }, None);
        assert!(none.is_empty(), "nothing mentions it: {none:?}");

        let short = FindOptions { lengths: vec![(10.0, 15.0)], count: Some(5), ..Default::default() };
        assert!(find(&words, &short, None).iter().all(|c| c.end - c.start <= 19.5));

        let early = FindOptions { within: Some((0.0, 30.0)), count: Some(5), ..Default::default() };
        assert!(find(&words, &early, None).iter().all(|c| (c.start + c.end) / 2.0 <= 30.5));
    }

    #[test]
    fn hindi_question_counts_as_hook() {
        let s = Sentence {
            start: 0.0, end: 3.0, text: "क्या आपको पता है सबसे बड़ी गलती क्या है?".into(), words: &[]
        };
        let f = features(&s, None);
        assert!(f.is_q && f.hook > 0.3, "hook = {}", f.hook);
    }

    #[test]
    fn fixed_count_is_respected_without_overlap() {
        let line = "What if I told you the secret to this build is one flag?";
        let words = transcript(&vec![line; 40], 2.5);
        let found =
            find(&words, &FindOptions { min_len: 15.0, max_len: 30.0, count: Some(3), ..Default::default() }, None);
        assert!(found.len() <= 3);
        for (a, b) in found.iter().zip(found.iter().skip(1)) {
            let overlap = a.end.min(b.end) - a.start.max(b.start);
            assert!(overlap <= 0.3 * (a.end - a.start).min(b.end - b.start));
        }
    }
}
