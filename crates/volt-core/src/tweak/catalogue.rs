//! Loading and validating a tweak catalogue from TOML text.
//!
//! The core never reads files: the composition root hands it the text. The
//! reader is written by hand over the parsed TOML (leaf values such as enums
//! still go through serde) so that every problem can be reported with the id of
//! the tweak and the path of the field, and so that all problems of a file are
//! reported at once instead of only the first one.

use std::collections::HashMap;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use super::definition::{Category, RestartRequirement, Tweak, TweakStep};
use crate::command::{Command, MAX_TIMEOUT_SECS};
use crate::label::{Evidence, Risk};
use crate::registry::{RegistryHive, RegistryValue, RegistryView};
use crate::service::ServiceStartType;

const TWEAK_KEYS: &[&str] = &[
    "id",
    "title",
    "description",
    "category",
    "risk",
    "evidence",
    "evidence_note",
    "restart",
    "requires_admin",
    "steps",
];

/// What is wrong with a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum IssueKind {
    /// The text is not valid TOML. `line` is 1-based when known.
    Syntax {
        /// The parser's message.
        message: String,
        /// Where it went wrong.
        line: Option<usize>,
    },
    /// A required field is absent.
    MissingField,
    /// A command step has no `revert` command. Everything Volt applies must be
    /// undoable.
    MissingRevert,
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
    /// Another tweak already uses this id.
    DuplicateId {
        /// 1-based position of the first `[[tweak]]` table with this id.
        first_position: usize,
    },
    /// An earlier step of the same tweak changes the same thing, so the tweak's
    /// state could never be told apart.
    DuplicateTarget {
        /// Index of the earlier step.
        first_step: usize,
    },
}

impl fmt::Display for IssueKind {
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
            Self::MissingRevert => f.write_str(
                "a command step must declare a revert command: everything Volt applies must be undoable",
            ),
            Self::UnknownField => f.write_str("unknown field"),
            Self::UnknownValue(message) | Self::Invalid(message) | Self::Malformed(message) => {
                f.write_str(message)
            }
            Self::Empty => f.write_str("must not be empty"),
            Self::DuplicateId { first_position } => {
                write!(f, "duplicate id, already used by tweak #{first_position}")
            }
            Self::DuplicateTarget { first_step } => write!(
                f,
                "changes the same thing as steps[{first_step}]; merge them into one step"
            ),
        }
    }
}

/// One problem found while loading a catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueIssue {
    /// 1-based position of the `[[tweak]]` table in the file; `None` for a
    /// problem with the file as a whole.
    pub position: Option<usize>,
    /// The tweak's id, when it could be read.
    pub tweak: Option<String>,
    /// Path of the field, such as `risk` or `steps[1].revert`.
    pub field: String,
    /// What is wrong with it.
    pub kind: IssueKind,
}

impl fmt::Display for CatalogueIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.tweak, self.position) {
            (Some(id), Some(position)) => write!(f, "tweak \"{id}\" (#{position}): ")?,
            (Some(id), None) => write!(f, "tweak \"{id}\": ")?,
            (None, Some(position)) => write!(f, "tweak #{position}: ")?,
            (None, None) => {}
        }
        if self.field.is_empty() {
            write!(f, "{}", self.kind)
        } else {
            write!(f, "field `{}`: {}", self.field, self.kind)
        }
    }
}

/// A catalogue was rejected as a whole. Nothing from it was loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueError {
    /// Every problem found, in file order. Never empty.
    pub issues: Vec<CatalogueIssue>,
}

