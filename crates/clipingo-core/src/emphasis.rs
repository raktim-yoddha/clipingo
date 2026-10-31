//! How strongly a word lands: numbers and money, power words, shouting, exclamations, long words. Smart zoom picks
//! its moments with it and caption styles colour the strong words. English, Hindi and Hinglish; lexicon is our own.

use std::collections::HashSet;
use std::sync::OnceLock;

fn set(s: &'static str) -> HashSet<&'static str> {
    s.split_whitespace().collect()
}

struct Words {
    stop: HashSet<&'static str>,
    power: HashSet<&'static str>,
    soft: HashSet<&'static str>,
    numbers: HashSet<&'static str>,
}

fn words() -> &'static Words {
    static W: OnceLock<Words> = OnceLock::new();
    W.get_or_init(|| Words {
        stop: set("a an the and or but so to of in on at for with from by as is are was were be been it its this that \
                   these those i i'm you you're your he she we they my me him her us them our their do does did have \
                   has had will would can could should just like really very okay ok yeah yes um uh well then than \
                   there here when where which who hai hain tha thi ka ki ke ko se mein aur ya toh bhi ye yeh wo woh \
                   है हैं था थी का की के को से में पर और या तो भी ये यह वो वह"),
        power: set("never always nobody everyone everything nothing forever million billion thousand money rich poor \
                   broke secret truth lie wrong mistake fail failed failure win won lose lost love hate fear scared \
                   death dead die kill free must important problem believe shocking insane crazy impossible huge \
                   massive amazing incredible dangerous risk quit dream fight war best worst biggest greatest stop \
                   kabhi nahi sach paisa lakh crore bilkul sabse zindagi galti pagal kamaal khatarnak zaroor sapna \
                   mehnat dhokha maut pyaar asli sirf hamesha \
                   कभी नहीं सच पैसा लाख करोड़ बिल्कुल सबसे ज़िंदगी गलती पागल कमाल खतरनाक ज़रूर सपना मेहनत धोखा मौत प्यार असली सिर्फ़ हमेशा"),
        soft: set("why how what first last now today only real need hard easy simple change life kya kyun kaise क्या क्यों कैसे"),
        numbers: set("two three four five six seven eight nine ten twenty thirty forty fifty hundred thousand million \
                   billion do teen char paanch das sau hazaar दो तीन चार पांच दस सौ हज़ार"),
    })
}

/// 0 for filler, about 1 for a strong word, more for a stronger one.
pub fn strength(raw: &str) -> f64 {
    let core: String = raw.chars().filter(|c| c.is_alphanumeric() || "$₹%'’".contains(*c)).collect();
    let low = core.to_lowercase().replace('’', "'");
    let w = words();
    let mut s = 0.0;
    if core.chars().any(|c| c.is_ascii_digit()) || w.numbers.contains(low.as_str()) {
        s += 1.2;
    }
    if core.contains(['$', '₹', '%']) {
        s += 1.0;
    }
    if s == 0.0 && w.stop.contains(low.as_str()) {
        return 0.0;
    }
    if w.power.contains(low.trim_end_matches("'s").trim_end_matches('s')) || w.power.contains(low.as_str()) {
        s += 1.0;
    } else if w.soft.contains(low.as_str()) {
        s += 0.55;
    }
    let tail = raw.trim_end_matches(['"', '\'', ')', '”', '’']);
    if tail.ends_with('!') {
        s += 1.0;
    } else if tail.ends_with('?') {
        s += 0.3;
    }
    let letters: String = core.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.chars().count() >= 2 && letters == letters.to_uppercase() && letters != letters.to_lowercase() {
        s += 0.8;
    }
    if letters.chars().count() >= 8 {
        s += 0.3;
    }
    s
}

/// A word worth colouring in a caption.
pub fn is_strong(raw: &str) -> bool {
    strength(raw) >= 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_what_lands() {
        assert_eq!(strength("the"), 0.0);
        assert!(is_strong("million"), "power word");
        assert!(is_strong("$100"), "money");
        assert!(is_strong("Never!") && strength("Never!") > strength("never"), "shouted");
        assert!(is_strong("पैसा") && is_strong("paisa"), "Hindi and Hinglish");
        assert!(!is_strong("folder"));
        assert!(strength("WOW") >= 0.8, "capitals");
    }
}
