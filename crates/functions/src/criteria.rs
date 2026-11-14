//! Excel criteria (COUNTIF, SUMIFS, DCOUNT…) and wildcard matching.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use gridcraft_core::parse::parse_number_text;
use gridcraft_core::{Array, CellError, RangeRef, Value, compare_numbers, compare_text, number_key};

/// Longest text criterion COUNTIF and its relatives accept, in UTF-16 code units as Excel
/// counts them (😀 is two units); a longer one is `#VALUE!`.
const MAX_CRITERION_UTF16_UNITS: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug)]
enum Operand {
    Number(f64),
    Bool(bool),
    Error(CellError),
    /// Text (lower-cased chars kept for wildcard matching).
    Text(String),
    /// A criterion cut to its first 255 UTF-16 units in the middle of a surrogate pair: the
    /// complete characters before the cut, followed (in Excel) by a lone high surrogate.
    ///
    /// No cell text holds a lone surrogate, so `=` matches nothing and `<>` everything, as
    /// Excel gives 0 for SUMIFS with a criterion of 128 emoji. For `<`, `<=`, `>` and `>=` the
    /// criterion sorts just above the kept text: a cell matches `>`/`>=` when it sorts above
    /// that text, and `<`/`<=` otherwise. This is an approximation (Excel wasn't checked) and
    /// only matters for comparison criteria over 255 units.
    CutText(String),
    /// Nothing after the operator (`"="`, `"<>"`) or an empty criterion.
    Blank,
}

/// A parsed criterion.
///
/// Public (fields private) so the calculation host can pass one through to
/// [`CountCache`]: it only forwards the value, the matching stays here.
#[derive(Clone, Debug)]
pub struct Criterion {
    op: Op,
    operand: Operand,
    /// The criterion was a bare `""` (or empty cell text): matches blanks and empty strings.
    bare_empty: bool,
}

impl Criterion {
    /// Parses a criterion value the way COUNTIF does.
    pub fn parse(v: &Value) -> Criterion {
        match v {
            Value::Number(n) => Criterion { op: Op::Eq, operand: Operand::Number(*n), bare_empty: false },
            Value::Bool(b) => Criterion { op: Op::Eq, operand: Operand::Bool(*b), bare_empty: false },
            Value::Error(e) => Criterion { op: Op::Eq, operand: Operand::Error(*e), bare_empty: false },
            Value::Empty => Criterion { op: Op::Eq, operand: Operand::Number(0.0), bare_empty: false },
            Value::Array(a) => Criterion::parse(a.data.first().unwrap_or(&Value::Empty)),
            Value::Text(t) => Criterion::parse_text(t),
        }
    }

    /// Parses the criteria argument of COUNTIF, SUMIF, AVERAGEIF and the single-pair *IFS
    /// functions: like `parse`, but text longer than 255 UTF-16 code units is `#VALUE!`, as in
    /// Excel (checked on Excel 2016 and Microsoft 365: `"a"×254 & "😀"` is 255 characters but
    /// 256 units, and is `#VALUE!`).
    pub fn parse_arg(v: &Value) -> Result<Criterion, CellError> {
        match v {
            Value::Array(a) => Criterion::parse_arg(a.data.first().unwrap_or(&Value::Empty)),
            // A UTF-8 string has at least as many bytes as UTF-16 units, so short text skips the count.
            Value::Text(t) if t.len() > MAX_CRITERION_UTF16_UNITS && t.encode_utf16().nth(MAX_CRITERION_UTF16_UNITS).is_some() => {
                Err(CellError::Value)
            }
            other => Ok(Criterion::parse(other)),
        }
    }

