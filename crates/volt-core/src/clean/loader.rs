//! Loading and validating cleaning rules from TOML text.
//!
//! Same contract as the tweak catalogue: the core never reads files, the whole
//! text is rejected when anything is wrong, and the error lists every problem
//! with the rule id and the path of the field.

use std::collections::HashMap;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use super::folders::Location;
use super::glob::{Pattern, PatternError};
use super::rule::{CleanCategory, CleaningRule, Constraints, MAX_DEPTH_LIMIT, RuleRoot};
use crate::label::Risk;

const RULE_KEYS: &[&str] = &[
    "id",
    "title",
    "description",
    "category",
    "risk",
    "root",
    "include",
    "exclude",
    "constraints",
    "requires_admin",
    "min_age_setting",
    "locked_by",
];
const MAX_MIN_AGE_DAYS: u32 = 3650;
const MAX_RELATIVE_COMPONENTS: usize = 16;
const MAX_RELATIVE_LEN: usize = 260;

/// What is wrong with a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RuleIssueKind {
    /// The text is not valid TOML. `line` is 1-based when known.
    Syntax {
        /// The parser's message.
        message: String,
        /// Where it went wrong.
        line: Option<usize>,
    },
    /// A required field is absent.
    MissingField,
    /// The field is not part of the format.
    UnknownField,
    /// The value is not one of the allowed names (the message lists them).
    UnknownValue(String),
    /// The value has the wrong type or is out of range.
    Invalid(String),
    /// The value is empty (or only white space).
    Empty,
    /// The value is not well formed; the text says why.
    Malformed(String),
    /// Another rule already uses this id.
    DuplicateId {
        /// 1-based position of the first `[[rule]]` table with this id.
        first_position: usize,
    },
    /// The root could lead outside what a rule may touch: an absolute path, a
    /// drive, a `..` component, or a whole location folder.
    UnsafeRoot(String),
    /// A pattern could match outside the rule's root.
    UnsafePattern(String),
}

impl fmt::Display for RuleIssueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax {
                message,
                line: Some(line),
            } => write!(f, "invalid TOML at line {line}: {message}"),
            Self::Syntax {
                message,
                line: None,
            } => write!(f, "invalid TOML: {message}"),
            Self::MissingField => f.write_str("required field is missing"),
            Self::UnknownField => f.write_str("unknown field"),
            Self::UnknownValue(message)
            | Self::Invalid(message)
            | Self::Malformed(message)
            | Self::UnsafeRoot(message)
            | Self::UnsafePattern(message) => f.write_str(message),
            Self::Empty => f.write_str("must not be empty"),
            Self::DuplicateId { first_position } => {
                write!(f, "duplicate id, already used by rule #{first_position}")
            }
        }
    }
}

/// One problem found while loading cleaning rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleIssue {
    /// 1-based position of the `[[rule]]` table in the file; `None` for a
    /// problem with the file as a whole.
    pub position: Option<usize>,
    /// The rule's id, when it could be read.
    pub rule: Option<String>,
    /// Path of the field, such as `root.path` or `constraints.max_depth`.
    pub field: String,
    /// What is wrong with it.
    pub kind: RuleIssueKind,
}

impl fmt::Display for RuleIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.rule, self.position) {
            (Some(id), Some(position)) => write!(f, "rule \"{id}\" (#{position}): ")?,
            (Some(id), None) => write!(f, "rule \"{id}\": ")?,
            (None, Some(position)) => write!(f, "rule #{position}: ")?,
            (None, None) => {}
        }
        if self.field.is_empty() {
            write!(f, "{}", self.kind)
        } else {
            write!(f, "field `{}`: {}", self.field, self.kind)
        }
    }
}

/// A set of rules was rejected as a whole. Nothing from it was loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesError {
    /// Every problem found, in file order. Never empty.
    pub issues: Vec<RuleIssue>,
}

