//! The small glob dialect of include and exclude patterns.
//!
//! A pattern is matched against the path of an entry relative to the rule's
//! root, written with `/` between components, ignoring case (Windows file
//! names are case-insensitive). It supports exactly:
//!
//! - `*` any run of characters inside one component (possibly none),
//! - `?` exactly one character inside one component,
//! - `**` as a whole component: any number of components, including none.
//!
//! Everything else is a literal, and anything that could be mistaken for a
//! richer syntax (`[`, `{`, ...) or that could point outside the root (`..`,
//! a leading `/`, a drive letter, `\`) is rejected when the rule is loaded.

use std::fmt;

/// Longest accepted pattern, in characters.
const MAX_PATTERN_LEN: usize = 260;

/// Why a pattern was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PatternError {
    /// It could reach outside the root or names an absolute location.
    Unsafe(String),
    /// It is not well formed.
    Malformed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Literal(char),
    /// `?`
    One,
    /// `*`
    Run,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// `**`
    AnyDepth,
    Name(Vec<Token>),
}

/// A validated include or exclude pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    source: String,
    segments: Vec<Segment>,
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

impl Pattern {
    pub(super) fn parse(text: &str) -> Result<Self, PatternError> {
        use PatternError::{Malformed, Unsafe};
        if text.trim().is_empty() {
            return Err(Malformed("must not be empty".to_owned()));
        }
        if text.chars().count() > MAX_PATTERN_LEN {
            return Err(Malformed(format!(
                "must be at most {MAX_PATTERN_LEN} characters"
            )));
        }
        if text.chars().any(char::is_control) {
            return Err(Malformed("must not contain control characters".to_owned()));
        }
        if text.contains('\\') {
            return Err(Malformed("use `/` between components, not `\\`".to_owned()));
        }
        if text.starts_with('/') || text.contains(':') {
            return Err(Unsafe(
                "must be relative to the rule root: no leading `/`, no drive letter".to_owned(),
            ));
        }
        if let Some(bad) = text.chars().find(|c| "[]{}<>|\"".contains(*c)) {
            return Err(Malformed(format!(
                "`{bad}` is not supported; only `*`, `?` and `**` are special"
            )));
        }

        let mut segments = Vec::new();
        for part in text.split('/') {
            match part {
                "" => {
                    return Err(Malformed(
                        "has an empty component (a doubled or trailing `/`)".to_owned(),
                    ));
                }
                "." => {
                    return Err(Malformed("`.` components are not allowed".to_owned()));
                }
                ".." => {
                    return Err(Unsafe(
                        "`..` would leave the rule root and is not allowed".to_owned(),
                    ));
                }
                "**" => segments.push(Segment::AnyDepth),
                other if other.contains("**") => {
                    return Err(Malformed(
                        "`**` must be a whole component, such as `a/**/b`".to_owned(),
                    ));
                }
                other => segments.push(Segment::Name(
                    other
                        .chars()
                        .map(|c| match c {
                            '*' => Token::Run,
                            '?' => Token::One,
                            other => Token::Literal(fold(other)),
                        })
                        .collect(),
                )),
            }
        }
        Ok(Self {
            source: text.to_owned(),
            segments,
        })
    }

    /// The pattern as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether the pattern matches the whole relative path given as its
    /// components (no separators inside a component).
    #[must_use]
    pub fn matches<S: AsRef<str>>(&self, components: &[S]) -> bool {
        // fits[i][j]: segments[i..] match components[j..].
        let (s, c) = (self.segments.len(), components.len());
        let mut fits = vec![vec![false; c + 1]; s + 1];
        fits[s][c] = true;
        for i in (0..s).rev() {
            for j in (0..=c).rev() {
                fits[i][j] = match &self.segments[i] {
                    Segment::AnyDepth => fits[i + 1][j] || (j < c && fits[i][j + 1]),
                    Segment::Name(tokens) => {
                        j < c && fits[i + 1][j + 1] && name_matches(tokens, components[j].as_ref())
                    }
                };
            }
        }
        fits[0][0]
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

/// Wildcard match of one component, linear in the common case and
/// `O(tokens * name)` at worst.
fn name_matches(tokens: &[Token], name: &str) -> bool {
    let name: Vec<char> = name.chars().map(fold).collect();
    let (mut t, mut n) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while n < name.len() {
        match tokens.get(t) {
            Some(Token::Run) => {
                backtrack = Some((t, n));
                t += 1;
            }
            Some(Token::One) => {
                t += 1;
                n += 1;
            }
            Some(Token::Literal(c)) if *c == name[n] => {
                t += 1;
                n += 1;
            }
            _ => match backtrack {
                Some((star, matched)) => {
                    backtrack = Some((star, matched + 1));
                    t = star + 1;
                    n = matched + 1;
                }
                None => return false,
            },
        }
    }
    tokens[t..].iter().all(|token| *token == Token::Run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').collect();
        Pattern::parse(pattern)
            .expect("valid pattern")
            .matches(&parts)
    }

    #[test]
    fn star_and_question_mark_stay_inside_one_component() {
        assert!(m("*.tmp", "a.tmp"));
        assert!(m("*.tmp", ".tmp"));
        assert!(!m("*.tmp", "dir/a.tmp"));
        assert!(m("a?c", "abc"));
        assert!(!m("a?c", "ac"));
        assert!(m("*a*b*", "xxaxxbxx"));
        assert!(!m("*a*b*", "xxbxxaxx"));
        assert!(m("*", "anything"));
    }

    #[test]
    fn double_star_matches_any_number_of_components() {
        assert!(m("**", "a"));
        assert!(m("**", "a/b/c"));
        assert!(m("cache/**", "cache/a"));
        assert!(m("cache/**", "cache/a/b"));
        assert!(m("cache/**", "cache"));
        assert!(!m("cache/**", "other/a"));
        assert!(m("**/*.log", "x.log"));
        assert!(m("**/*.log", "a/b/x.log"));
        assert!(!m("**/*.log", "a/b/x.txt"));
        assert!(m("a/**/z", "a/z"));
        assert!(m("a/**/z", "a/b/c/z"));
    }

    #[test]
    fn matching_ignores_case() {
        assert!(m("*.TMP", "readme.tmp"));
        assert!(m("Cache/**", "CACHE/x"));
    }

    #[test]
    fn dangerous_or_unsupported_patterns_are_refused() {
        for bad in [
            "../x", "a/../b", "/abs", "C:/x", "C:x", "a\\b", "a//b", "a/", "", "  ", "./a", "a**",
            "[ab]", "{a,b}", "a|b", "x\u{0}y",
        ] {
            assert!(Pattern::parse(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(matches!(
            Pattern::parse("../x"),
            Err(PatternError::Unsafe(_))
        ));
        assert!(matches!(Pattern::parse("/x"), Err(PatternError::Unsafe(_))));
        assert!(matches!(
            Pattern::parse("a**"),
            Err(PatternError::Malformed(_))
        ));
    }
}