    /// Parses a criteria argument of an *IFS function with two or more criteria pairs. Excel
    /// doesn't reject a longer text criterion there: it matches its first 255 UTF-16 code units
    /// (checked on Excel 2016 and Microsoft 365 with COUNTIFS, SUMIFS, AVERAGEIFS, MAXIFS and
    /// MINIFS). A cut through a surrogate pair matches nothing; see `Operand::CutText`.
    pub(crate) fn parse_arg_truncated(v: &Value) -> Criterion {
        match v {
            Value::Array(a) => Criterion::parse_arg_truncated(a.data.first().unwrap_or(&Value::Empty)),
            Value::Text(t) if t.len() > MAX_CRITERION_UTF16_UNITS => {
                let mut units = 0;
                for (i, c) in t.char_indices() {
                    if units + c.len_utf16() > MAX_CRITERION_UTF16_UNITS {
                        if units == MAX_CRITERION_UTF16_UNITS {
                            return Criterion::parse_text(&t[..i]);
                        }
                        // The cut falls between the two halves of `c`.
                        let (op, rest) = split_op(&t[..i]);
                        return Criterion { op, operand: Operand::CutText(rest.to_string()), bare_empty: false };
                    }
                    units += c.len_utf16();
                }
                Criterion::parse_text(t)
            }
            other => Criterion::parse(other),
        }
    }

    fn parse_text(t: &str) -> Criterion {
        if t.is_empty() {
            return Criterion { op: Op::Eq, operand: Operand::Blank, bare_empty: true };
        }
        let (op, rest) = split_op(t);
        let operand = if rest.is_empty() {
            Operand::Blank
        } else if let Some(n) = parse_number_text(rest) {
            Operand::Number(n)
        } else if rest.eq_ignore_ascii_case("TRUE") {
            Operand::Bool(true)
        } else if rest.eq_ignore_ascii_case("FALSE") {
            Operand::Bool(false)
        } else if let Some(e) = CellError::parse(rest) {
            Operand::Error(e)
        } else {
            Operand::Text(rest.to_string())
        };
        Criterion { op, operand, bare_empty: false }
    }

    /// Whether a cell value satisfies the criterion.
    pub(crate) fn matches(&self, v: &Value) -> bool {
        let v = match v {
            Value::Array(a) => a.data.first().cloned().unwrap_or(Value::Empty),
            other => other.clone(),
        };
        if self.bare_empty {
            return matches!(&v, Value::Empty) || matches!(&v, Value::Text(t) if t.is_empty());
        }
        match &self.operand {
            Operand::Blank => {
                let blank = matches!(v, Value::Empty);
                match self.op {
                    Op::Eq => blank,
                    Op::Ne => !blank,
                    // "<" / ">" with nothing: Excel compares against "" for text cells, but an
                    // empty string never matches ("<=" and ">=" count it in neither).
                    _ => match &v {
                        Value::Text(t) if !t.is_empty() => cmp_op(self.op, compare_text(t, "")),
                        _ => false,
                    },
                }
            }
            Operand::Number(n) => {
                let cell = match &v {
                    Value::Number(x) => Some(*x),
                    // Numeric text matches only for equality (COUNTIF("1") counts text "1").
                    Value::Text(t) if matches!(self.op, Op::Eq | Op::Ne) => parse_number_text(t),
                    _ => None,
                };
                match cell {
                    Some(x) => cmp_op(self.op, compare_numbers(x, *n)),
                    None => self.op == Op::Ne,
                }
            }
            Operand::Bool(b) => match &v {
                Value::Bool(x) => cmp_op(self.op, x.cmp(b)),
                _ => self.op == Op::Ne,
            },
            Operand::Error(e) => match &v {
                Value::Error(x) => cmp_op(self.op, x.code().cmp(&e.code())),
                _ => self.op == Op::Ne,
            },
            Operand::Text(pat) => match self.op {
                Op::Eq | Op::Ne => {
                    // Blank cells hold no text: `"*"` skips them and `"<>*"` counts them.
                    let cell_text = match &v {
                        Value::Text(t) => Some(t.to_string()),
                        _ => None,
                    };
                    let m = cell_text.is_some_and(|t| wildcard_match(pat, &t));
                    if self.op == Op::Eq { m } else { !m }
                }
                _ => match &v {
                    Value::Text(t) => cmp_op(self.op, compare_text(t, pat)),
                    _ => false,
                },
            },
            Operand::CutText(kept) => match self.op {
                Op::Eq => false,
                Op::Ne => true,
                Op::Lt | Op::Le => matches!(&v, Value::Text(t) if compare_text(t, kept) != Ordering::Greater),
                Op::Gt | Op::Ge => matches!(&v, Value::Text(t) if compare_text(t, kept) == Ordering::Greater),
            },
        }
    }
}