impl fmt::Display for CatalogueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "tweak catalogue rejected: {} problem(s)",
            self.issues.len()
        )?;
        for issue in &self.issues {
            write!(f, "\n  - {issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CatalogueError {}

/// A validated set of tweaks with unique ids.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Catalogue {
    tweaks: Vec<Tweak>,
}

impl Catalogue {
    /// Loads a catalogue from TOML text: an array of `[[tweak]]` tables.
    ///
    /// ```toml
    /// [[tweak]]
    /// id = "disable-hibernation"        # lower-case letters, digits, single hyphens
    /// title = "Turn off hibernation"
    /// description = "Frees the hibernation file."
    /// category = "performance"          # performance, gaming, privacy, appearance,
    ///                                   # network, windows_update, security, debloat
    /// risk = "caution"                  # safe, caution, advanced
    /// evidence = "measured"             # measured, anecdotal
    /// evidence_note = "Frees about 75% of installed RAM on disk."
    /// restart = "none"                  # optional: none, sign_out, reboot
    /// requires_admin = false            # optional: true for machine-wide changes, services
    ///                                   # and commands that need administrator rights
    ///
    /// [[tweak.steps]]                   # applied in order, reverted newest first
    /// kind = "registry_set"
    /// hive = "HKLM"                     # HKCR, HKCU, HKLM, HKU, HKCC
    /// subkey = 'SYSTEM\CurrentControlSet\Control\Power'
    /// name = "HibernateEnabled"         # "" is the key's default value
    /// view = "native"                   # optional: native, bit32, bit64
    /// value = { dword = 0 }             # dword, qword, sz, expand_sz, multi_sz, binary
    /// off = { dword = 1 }               # optional: the Windows default, which a reset writes;
    ///                                   # without it a reset removes the value
    ///
    /// [[tweak.steps]]
    /// kind = "registry_delete"          # same fields, without `value`; `off` is what a reset
    ///                                   # writes back, and without it a reset is not possible
    ///
    /// [[tweak.steps]]
    /// kind = "registry_key"             # a key that must exist, or must be absent
    /// hive = "HKCU"                     # (same `hive`, `subkey` and `view` as above)
    /// subkey = 'Software\Example'
    /// exists = true                     # false: the key must not exist (no reset then)
    ///
    /// [[tweak.steps]]
    /// kind = "service"
    /// service = "SysMain"
    /// start_type = "disabled"           # boot, system, automatic, automatic_delayed,
    ///                                   # manual, disabled
    /// off = "automatic"                 # optional: the Windows default a reset sets
    ///
    /// [[tweak.steps]]
    /// kind = "command"                  # program + argument list, never a shell string
    /// detect = { program = "powercfg", args = ["/query"] }   # exit 0 = applied, 1 = not
    /// apply = { program = "powercfg", args = ["/hibernate", "off"] }
    /// revert = { program = "powercfg", args = ["/hibernate", "on"] }   # required
    /// off = { program = "powercfg", args = ["/hibernate", "on"] }      # optional: what a reset runs
    /// # Any command table may add `timeout_secs = 120` (1 to 600; the default is the adapter's).
    /// ```
    ///
    /// A tweak can be reset (turned off although Volt did not apply it) only when every step says
    /// what it goes back to: a set step always does, the others need `off` (a key step that makes
    /// a key exist is deleted).
    ///
    /// The whole catalogue is rejected when anything is wrong, and the error
    /// lists every problem (tweak id and field path each), not only the first.
    pub fn from_toml(text: &str) -> Result<Self, CatalogueError> {
        let root: Table = match text.parse() {
            Ok(root) => root,
            Err(error) => {
                let line = error
                    .span()
                    .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
                return Err(CatalogueError {
                    issues: vec![CatalogueIssue {
                        position: None,
                        tweak: None,
                        field: String::new(),
                        kind: IssueKind::Syntax {
                            message: error.message().to_owned(),
                            line,
                        },
                    }],
                });
            }
        };

        Self::from_root(&root)
    }

    /// Validates an already parsed file. The tests also feed it serialized
    /// tweaks to prove the schema and the serialized form agree.
    pub(super) fn from_root(root: &Table) -> Result<Self, CatalogueError> {
        let mut issues = Vec::new();
        let file_issue = |field: &str, kind| CatalogueIssue {
            position: None,
            tweak: None,
            field: field.to_owned(),
            kind,
        };
        for key in root.keys().filter(|key| *key != "tweak") {
            issues.push(file_issue(key, IssueKind::UnknownField));
        }

        let mut tweaks = Vec::new();
        match root.get("tweak") {
            None => {}
            Some(Value::Array(items)) => {
                let mut first_seen: HashMap<String, usize> = HashMap::new();
                for (index, item) in items.iter().enumerate() {
                    let position = index + 1;
                    let Some(table) = item.as_table() else {
                        issues.push(CatalogueIssue {
                            position: Some(position),
                            ..file_issue(
                                "",
                                IssueKind::Invalid("each tweak must be a table".to_owned()),
                            )
                        });
                        continue;
                    };
                    let mut reader = Reader {
                        position,
                        id: None,
                        issues: &mut issues,
                    };
                    let tweak = reader.tweak(table);
                    if let Some(id) = reader.id.clone() {
                        match first_seen.get(&id) {
                            Some(&first_position) => reader
                                .report("id".to_owned(), IssueKind::DuplicateId { first_position }),
                            None => {
                                first_seen.insert(id, position);
                            }
                        }
                    }
                    tweaks.extend(tweak);
                }
            }
            Some(_) => issues.push(file_issue(
                "tweak",
                IssueKind::Invalid("must be an array of tables, written [[tweak]]".to_owned()),
            )),
        }

        if issues.is_empty() {
            Ok(Self { tweaks })
        } else {
            Err(CatalogueError { issues })
        }
    }

    /// The tweak with this id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Tweak> {
        self.tweaks.iter().find(|tweak| tweak.id == id)
    }

    /// All tweaks, in file order.
    #[must_use]
    pub fn tweaks(&self) -> &[Tweak] {
        &self.tweaks
    }

    /// Number of tweaks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tweaks.len()
    }

    /// Whether there are no tweaks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tweaks.is_empty()
    }
}