impl fmt::Display for RulesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cleaning rules rejected: {} problem(s)",
            self.issues.len()
        )?;
        for issue in &self.issues {
            write!(f, "\n  - {issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RulesError {}

/// A validated set of cleaning rules with unique ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleaningRules {
    rules: Vec<CleaningRule>,
}

impl CleaningRules {
    /// Loads rules from TOML text: an array of `[[rule]]` tables.
    ///
    /// ```toml
    /// [[rule]]
    /// id = "user-temp-files"            # lower-case letters, digits, single hyphens
    /// title = "Temporary files"
    /// description = "Files programs left in the temp folder and no longer use."
    /// category = "temporary_files"      # temporary_files, application_cache, browser_cache,
    ///                                   # shader_cache, chat_app_cache, developer_cache, logs, crash_reports, windows_update
    /// risk = "safe"                     # safe, caution, advanced
    /// include = ["**"]                  # files to take, relative to the root, `/` separated
    /// exclude = ["*.lock", "keep/**"]   # optional: left alone (a matching folder is not entered)
    ///
    /// [rule.root]
    /// location = "user_temp"            # user_temp, local_app_data, roaming_app_data,
    ///                                   # program_data, windows, user_profile
    /// path = ""                         # relative path below the location; empty only for user_temp
    ///
    /// [rule.constraints]
    /// min_age_days = 2                  # required; 0 = no minimum
    /// max_depth = 8                     # required; 0 = only entries directly in the root, at most 32
    /// remove_empty_dirs = true          # optional, default false
    /// ```
    ///
    /// Two optional top-level fields describe the rule's environment:
    /// `requires_admin = true` says the location can only be cleaned with
    /// administrator rights (default `false`), and `min_age_setting =
    /// "clean.temp_min_age_days"` names the settings key that may override
    /// `constraints.min_age_days` (see [`CleanEngine::with_settings`](super::CleanEngine::with_settings)).
    ///
    /// Patterns support `*`, `?` and `**` as a whole component and ignore case
    /// (see [`Pattern`]). A root is never an absolute path: it is a named
    /// location plus a relative path. Absolute paths and drives, `..`, and
    /// the whole of a location folder (except the temp folder) are rejected
    /// here; whether the resolved folder is itself safe to clean is decided
    /// when the rule is scanned.
    ///
    /// The whole set is rejected when anything is wrong, and the error lists
    /// every problem (rule id and field path each), not only the first.
    pub fn from_toml(text: &str) -> Result<Self, RulesError> {
        let root: Table = match text.parse() {
            Ok(root) => root,
            Err(error) => {
                let line = error
                    .span()
                    .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
                return Err(RulesError {
                    issues: vec![RuleIssue {
                        position: None,
                        rule: None,
                        field: String::new(),
                        kind: RuleIssueKind::Syntax {
                            message: error.message().to_owned(),
                            line,
                        },
                    }],
                });
            }
        };

        let mut issues = Vec::new();
        let file_issue = |field: &str, kind| RuleIssue {
            position: None,
            rule: None,
            field: field.to_owned(),
            kind,
        };
        for key in root.keys().filter(|key| *key != "rule") {
            issues.push(file_issue(key, RuleIssueKind::UnknownField));
        }

        let mut rules = Vec::new();
        match root.get("rule") {
            None => {}
            Some(Value::Array(items)) => {
                let mut first_seen: HashMap<String, usize> = HashMap::new();
                for (index, item) in items.iter().enumerate() {
                    let position = index + 1;
                    let Some(table) = item.as_table() else {
                        issues.push(RuleIssue {
                            position: Some(position),
                            ..file_issue(
                                "",
                                RuleIssueKind::Invalid("each rule must be a table".to_owned()),
                            )
                        });
                        continue;
                    };
                    let mut reader = Reader {
                        position,
                        id: None,
                        issues: &mut issues,
                    };
                    let rule = reader.rule(table);
                    if let Some(id) = reader.id.clone() {
                        match first_seen.get(&id) {
                            Some(&first_position) => reader.report(
                                "id".to_owned(),
                                RuleIssueKind::DuplicateId { first_position },
                            ),
                            None => {
                                first_seen.insert(id, position);
                            }
                        }
                    }
                    rules.extend(rule);
                }
            }
            Some(_) => issues.push(file_issue(
                "rule",
                RuleIssueKind::Invalid("must be an array of tables, written [[rule]]".to_owned()),
            )),
        }

        if issues.is_empty() {
            Ok(Self { rules })
        } else {
            Err(RulesError { issues })
        }
    }

    /// The rule with this id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&CleaningRule> {
        self.rules.iter().find(|rule| rule.id == id)
    }

    /// All rules, in file order.
    #[must_use]
    pub fn rules(&self) -> &[CleaningRule] {
        &self.rules
    }

    /// Number of rules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether there are no rules.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// Reads one `[[rule]]` table and records every problem it finds.
struct Reader<'a> {
    position: usize,
    id: Option<String>,
    issues: &'a mut Vec<RuleIssue>,
}