/// Frequency map of one range's values, for answering COUNTIF without scanning the range
/// on every call. Built once from the range's materialized values, it answers an
/// exact-equality criterion (`=` / `<>`, no wildcards) by lookup; anything else (an
/// ordering operator, a wildcard pattern, a comparison on a cut text) returns `None` so
/// the caller falls back to scanning, exactly as [`Criterion::matches`] would.
///
/// The map mirrors `matches` field for field: numbers keyed by [`number_key`] (15-digit
/// rounding, so `0.1+0.2` and `0.3` share a key), text cells keyed by their lower-cased
/// form (wildcard matching is case-insensitive), text that parses as a number counted
/// under its numeric value too (a Number criterion counts text `"5"`), NaN cells counted
/// apart (`compare_numbers` calls any NaN equal to any number), and blanks kept separate
/// (a Number criterion never counts them).
struct FreqMap {
    total: usize,
    numbers: HashMap<u64, usize>,
    num_text: HashMap<u64, usize>,
    texts: HashMap<String, usize>,
    bools: [usize; 2],
    errors: HashMap<u32, usize>,
    nan: usize,
    empty: usize,
}

impl FreqMap {
    /// Builds the map from a slice of values. `None` when a value can't be keyed (a nested
    /// array), so the caller falls back to scanning.
    fn build_values(values: &[Value]) -> Option<FreqMap> {
        let mut m = FreqMap {
            total: values.len(),
            numbers: HashMap::default(),
            num_text: HashMap::default(),
            texts: HashMap::default(),
            bools: [0, 0],
            errors: HashMap::default(),
            nan: 0,
            empty: 0,
        };
        for v in values {
            // `matches` reads an array value's first element; do the same, one level.
            let v = match v {
                Value::Array(a) => match a.data.first() {
                    Some(Value::Array(_)) | None => return None,
                    Some(first) => first,
                },
                other => other,
            };
            match v {
                Value::Number(n) => {
                    if n.is_nan() {
                        m.nan += 1;
                    } else {
                        *m.numbers.entry(number_key(*n)).or_insert(0) += 1;
                    }
                }
                Value::Text(t) => {
                    let key: String = lower_chars(t).into_iter().collect();
                    *m.texts.entry(key).or_insert(0) += 1;
                    if let Some(n) = parse_number_text(t) {
                        if n.is_nan() {
                            m.nan += 1;
                        } else {
                            *m.num_text.entry(number_key(n)).or_insert(0) += 1;
                        }
                    }
                }
                Value::Bool(b) => m.bools[*b as usize] += 1,
                Value::Error(e) => *m.errors.entry(e.code()).or_insert(0) += 1,
                Value::Empty => m.empty += 1,
                Value::Array(_) => return None,
            }
        }
        Some(m)
    }

    /// Exact-equality count for `criterion` (`None`: the criterion needs a scan).
    fn count(&self, c: &Criterion) -> Option<usize> {
        // A bare `""` matches blanks and empty strings, whatever the operator.
        if c.bare_empty {
            return Some(self.empty + self.texts.get("").copied().unwrap_or(0));
        }
        match c.op {
            Op::Eq | Op::Ne => {}
            _ => return None,
        }
        let eq = match &c.operand {
            Operand::Blank => self.empty,
            Operand::Number(n) => {
                if n.is_nan() {
                    self.numbers.values().sum::<usize>() + self.num_text.values().sum::<usize>() + self.nan
                } else {
                    let k = number_key(*n);
                    self.numbers.get(&k).copied().unwrap_or(0)
                        + self.num_text.get(&k).copied().unwrap_or(0)
                        + self.nan
                }
            }
            Operand::Bool(b) => self.bools[*b as usize],
            Operand::Error(e) => self.errors.get(&e.code()).copied().unwrap_or(0),
            Operand::Text(pat) => {
                if has_wildcards(pat) {
                    return None;
                }
                let key: String = lower_chars(pat).into_iter().collect();
                self.texts.get(&key).copied().unwrap_or(0)
            }
            Operand::CutText(_) => 0,
        };
        Some(match c.op {
            Op::Eq => eq,
            Op::Ne => self.total - eq,
            _ => return None,
        })
    }
}