/// Reads one `[[tweak]]` table and records every problem it finds.
struct Reader<'a> {
    position: usize,
    id: Option<String>,
    issues: &'a mut Vec<CatalogueIssue>,
}

impl Reader<'_> {
    fn report(&mut self, field: String, kind: IssueKind) {
        self.issues.push(CatalogueIssue {
            position: Some(self.position),
            tweak: self.id.clone(),
            field,
            kind,
        });
    }

    /// The tweak, or `None` when anything was reported for it.
    fn tweak(&mut self, table: &Table) -> Option<Tweak> {
        let before = self.issues.len();
        self.only_keys(table, "", TWEAK_KEYS);

        self.id = self.text(table, "", "id", false);
        if let Some(reason) = self.id.as_deref().and_then(id_problem) {
            self.report("id".to_owned(), IssueKind::Malformed(reason.to_owned()));
        }
        let title = self.text(table, "", "title", false);
        let description = self.text(table, "", "description", false);
        let category = self.parsed::<Category>(table, "", "category");
        let risk = self.parsed::<Risk>(table, "", "risk");
        let evidence = self.parsed::<Evidence>(table, "", "evidence");
        let evidence_note = self.text(table, "", "evidence_note", false);
        let restart = self.parsed_or(table, "", "restart", RestartRequirement::None);
        let requires_admin = self.parsed_or(table, "", "requires_admin", false);
        let steps = self.steps(table);
        if self.issues.len() == before {
            self.check_duplicate_targets(&steps);
        }

        if self.issues.len() > before {
            return None;
        }
        Some(Tweak {
            id: self.id.clone()?,
            title: title?,
            description: description?,
            category: category?,
            risk: risk?,
            evidence: evidence?,
            evidence_note: evidence_note?,
            restart: restart?,
            requires_admin: requires_admin?,
            steps,
        })
    }

    fn steps(&mut self, table: &Table) -> Vec<TweakStep> {
        let Some(value) = self.required(table, "", "steps") else {
            return Vec::new();
        };
        let Some(items) = value.as_array() else {
            self.report(
                "steps".to_owned(),
                IssueKind::Invalid(
                    "must be an array of tables, written [[tweak.steps]]".to_owned(),
                ),
            );
            return Vec::new();
        };
        if items.is_empty() {
            self.report("steps".to_owned(), IssueKind::Empty);
        }
        let mut steps = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            let Some(step_table) = item.as_table() else {
                self.report(
                    format!("steps[{index}]"),
                    IssueKind::Invalid("each step must be a table".to_owned()),
                );
                continue;
            };
            steps.extend(self.step(step_table, &format!("steps[{index}].")));
        }
        steps
    }

    fn step(&mut self, table: &Table, path: &str) -> Option<TweakStep> {
        let kind = self.text(table, path, "kind", false)?;
        match kind.as_str() {
            "registry_set" => self.registry_set_step(table, path),
            "registry_delete" => self.registry_delete_step(table, path),
            "registry_key" => self.registry_key_step(table, path),
            "service" => self.service_step(table, path),
            "command" => self.command_step(table, path),
            other => {
                self.report(
                    format!("{path}kind"),
                    IssueKind::UnknownValue(format!(
                        "unknown step kind `{other}`, expected one of `registry_set`, \
                         `registry_delete`, `registry_key`, `service`, `command`"
                    )),
                );
                None
            }
        }
    }

    fn registry_set_step(&mut self, table: &Table, path: &str) -> Option<TweakStep> {
        self.only_keys(
            table,
            path,
            &["kind", "hive", "subkey", "name", "view", "value", "off"],
        );
        let target = self.registry_target(table, path);
        let value = self.parsed::<RegistryValue>(table, path, "value");
        let off = self.parsed_or::<Option<RegistryValue>>(table, path, "off", None);
        if let (Some(value), Some(Some(off))) = (&value, &off)
            && !self.off_differs(path, "value", value == off)
        {
            return None;
        }
        let (hive, subkey, name, view) = target?;
        Some(TweakStep::RegistrySet {
            hive,
            subkey,
            name,
            view,
            value: value?,
            off: off?,
        })
    }

    fn registry_delete_step(&mut self, table: &Table, path: &str) -> Option<TweakStep> {
        self.only_keys(
            table,
            path,
            &["kind", "hive", "subkey", "name", "view", "off"],
        );
        let target = self.registry_target(table, path);
        let off = self.parsed_or::<Option<RegistryValue>>(table, path, "off", None);
        let (hive, subkey, name, view) = target?;
        Some(TweakStep::RegistryDelete {
            hive,
            subkey,
            name,
            view,
            off: off?,
        })
    }

    fn registry_key_step(&mut self, table: &Table, path: &str) -> Option<TweakStep> {
        self.only_keys(table, path, &["kind", "hive", "subkey", "view", "exists"]);
        let hive = self.parsed::<RegistryHive>(table, path, "hive");
        let subkey = self.subkey(table, path);
        let view = self.parsed_or(table, path, "view", RegistryView::Native);
        let exists = self.parsed::<bool>(table, path, "exists");
        Some(TweakStep::RegistryKey {
            hive: hive?,
            subkey: subkey?,
            view: view?,
            exists: exists?,
        })
    }

    fn service_step(&mut self, table: &Table, path: &str) -> Option<TweakStep> {
        self.only_keys(table, path, &["kind", "service", "start_type", "off"]);
        let service = self.text(table, path, "service", false);
        if let Some(name) = &service
            && let Some(reason) = service_problem(name)
        {
            self.report(
                format!("{path}service"),
                IssueKind::Malformed(reason.to_owned()),
            );
            return None;
        }
        let start_type = self.parsed::<ServiceStartType>(table, path, "start_type");
        let off = self.parsed_or::<Option<ServiceStartType>>(table, path, "off", None);
        if let (Some(start_type), Some(Some(off))) = (&start_type, &off)
            && !self.off_differs(path, "start_type", start_type == off)
        {
            return None;
        }
        Some(TweakStep::Service {
            service: service?,
            start_type: start_type?,
            off: off?,
        })
    }

    fn command_step(&mut self, table: &Table, path: &str) -> Option<TweakStep> {
        self.only_keys(table, path, &["kind", "detect", "apply", "revert", "off"]);
        let detect = self.command(table, path, "detect", IssueKind::MissingField);
        let apply = self.command(table, path, "apply", IssueKind::MissingField);
        let revert = self.command(table, path, "revert", IssueKind::MissingRevert);
        let off = if table.contains_key("off") {
            self.command(table, path, "off", IssueKind::MissingField)
                .map(Some)
        } else {
            Some(None)
        };
        Some(TweakStep::Command {
            detect: detect?,
            apply: apply?,
            revert: revert?,
            off: off?,
        })
    }

    /// Reports `off` when it equals what the step sets (`same`): the step could never be turned
    /// off. Returns whether it is fine.
    fn off_differs(&mut self, path: &str, field: &str, same: bool) -> bool {
        if same {
            self.report(
                format!("{path}off"),
                IssueKind::Invalid(format!(
                    "must differ from `{field}`, or the step could never be turned off"
                )),
            );
        }
        !same
    }

    fn registry_target(
        &mut self,
        table: &Table,
        path: &str,
    ) -> Option<(RegistryHive, String, String, RegistryView)> {
        let hive = self.parsed::<RegistryHive>(table, path, "hive");
        let subkey = self.subkey(table, path);
        let mut name = self.text(table, path, "name", true);
        if name
            .as_deref()
            .is_some_and(|n| n.chars().any(char::is_control))
        {
            self.report(
                format!("{path}name"),
                IssueKind::Malformed("must not contain control characters".to_owned()),
            );
            name = None;
        }
        let view = self.parsed_or(table, path, "view", RegistryView::Native);
        Some((hive?, subkey?, name?, view?))
    }

    /// The `subkey` field: a well-formed registry path.
    fn subkey(&mut self, table: &Table, path: &str) -> Option<String> {
        let subkey = self.text(table, path, "subkey", false)?;
        if let Some(reason) = subkey_problem(&subkey) {
            self.report(
                format!("{path}subkey"),
                IssueKind::Malformed(reason.to_owned()),
            );
            return None;
        }
        Some(subkey)
    }

    /// A command table: a non-empty `program` and an optional `args` list.
    fn command(
        &mut self,
        table: &Table,
        path: &str,
        key: &str,
        missing: IssueKind,
    ) -> Option<Command> {
        let field = format!("{path}{key}");
        let Some(value) = table.get(key) else {
            self.report(field, missing);
            return None;
        };
        let Some(command_table) = value.as_table() else {
            self.report(
                field,
                IssueKind::Invalid(
                    "must be a table such as { program = \"name\", args = [\"a\", \"b\"] }; \
                     a command is never a shell string"
                        .to_owned(),
                ),
            );
            return None;
        };
        let inner = format!("{field}.");
        self.only_keys(command_table, &inner, &["program", "args", "timeout_secs"]);
        let mut program = self.text(command_table, &inner, "program", false);
        if program
            .as_deref()
            .is_some_and(|p| p.chars().any(char::is_control))
        {
            self.report(
                format!("{inner}program"),
                IssueKind::Malformed("must not contain control characters".to_owned()),
            );
            program = None;
        }
        let args = match command_table.get("args") {
            None => Some(Vec::new()),
            Some(args) => self.convert::<Vec<String>>(args, format!("{inner}args")),
        };
        if args
            .as_ref()
            .is_some_and(|a| a.iter().any(|arg| arg.contains('\0')))
        {
            self.report(
                format!("{inner}args"),
                IssueKind::Malformed("arguments must not contain NUL".to_owned()),
            );
            return None;
        }
        let timeout_secs = match command_table.get("timeout_secs") {
            None => None,
            Some(value) => Some(self.timeout_secs(value, format!("{inner}timeout_secs"))?),
        };
        Some(Command {
            program: program?,
            args: args?,
            timeout_secs,
        })
    }

    /// A command's own time limit: whole seconds from 1 to [`MAX_TIMEOUT_SECS`].
    fn timeout_secs(&mut self, value: &Value, field: String) -> Option<u32> {
        let secs = value
            .as_integer()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (1..=MAX_TIMEOUT_SECS).contains(n));
        if secs.is_none() {
            self.report(
                field,
                IssueKind::Invalid(format!(
                    "must be a whole number of seconds from 1 to {MAX_TIMEOUT_SECS}"
                )),
            );
        }
        secs
    }

    /// Two steps that change the same registry value or service.
    fn check_duplicate_targets(&mut self, steps: &[TweakStep]) {
        let mut first: HashMap<String, usize> = HashMap::new();
        for (index, step) in steps.iter().enumerate() {
            let key = match step {
                TweakStep::RegistrySet {
                    hive,
                    subkey,
                    name,
                    view,
                    ..
                }
                | TweakStep::RegistryDelete {
                    hive,
                    subkey,
                    name,
                    view,
                    ..
                } => format!("{hive:?}|{view:?}|{subkey}|{name}").to_lowercase(),
                TweakStep::RegistryKey {
                    hive, subkey, view, ..
                } => format!("key|{hive:?}|{view:?}|{subkey}").to_lowercase(),
                TweakStep::Service { service, .. } => format!("service|{service}").to_lowercase(),
                TweakStep::Command { .. } => continue,
            };
            match first.get(&key) {
                Some(&first_step) => self.report(
                    format!("steps[{index}]"),
                    IssueKind::DuplicateTarget { first_step },
                ),
                None => {
                    first.insert(key, index);
                }
            }
        }
    }

    // ---- field helpers -----------------------------------------------------------------

    fn required<'t>(&mut self, table: &'t Table, path: &str, key: &str) -> Option<&'t Value> {
        let value = table.get(key);
        if value.is_none() {
            self.report(format!("{path}{key}"), IssueKind::MissingField);
        }
        value
    }

    fn only_keys(&mut self, table: &Table, path: &str, allowed: &[&str]) {
        for key in table.keys().filter(|key| !allowed.contains(&key.as_str())) {
            self.report(format!("{path}{key}"), IssueKind::UnknownField);
        }
    }

    /// A required string; empty (after trimming) is rejected unless allowed.
    fn text(&mut self, table: &Table, path: &str, key: &str, allow_empty: bool) -> Option<String> {
        let value = self.required(table, path, key)?;
        let field = format!("{path}{key}");
        let Some(text) = value.as_str() else {
            self.report(field, IssueKind::Invalid("must be a string".to_owned()));
            return None;
        };
        if !allow_empty && text.trim().is_empty() {
            self.report(field, IssueKind::Empty);
            return None;
        }
        Some(text.to_owned())
    }

    /// A required value of any type serde can read.
    fn parsed<T: DeserializeOwned>(&mut self, table: &Table, path: &str, key: &str) -> Option<T> {
        let value = self.required(table, path, key)?;
        self.convert(value, format!("{path}{key}"))
    }

    /// An optional value; absent means `default`.
    fn parsed_or<T: DeserializeOwned>(
        &mut self,
        table: &Table,
        path: &str,
        key: &str,
        default: T,
    ) -> Option<T> {
        match table.get(key) {
            None => Some(default),
            Some(value) => self.convert(value, format!("{path}{key}")),
        }
    }

    fn convert<T: DeserializeOwned>(&mut self, value: &Value, field: String) -> Option<T> {
        match T::deserialize(value.clone()) {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                let message = error.message().to_owned();
                let kind = if message.starts_with("unknown variant") {
                    IssueKind::UnknownValue(message)
                } else {
                    IssueKind::Invalid(message)
                };
                self.report(field, kind);
                None
            }
        }
    }
}

/// Why `id` is not a valid tweak id, if it is not.
fn id_problem(id: &str) -> Option<&'static str> {
    const RULE: &str = "must be lower-case letters and digits separated by single hyphens, \
                        such as `disable-sysmain`";
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

/// Why `subkey` is not a usable registry key path, if it is not.
fn subkey_problem(subkey: &str) -> Option<&'static str> {
    if subkey.chars().any(char::is_control) {
        Some("must not contain control characters")
    } else if subkey.split('\\').any(|part| part.trim().is_empty()) {
        Some(
            "contains an empty component: no leading, trailing or doubled backslash, \
             and no blank key names",
        )
    } else {
        None
    }
}

/// Why `service` is not a usable service name, if it is not.
fn service_problem(service: &str) -> Option<&'static str> {
    if service.chars().any(char::is_control) || service.contains(['\\', '/']) {
        Some("must be a service name without control characters, `\\` or `/`")
    } else {
        None
    }
}