impl Reader<'_> {
    fn report(&mut self, field: String, kind: RuleIssueKind) {
        self.issues.push(RuleIssue {
            position: Some(self.position),
            rule: self.id.clone(),
            field,
            kind,
        });
    }

    /// The rule, or `None` when anything was reported for it.
    fn rule(&mut self, table: &Table) -> Option<CleaningRule> {
        let before = self.issues.len();
        // The id first, so every later issue can name the rule.
        self.id = self.text(table, "", "id");
        self.only_keys(table, "", RULE_KEYS);
        if let Some(reason) = self.id.as_deref().and_then(id_problem) {
            self.report("id".to_owned(), RuleIssueKind::Malformed(reason.to_owned()));
        }
        let title = self.text(table, "", "title");
        let description = self.text(table, "", "description");
        let category = self.parsed::<CleanCategory>(table, "", "category");
        let risk = self.parsed::<Risk>(table, "", "risk");
        let root = self.root(table);
        let include = self.patterns(table, "include", true);
        let exclude = self.patterns(table, "exclude", false);
        let constraints = self.constraints(table);
        let requires_admin = match table.get("requires_admin") {
            None => Some(false),
            Some(value) => self.convert::<bool>(value, "requires_admin".to_owned()),
        };
        let min_age_setting = self.min_age_setting(table);
        let locked_by = self.locked_by(table);

        if self.issues.len() > before {
            return None;
        }
        Some(CleaningRule {
            id: self.id.clone()?,
            title: title?,
            description: description?,
            category: category?,
            risk: risk?,
            root: root?,
            include: include?,
            exclude: exclude?,
            constraints: constraints?,
            requires_admin: requires_admin?,
            min_age_setting,
            locked_by: locked_by?,
        })
    }

    /// The optional list of program names that hold the rule's files open.
    /// `None` after a reported problem.
    fn locked_by(&mut self, table: &Table) -> Option<Vec<String>> {
        let Some(value) = table.get("locked_by") else {
            return Some(Vec::new());
        };
        let Some(items) = value.as_array() else {
            self.report(
                "locked_by".to_owned(),
                RuleIssueKind::Invalid("must be an array of strings".to_owned()),
            );
            return None;
        };
        let mut names = Vec::with_capacity(items.len());
        let mut ok = true;
        for (index, item) in items.iter().enumerate() {
            let field = format!("locked_by[{index}]");
            match item.as_str().map(str::to_lowercase) {
                Some(name) if is_program_name(&name) => names.push(name),
                Some(_) => {
                    self.report(
                        field,
                        RuleIssueKind::Malformed(
                            "must be an executable file name such as `chrome.exe`, with no folder"
                                .to_owned(),
                        ),
                    );
                    ok = false;
                }
                None => {
                    self.report(field, RuleIssueKind::Invalid("must be a string".to_owned()));
                    ok = false;
                }
            }
        }
        ok.then_some(names)
    }

    /// The optional settings key that overrides the minimum age. `None` when
    /// the field is absent, and also after a reported problem (the rule is then
    /// rejected as a whole, so the caller needs no third state).
    fn min_age_setting(&mut self, table: &Table) -> Option<String> {
        let value = table.get("min_age_setting")?;
        let field = "min_age_setting".to_owned();
        let Some(key) = value.as_str() else {
            self.report(field, RuleIssueKind::Invalid("must be a string".to_owned()));
            return None;
        };
        if !is_setting_key(key) {
            self.report(
                field,
                RuleIssueKind::Malformed(
                    "must be lower-case words of letters, digits and underscores separated by \
                     dots, such as `clean.temp_min_age_days`, at most 64 characters"
                        .to_owned(),
                ),
            );
            return None;
        }
        Some(key.to_owned())
    }

    fn root(&mut self, table: &Table) -> Option<RuleRoot> {
        let value = self.required(table, "", "root")?;
        let Some(root_table) = value.as_table() else {
            self.report(
                "root".to_owned(),
                RuleIssueKind::Invalid(
                    "must be a table such as { location = \"user_temp\", path = \"Cache\" }"
                        .to_owned(),
                ),
            );
            return None;
        };
        self.only_keys(root_table, "root.", &["location", "path"]);
        let location = self.parsed::<Location>(root_table, "root.", "location");
        // `path` is optional only for a location that allows the folder itself;
        // the check below decides.
        let relative = match root_table.get("path") {
            None => Some(String::new()),
            Some(Value::String(path)) => Some(path.clone()),
            Some(_) => {
                self.report(
                    "root.path".to_owned(),
                    RuleIssueKind::Invalid("must be a string".to_owned()),
                );
                None
            }
        };
        let (location, path) = (location?, relative?);
        let components = self.relative_root(location, &path)?;
        Some(RuleRoot {
            location,
            relative: components,
        })
    }

    /// The components of a relative root path, or `None` after reporting why
    /// it is refused.
    fn relative_root(&mut self, location: Location, path: &str) -> Option<Vec<String>> {
        use RuleIssueKind::{Malformed, UnsafeRoot};
        let field = "root.path".to_owned();
        if path.is_empty() {
            if location.allows_empty_path() {
                return Some(Vec::new());
            }
            self.report(
                field,
                UnsafeRoot(format!(
                    "the root would be the whole `{location}` folder; give a relative path \
                     below it"
                )),
            );
            return None;
        }
        if path.chars().any(char::is_control) {
            self.report(
                field,
                Malformed("must not contain control characters".to_owned()),
            );
            return None;
        }
        if path.contains('\\') {
            self.report(
                field,
                Malformed("use `/` between components, not `\\`".to_owned()),
            );
            return None;
        }
        if path.starts_with('/') || path.contains(':') {
            self.report(
                field,
                UnsafeRoot(
                    "an absolute path or a drive is not allowed; a root is a named location \
                     plus a relative path"
                        .to_owned(),
                ),
            );
            return None;
        }
        if path.chars().count() > MAX_RELATIVE_LEN {
            self.report(
                field,
                Malformed(format!("must be at most {MAX_RELATIVE_LEN} characters")),
            );
            return None;
        }
        let mut components = Vec::new();
        for part in path.split('/') {
            let problem = match part {
                ".." => {
                    self.report(
                        field,
                        UnsafeRoot("`..` would leave the location and is not allowed".to_owned()),
                    );
                    return None;
                }
                "" => Some("has an empty component (a doubled or trailing `/`)"),
                "." => Some("`.` components are not allowed"),
                p if p.ends_with(' ') || p.ends_with('.') => {
                    Some("a component must not end with a space or a dot")
                }
                p if p.chars().any(|c| "*?<>|\"".contains(c)) => {
                    Some("wildcards and characters Windows forbids in names are not allowed")
                }
                _ => None,
            };
            if let Some(problem) = problem {
                self.report(field, Malformed(problem.to_owned()));
                return None;
            }
            components.push(part.to_owned());
        }
        if components.len() > MAX_RELATIVE_COMPONENTS {
            self.report(
                field,
                Malformed(format!(
                    "must have at most {MAX_RELATIVE_COMPONENTS} components"
                )),
            );
            return None;
        }
        Some(components)
    }

    /// A list of patterns; `required` means at least one.
    fn patterns(&mut self, table: &Table, key: &str, required: bool) -> Option<Vec<Pattern>> {
        let value = match table.get(key) {
            None if required => {
                self.report(key.to_owned(), RuleIssueKind::MissingField);
                return None;
            }
            None => return Some(Vec::new()),
            Some(value) => value,
        };
        let Some(items) = value.as_array() else {
            self.report(
                key.to_owned(),
                RuleIssueKind::Invalid("must be an array of strings".to_owned()),
            );
            return None;
        };
        if required && items.is_empty() {
            self.report(key.to_owned(), RuleIssueKind::Empty);
            return None;
        }
        let mut patterns = Vec::with_capacity(items.len());
        let mut ok = true;
        for (index, item) in items.iter().enumerate() {
            let field = format!("{key}[{index}]");
            let Some(text) = item.as_str() else {
                self.report(field, RuleIssueKind::Invalid("must be a string".to_owned()));
                ok = false;
                continue;
            };
            match Pattern::parse(text) {
                Ok(pattern) => patterns.push(pattern),
                Err(PatternError::Unsafe(message)) => {
                    self.report(field, RuleIssueKind::UnsafePattern(message));
                    ok = false;
                }
                Err(PatternError::Malformed(message)) => {
                    self.report(field, RuleIssueKind::Malformed(message));
                    ok = false;
                }
            }
        }
        ok.then_some(patterns)
    }

    fn constraints(&mut self, table: &Table) -> Option<Constraints> {
        let value = self.required(table, "", "constraints")?;
        let Some(inner) = value.as_table() else {
            self.report(
                "constraints".to_owned(),
                RuleIssueKind::Invalid("must be a table".to_owned()),
            );
            return None;
        };
        self.only_keys(
            inner,
            "constraints.",
            &["min_age_days", "max_depth", "remove_empty_dirs"],
        );
        let min_age_days = self.parsed::<u32>(inner, "constraints.", "min_age_days");
        let max_depth = self.parsed::<u32>(inner, "constraints.", "max_depth");
        let remove_empty_dirs = match inner.get("remove_empty_dirs") {
            None => Some(false),
            Some(value) => self.convert::<bool>(value, "constraints.remove_empty_dirs".to_owned()),
        };
        if let Some(days) = min_age_days
            && days > MAX_MIN_AGE_DAYS
        {
            self.report(
                "constraints.min_age_days".to_owned(),
                RuleIssueKind::Invalid(format!("must be at most {MAX_MIN_AGE_DAYS}")),
            );
            return None;
        }
        if let Some(depth) = max_depth
            && depth > MAX_DEPTH_LIMIT
        {
            self.report(
                "constraints.max_depth".to_owned(),
                RuleIssueKind::Invalid(format!("must be at most {MAX_DEPTH_LIMIT}")),
            );
            return None;
        }
        Some(Constraints {
            min_age_days: min_age_days?,
            max_depth: max_depth?,
            remove_empty_dirs: remove_empty_dirs?,
        })
    }

    // ---- field helpers -----------------------------------------------------------------

    fn required<'t>(&mut self, table: &'t Table, path: &str, key: &str) -> Option<&'t Value> {
        let value = table.get(key);
        if value.is_none() {
            self.report(format!("{path}{key}"), RuleIssueKind::MissingField);
        }
        value
    }

    fn only_keys(&mut self, table: &Table, path: &str, allowed: &[&str]) {
        for key in table.keys().filter(|key| !allowed.contains(&key.as_str())) {
            self.report(format!("{path}{key}"), RuleIssueKind::UnknownField);
        }
    }

    /// A required, non-blank string.
    fn text(&mut self, table: &Table, path: &str, key: &str) -> Option<String> {
        let value = self.required(table, path, key)?;
        let field = format!("{path}{key}");
        let Some(text) = value.as_str() else {
            self.report(field, RuleIssueKind::Invalid("must be a string".to_owned()));
            return None;
        };
        if text.trim().is_empty() {
            self.report(field, RuleIssueKind::Empty);
            return None;
        }
        Some(text.to_owned())
    }

    /// A required value of any type serde can read.
    fn parsed<T: DeserializeOwned>(&mut self, table: &Table, path: &str, key: &str) -> Option<T> {
        let value = self.required(table, path, key)?;
        self.convert(value, format!("{path}{key}"))
    }

    fn convert<T: DeserializeOwned>(&mut self, value: &Value, field: String) -> Option<T> {
        match T::deserialize(value.clone()) {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                let message = error.message().to_owned();
                let kind = if message.starts_with("unknown variant") {
                    RuleIssueKind::UnknownValue(message)
                } else {
                    RuleIssueKind::Invalid(message)
                };
                self.report(field, kind);
                None
            }
        }
    }
}

/// Whether `key` is a well formed settings key: dot separated words that start
/// with a lower-case letter and continue with lower-case letters, digits and
/// underscores.
fn is_setting_key(key: &str) -> bool {
    key.len() <= 64
        && key.split('.').all(|word| {
            let mut chars = word.chars();
            chars.next().is_some_and(|c| c.is_ascii_lowercase())
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// Whether `name` is a plain executable file name (`chrome.exe`).
fn is_program_name(name: &str) -> bool {
    name.len() <= 64
        && name.strip_suffix(".exe").is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
        })
}

/// Why `id` is not a valid rule id, if it is not.
fn id_problem(id: &str) -> Option<&'static str> {
    const RULE: &str = "must be lower-case letters and digits separated by single hyphens, \
                        such as `user-temp-files`";
    let well_formed = id.split('-').all(|part| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    });
    if !well_formed {
        Some(RULE)
    } else if id.len() > 64 {
        Some("must be at most 64 characters")
    } else {
        None
    }
}