/// Per-pass cache of range frequency maps, so COUNTIF over the same large range answers
/// from one O(n) build instead of one O(n) scan per call (a workbook with a hundred
/// thousand COUNTIFs over a hundred-thousand-row range otherwise does ~10^10 comparisons
/// on every recalculation). Lives on the evaluation host, one per recalculation round: a
/// range's values can't change mid-round, so a map built from the range's current values
/// stays right for the rest of the round. The host must not hand it a range whose values
/// can still change (a spill laid out later in the round, or an iterative recalculation).
#[derive(Default)]
pub struct CountCache {
    maps: HashMap<(usize, RangeRef), Rc<FreqMap>>,
}

impl CountCache {
    /// Count of cells in `range` (identified by `source`, the sheet + range it was read
    /// from) matching `criterion`. `None` when the criterion needs a scan.
    pub fn count(&mut self, source: (usize, RangeRef), range: &Array, criterion: &Criterion) -> Option<f64> {
        self.count_values(source, &range.data, criterion)
    }

    /// Count of the values in `values` (identified by `source`) matching `criterion`: the
    /// map is built from `values` on first use and reused afterwards, so callers that
    /// already hold the values pay one O(n) build per range instead of one per call.
    /// `None` when the criterion needs a scan (or the values can't be keyed).
    pub fn count_values(
        &mut self,
        source: (usize, RangeRef),
        values: &[Value],
        criterion: &Criterion,
    ) -> Option<f64> {
        let map = match self.maps.get(&source) {
            Some(m) => m.clone(),
            None => {
                let m = Rc::new(FreqMap::build_values(values)?);
                self.maps.insert(source, m.clone());
                m
            }
        };
        map.count(criterion).map(|n| n as f64)
    }

    /// Count from an already-built map only (`None` when no map exists for `source` or
    /// the criterion needs a scan): lets a caller skip reading the range entirely when
    /// the map is already there.
    pub fn get(&self, source: (usize, RangeRef), criterion: &Criterion) -> Option<f64> {
        self.maps.get(&source)?.count(criterion).map(|n| n as f64)
    }
}

/// Splits a leading comparison operator (`=` when there is none) off a text criterion.
fn split_op(t: &str) -> (Op, &str) {
    if let Some(r) = t.strip_prefix(">=") {
        (Op::Ge, r)
    } else if let Some(r) = t.strip_prefix("<=") {
        (Op::Le, r)
    } else if let Some(r) = t.strip_prefix("<>") {
        (Op::Ne, r)
    } else if let Some(r) = t.strip_prefix('=') {
        (Op::Eq, r)
    } else if let Some(r) = t.strip_prefix('>') {
        (Op::Gt, r)
    } else if let Some(r) = t.strip_prefix('<') {
        (Op::Lt, r)
    } else {
        (Op::Eq, t)
    }
}

fn cmp_op(op: Op, o: Ordering) -> bool {
    match op {
        Op::Eq => o == Ordering::Equal,
        Op::Ne => o != Ordering::Equal,
        Op::Lt => o == Ordering::Less,
        Op::Le => o != Ordering::Greater,
        Op::Gt => o == Ordering::Greater,
        Op::Ge => o != Ordering::Less,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    Char(char),
    Any,
    Star,
}

fn tokenize(pattern: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut it = pattern.chars();
    while let Some(c) = it.next() {
        match c {
            '~' => match it.next() {
                Some(n) => out.extend(std::iter::once(lower_char(n)).map(Tok::Char)),
                None => out.push(Tok::Char('~')),
            },
            '*' => out.push(Tok::Star),
            '?' => out.push(Tok::Any),
            c => out.extend(std::iter::once(lower_char(c)).map(Tok::Char)),
        }
    }
    out
}

/// Lowercase one char to exactly one char, so positions in the lowered text match the original
/// (U+0130 `İ` folds to `i`, like `fold` in text.rs).
fn lower_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn lower_chars(s: &str) -> Vec<char> {
    s.chars().map(lower_char).collect()
}

/// Whether the pattern contains an unescaped wildcard.
pub(crate) fn has_wildcards(pattern: &str) -> bool {
    tokenize(pattern).iter().any(|t| !matches!(t, Tok::Char(_))) || pattern.contains('~')
}

/// Case-insensitive whole-text match with `*`, `?` and `~` escapes.
pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p = tokenize(pattern);
    let t = lower_chars(text);
    match_from(&p, &t, true).is_some()
}

