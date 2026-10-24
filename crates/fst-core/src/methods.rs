use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Plain,
    Ordered,
    Unordered,
    Functional,
    FunctionalOrdered,
    FunctionalUnordered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
    /// The token without the leading `#`.
    pub text: String,
    pub kind: MethodKind,
}

const SEG: &str = r"[\p{L}\p{N}_]+(?:-[\p{L}\p{N}_]+)*";

/// Stands in for an inline code span. Non-whitespace and not a valid
/// segment character, so it can never form or join a method token.
const CODE_PLACEHOLDER: char = '\u{FFFC}';

fn mask_inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(s) = rest.find('`') {
        let after = &rest[s + 1..];
        match after.find('`') {
            Some(e) => {
                out.push_str(&rest[..s]);
                out.push(CODE_PLACEHOLDER);
                rest = &after[e + 1..];
            }
            None => break, // unclosed backtick stays literal
        }
    }
    out.push_str(rest);
    out
}

/// Removes fenced blocks (kept as empty lines, so line structure survives)
/// and single-backtick inline spans.
fn mask_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let after_fence = if indent <= 3 {
            line[indent..].strip_prefix("```")
        } else {
            None
        };
        let ends_line = line.ends_with('\n');

        if in_fence {
            // A closing fence carries nothing after the backticks.
            if after_fence.is_some_and(|r| r.trim().is_empty()) {
                in_fence = false;
            }
            if ends_line {
                out.push('\n');
            }
        } else if after_fence.is_some_and(|r| !r.contains('`')) {
            in_fence = true;
            if ends_line {
                out.push('\n');
            }
        } else {
            out.push_str(&mask_inline(line));
        }
    }
    out
}

fn candidate_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:^|\s)#(\S+)").unwrap())
}

fn grammar_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"^(?:(?P<fo>f/{s}(?:/{s})*)|(?P<fu>f/{s}(?:--{s})+)|(?P<o>{s}(?:/{s})+)|(?P<u>{s}(?:--{s})+)|(?P<p>{s}))$",
            s = SEG
        ))
        .unwrap()
    })
}

fn classify(token: &str) -> Option<MethodKind> {
    let c = grammar_re().captures(token)?;
    Some(if c.name("fo").is_some() {
        if token.matches('/').count() == 1 {
            MethodKind::Functional
        } else {
            MethodKind::FunctionalOrdered
        }
    } else if c.name("fu").is_some() {
        MethodKind::FunctionalUnordered
    } else if c.name("o").is_some() {
        MethodKind::Ordered
    } else if c.name("u").is_some() {
        MethodKind::Unordered
    } else {
        MethodKind::Plain
    })
}

/// Unique methods in order of first appearance.
pub fn extract_methods(text: &str) -> Vec<Method> {
    let masked = mask_code(text);
    let mut out: Vec<Method> = Vec::new();
    for cap in candidate_re().captures_iter(&masked) {
        let raw = cap.get(1).unwrap().as_str();
        let token = raw.trim_end_matches(|c: char| {
            matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '\'' | '"')
        });
        if let Some(kind) = classify(token) {
            if !out.iter().any(|m| m.text == token) {
                out.push(Method { text: token.to_string(), kind });
            }
        }
    }
    out
}
