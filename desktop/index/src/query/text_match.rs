//! Word-level matching for search: fold text to comparable words, then score
//! how well each query term is answered by a field's words.

/// Best score a single term can earn against a single word.
pub(crate) const QUALITY_EXACT: i32 = 100;
const QUALITY_PREFIX: i32 = 90;
const QUALITY_INFIX: i32 = 55;
/// A script written without spaces has no word starts to prefer, so a match
/// inside a run of it is as good as a match gets.
const QUALITY_INFIX_UNSEGMENTED: i32 = 80;
const QUALITY_TYPO: i32 = 45;
const QUALITY_ABBREVIATION: i32 = 35;
/// Anything at or below this is a guess at what was meant, not a match of what
/// was typed.
pub(crate) const QUALITY_GUESS_CEILING: i32 = QUALITY_TYPO;

#[derive(Clone, Debug)]
pub(crate) struct SearchTerms {
    terms: Words,
    /// The query as typed, lowercased: the fallback for a query that has no
    /// words in it at all (`#`, `->`).
    literal: String,
}

impl SearchTerms {
    pub(crate) fn new(query: &str) -> Self {
        Self {
            terms: Words::of(query),
            literal: query.trim().to_lowercase(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.literal.is_empty()
    }

    /// Load `words` with the words of a field this query will be matched
    /// against. An empty query matches everything without looking, so it gets
    /// none.
    pub(crate) fn fill(&self, words: &mut Words, text: &str) {
        if self.terms.len() == 0 {
            words.clear();
        } else {
            words.fill(text);
        }
    }

    pub(crate) fn words_of(&self, text: &str) -> Words {
        let mut words = Words::default();
        self.fill(&mut words, text);
        words
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FieldMatch {
    /// 0..=1000 for the terms themselves, plus bonuses for phrase and position.
    pub(crate) score: i32,
    /// Every term was matched as typed (exact, prefix or inside a word).
    pub(crate) literal: bool,
}

/// Score `primary` against the query. Every term has to be answered by a word
/// of `primary` or, failing that, of one of the `context` fields, and at least
/// one by `primary` itself: context narrows a search ("work report"), it never
/// produces a hit on its own. `context` is only built when a term needs it.
pub(crate) fn match_fields<const N: usize>(
    query: &SearchTerms,
    primary_text: &str,
    primary: &Words,
    context: impl FnOnce() -> [Words; N],
) -> Option<FieldMatch> {
    if query.is_empty() {
        return Some(FieldMatch {
            score: 0,
            literal: true,
        });
    }
    if query.terms.len() == 0 {
        return primary_text
            .to_lowercase()
            .contains(&query.literal)
            .then_some(FieldMatch {
                score: 500,
                literal: true,
            });
    }

    // Tracked as running facts about the matched positions rather than a list
    // of them: this runs once per line of the workspace, and most lines match
    // nothing.
    let mut quality_sum = 0;
    let mut weakest = QUALITY_EXACT;
    let mut answered = 0;
    let mut first = 0;
    let mut previous: Option<usize> = None;
    let mut adjacent = true;
    let mut ordered = true;
    for term in query.terms.iter() {
        let Some((ix, quality)) = best_word(term, primary, previous.map(|ix| ix + 1)) else {
            continue;
        };
        quality_sum += quality;
        weakest = weakest.min(quality);
        match previous {
            Some(previous) => {
                adjacent &= ix == previous + 1;
                ordered &= ix > previous;
            }
            None => first = ix,
        }
        previous = Some(ix);
        answered += 1;
    }
    if answered == 0 {
        return None;
    }
    let term_count = query.terms.len();
    if answered < term_count {
        let context = context();
        for term in query.terms.iter() {
            if best_word(term, primary, None).is_some() {
                continue;
            }
            let quality = context
                .iter()
                .filter_map(|words| best_word(term, words, None))
                .map(|(_, quality)| quality)
                .max()?;
            quality_sum += quality / 2;
            weakest = weakest.min(quality);
        }
    }

    let mut score = quality_sum * 10 / term_count as i32;
    if answered == term_count && term_count > 1 {
        if adjacent {
            score += 150;
        } else if ordered {
            score += 60;
        }
    }
    if answered == primary.len() && weakest == QUALITY_EXACT {
        score += 200;
    }
    if first == 0 {
        score += 60;
    }
    score -= first.min(10) as i32 * 4;
    score -= primary.len().min(30) as i32 * 2;

    Some(FieldMatch {
        score,
        literal: weakest > QUALITY_GUESS_CEILING,
    })
}

/// The word that answers `term` best, earliest first; on a tie the word at
/// `prefer` wins, so a phrase typed in order is matched in order.
fn best_word(term: &str, words: &Words, prefer: Option<usize>) -> Option<(usize, i32)> {
    let mut best: Option<(usize, i32)> = None;
    for (ix, word) in words.iter().enumerate() {
        let quality = word_quality(term, word);
        if quality == 0 {
            continue;
        }
        let better = best.is_none_or(|(_, best_quality)| {
            quality > best_quality || (quality == best_quality && prefer == Some(ix))
        });
        if better {
            best = Some((ix, quality));
        }
    }
    best
}

fn word_quality(term: &str, word: &str) -> i32 {
    if word == term {
        return QUALITY_EXACT;
    }
    let term_len = term.chars().count();
    let word_len = word.chars().count();
    if word.starts_with(term) {
        return QUALITY_PREFIX - (word_len - term_len).min(10) as i32;
    }
    if term_len >= 2 && word.contains(term) {
        return if term.is_ascii() {
            QUALITY_INFIX
        } else {
            QUALITY_INFIX_UNSEGMENTED
        };
    }
    // Past here the term is not in the word as typed. Both guesses below insist
    // on the first letter, which people rarely get wrong and which keeps short
    // words from matching each other by accident.
    if term_len < 3 || term.chars().next() != word.chars().next() {
        return 0;
    }
    let allowed = match term_len {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    };
    if allowed > 0 && term_len <= MAX_TYPO_WORD_CHARS {
        // Still typing: compare against as much of the word as has been typed.
        // Four letters are too few for that — most words sharing three of
        // their first four would match.
        let compared_len = if term_len >= 5 && word_len > term_len + allowed {
            term_len
        } else {
            word_len
        };
        if compared_len <= MAX_TYPO_WORD_CHARS {
            let mut term_chars = ['\0'; MAX_TYPO_WORD_CHARS];
            let mut word_chars = ['\0'; MAX_TYPO_WORD_CHARS];
            for (slot, ch) in term_chars.iter_mut().zip(term.chars()) {
                *slot = ch;
            }
            for (slot, ch) in word_chars.iter_mut().zip(word.chars()) {
                *slot = ch;
            }
            if within_edit_distance(
                &term_chars[..term_len],
                &word_chars[..compared_len],
                allowed,
            ) {
                return QUALITY_TYPO;
            }
        }
    }
    if term_len * 5 >= word_len * 3 && is_subsequence(term, word) {
        return QUALITY_ABBREVIATION;
    }
    0
}

/// Words longer than this are matched as typed or not at all, which lets the
/// typo check below work on the stack.
const MAX_TYPO_WORD_CHARS: usize = 32;

/// Optimal-string-alignment distance (insert, delete, substitute, swap two
/// neighbours), answered only as "no more than `limit`".
fn within_edit_distance(left: &[char], right: &[char], limit: usize) -> bool {
    if left.len().abs_diff(right.len()) > limit {
        return false;
    }
    let mut before_previous = [0usize; MAX_TYPO_WORD_CHARS + 1];
    let mut previous: [usize; MAX_TYPO_WORD_CHARS + 1] = std::array::from_fn(|ix| ix);
    let mut current = [0usize; MAX_TYPO_WORD_CHARS + 1];
    for i in 1..=left.len() {
        current[0] = i;
        let mut row_min = current[0];
        for j in 1..=right.len() {
            let cost = usize::from(left[i - 1] != right[j - 1]);
            let mut value = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && left[i - 1] == right[j - 2] && left[i - 2] == right[j - 1] {
                value = value.min(before_previous[j - 2] + 1);
            }
            current[j] = value;
            row_min = row_min.min(value);
        }
        if row_min > limit {
            return false;
        }
        std::mem::swap(&mut before_previous, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()] <= limit
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut haystack = haystack.chars();
    needle.chars().all(|ch| haystack.any(|other| other == ch))
}

/// A field split into words, lowercased and with accents removed, so `cafe`
/// finds `Café` and `uber` finds `Über`. One buffer holds every word, and
/// [`Words::fill`] reuses it, so scanning a workspace allocates per query
/// rather than per line.
#[derive(Clone, Debug, Default)]
pub(crate) struct Words {
    text: String,
    ends: Vec<usize>,
}

impl Words {
    pub(crate) fn of(text: &str) -> Self {
        let mut words = Self::default();
        words.fill(text);
        words
    }

    pub(crate) fn fill(&mut self, text: &str) {
        self.clear();
        let mut word_start = 0;
        for ch in text.chars() {
            if is_combining_mark(ch) {
                continue;
            }
            if ch.is_alphanumeric() {
                for lower in ch.to_lowercase() {
                    push_folded(&mut self.text, lower);
                }
            } else if self.text.len() > word_start {
                word_start = self.text.len();
                self.ends.push(word_start);
            }
        }
        if self.text.len() > word_start {
            self.ends.push(self.text.len());
        }
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.ends.clear();
    }

    fn len(&self) -> usize {
        self.ends.len()
    }

    fn iter(&self) -> impl Iterator<Item = &str> {
        let mut start = 0;
        self.ends.iter().map(move |&end| {
            let word = &self.text[start..end];
            start = end;
            word
        })
    }
}

fn is_combining_mark(ch: char) -> bool {
    ('\u{0300}'..='\u{036f}').contains(&ch)
}

fn push_folded(out: &mut String, ch: char) {
    let folded = match ch {
        'à'..='å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' | 'ŧ' => 't',
        'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        'æ' => {
            out.push_str("ae");
            return;
        }
        'œ' => {
            out.push_str("oe");
            return;
        }
        'ß' => {
            out.push_str("ss");
            return;
        }
        other => other,
    };
    out.push(folded);
}