/// Greedy-free matcher: returns the matched length of `t` when `p` matches a prefix of `t`
/// (or all of `t` when `whole`). Iterative with backtracking on the last star (linear-ish).
fn match_from(p: &[Tok], t: &[char], whole: bool) -> Option<usize> {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    loop {
        if pi == p.len() && (!whole || ti == t.len()) {
            return Some(ti);
        }
        let step = match (p.get(pi), t.get(ti)) {
            (Some(Tok::Star), _) => {
                star = Some((pi, ti));
                pi += 1;
                continue;
            }
            (Some(Tok::Any), Some(_)) => true,
            (Some(Tok::Char(c)), Some(x)) => c == x,
            _ => false,
        };
        if step {
            pi += 1;
            ti += 1;
            continue;
        }
        match star {
            Some((sp, st)) if st < t.len() => {
                star = Some((sp, st + 1));
                pi = sp + 1;
                ti = st + 1;
            }
            _ => return None,
        }
    }
}

/// SEARCH-style find: the char index (0-based) of the first position at or after `start` where
/// the wildcard pattern matches, case-insensitively.
pub(crate) fn wildcard_find(pattern: &str, text: &str, start: usize) -> Option<usize> {
    let p = tokenize(pattern);
    let t = lower_chars(text);
    if p.is_empty() {
        return if start <= t.len() { Some(start) } else { None };
    }
    (start..t.len()).find(|&i| t.get(i..).is_some_and(|rest| match_from(&p, rest, false).is_some()))
}

/// Exact-or-wildcard equality used by MATCH/VLOOKUP/XLOOKUP exact modes: text with wildcards
/// matches by pattern (only when `wild`), otherwise values compare case-insensitively.
pub(crate) fn lookup_equal(needle: &Value, hay: &Value, wild: bool) -> bool {
    match (needle, hay) {
        (Value::Text(p), Value::Text(t)) => {
            if wild {
                wildcard_match(p, t)
            } else {
                compare_text(p, t) == Ordering::Equal
            }
        }
        (Value::Number(a), Value::Number(b)) => compare_numbers(*a, *b) == Ordering::Equal,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Error(a), Value::Error(b)) => a == b,
        (Value::Empty, Value::Empty) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(c: &str, v: Value) -> bool {
        Criterion::parse(&Value::from(c)).matches(&v)
    }

    #[test]
    fn criteria() {
        assert!(m(">5", Value::Number(6.0)));
        assert!(!m(">5", Value::Number(5.0)));
        assert!(m(">=5", Value::Number(5.0)));
        assert!(m("<>5", Value::from("x")));
        assert!(m("<>5", Value::Empty));
        assert!(!m(">5", Value::from("9")));
        assert!(m("5", Value::from("5")));
        assert!(m("a*", Value::from("Apple")));
        assert!(!m("a*", Value::from("banana")));
        assert!(m("?pple", Value::from("apple")));
        assert!(m("~*", Value::from("*")));
        assert!(!m("~*", Value::from("x")));
        assert!(m("", Value::Empty));
        assert!(m("", Value::from("")));
        assert!(!m("", Value::Number(0.0)));
        assert!(m("=", Value::Empty));
        assert!(!m("=", Value::from("a")));
        assert!(m("<>", Value::from("a")));
        assert!(!m("<>", Value::Empty));
        assert!(m("TRUE", Value::Bool(true)));
        // A bare "<=" or ">=" never matches an empty string (Excel 365: COUNTIF(="", "<=") is 0).
        assert!(!m("<=", Value::from("")));
        assert!(!m(">=", Value::from("")));
        assert!(m("<b", Value::from("apple")));
        assert!(!m("<b", Value::Number(1.0)));
        assert!(m("#N/A", Value::Error(CellError::NA)));
        assert!(Criterion::parse(&Value::Number(3.0)).matches(&Value::Number(3.0)));
        assert!(Criterion::parse(&Value::Number(0.3)).matches(&Value::Number(0.1 + 0.2)));
        assert!(lookup_equal(&Value::Number(0.3), &Value::Number(0.1 + 0.2), false));
        assert!(m("<>a*", Value::from("bcd")));
        assert!(m(">1/1/2020", Value::Number(44000.0)));
        // Text patterns never match blank cells, but an empty string is text.
        assert!(!m("*", Value::Empty));
        assert!(m("*", Value::from("")));
        assert!(m("<>*", Value::Empty));
        assert!(!m("<>*", Value::from("")));
        assert!(!m("?*", Value::Empty));
        assert!(m("<>a*", Value::Empty));
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("a*c", "abbbc"));
        assert!(!wildcard_match("a*c", "abbbd"));
        assert!(wildcard_match("*~?", "what?"));
        assert_eq!(wildcard_find("b?d", "abcd abd", 0), Some(1));
        assert_eq!(wildcard_find("b?d", "abxxd bcd", 0), Some(6));
        assert_eq!(wildcard_find("c*", "abcd", 0), Some(2));
        assert_eq!(wildcard_find("z", "abcd", 0), None);
        assert_eq!(wildcard_find("b", "a\u{130}bc", 0), Some(2));
        assert!(wildcard_match("i*", "\u{130}stanbul"));
        assert!(has_wildcards("a*"));
        assert!(!has_wildcards("abc"));
    }

    /// The frequency cache must agree with a plain scan for every criterion it answers,
    /// and decline (fall back to the scan) for operators and wildcard patterns.
    #[test]
    fn count_cache_matches_scan() {
        use gridcraft_core::{Array, CellRef, RangeRef};
        let values = vec![
            Value::Number(5.0),
            Value::Number(0.1 + 0.2),
            Value::Number(0.3),
            Value::from("5"),
            Value::from("Apple"),
            Value::from("apple"),
            Value::from(""),
            Value::Bool(true),
            Value::Bool(false),
            Value::Empty,
            Value::Error(CellError::NA),
            Value::Number(0.0),
            Value::Number(-0.0),
            Value::from("5.0"),
        ];
        let range = Array::new(values.len(), 1, values.clone()).unwrap();
        let source = (0, RangeRef::new(CellRef::new(0, 0), CellRef::new(values.len() as u32 - 1, 0)));
        // Criteria the cache must answer, with the scan as the oracle.
        for c in ["5", "0.3", "apple", "APPLE", "TRUE", "FALSE", "#N/A", "", "=", "<>", "<>5", "<>apple", "0"] {
            let crit = Criterion::parse_arg(&Value::from(c)).unwrap();
            let scan = range.iter().filter(|v| crit.matches(v)).count() as f64;
            let mut cache = CountCache::default();
            let got = cache.count(source, &range, &crit);
            assert_eq!(got, Some(scan), "criterion {c:?}: cache {got:?} vs scan {scan}");
        }
        // Operators and wildcard patterns need a scan.
        for c in [">5", ">=5", "<5", "a*", "?pple", "<>a*"] {
            let crit = Criterion::parse_arg(&Value::from(c)).unwrap();
            let mut cache = CountCache::default();
            assert_eq!(cache.count(source, &range, &crit), None, "criterion {c:?} should need a scan");
        }
        // Two ranges under different keys keep separate maps.
        let other = Array::new(3, 1, vec![Value::from("apple"), Value::from("apple"), Value::Number(1.0)]).unwrap();
        let other_source = (0, RangeRef::new(CellRef::new(0, 1), CellRef::new(2, 1)));
        let crit = Criterion::parse_arg(&Value::from("apple")).unwrap();
        let mut cache = CountCache::default();
        assert_eq!(cache.count(source, &range, &crit), Some(2.0));
        assert_eq!(cache.count(other_source, &other, &crit), Some(2.0));
        let crit5 = Criterion::parse_arg(&Value::from("5")).unwrap();
        assert_eq!(cache.count(source, &range, &crit5), Some(3.0));
        assert_eq!(cache.count(other_source, &other, &crit5), Some(0.0));
    }
}
