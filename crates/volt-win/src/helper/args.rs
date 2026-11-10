//! The helper's command line, parsed strictly.
//!
//! The helper accepts exactly one shape, in exactly this order:
//!
//! ```text
//! volt-helper.exe serve --pipe volt-<32 lower-case hex digits> --ui-pid <pid> --ui-created <ticks>
//! ```
//!
//! `ui-pid` and `ui-created` (the process id and creation time, in 100 ns ticks, of the UI that
//! started the helper) are plain decimal numbers without sign, zeros in front or spaces. Anything
//! else (another word, a missing or repeated flag, another order, another number format, one more
//! argument) is refused, so a parser that accepted an argument could never be the way in for one.
//! The check is simple on purpose: an accepted command line is exactly the text [`HelperArgs::render`]
//! writes for the values it parsed.

use std::fmt;
use std::fmt::Write as _;

use volt_core::PortError;

use crate::error::from_win32;

/// The mode word.
const SERVE: &str = "serve";
const PIPE_FLAG: &str = "--pipe";
const UI_PID_FLAG: &str = "--ui-pid";
const UI_CREATED_FLAG: &str = "--ui-created";

/// The prefix of every pipe name.
const PIPE_PREFIX: &str = "volt-";
/// 128 bits as lower-case hex.
const PIPE_HEX_LEN: usize = 32;

/// A pipe name of the form `volt-` plus 32 lower-case hex digits (128 random bits).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PipeName(String);

impl PipeName {
    /// Accepts a name that matches `^volt-[0-9a-f]{32}$`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix(PIPE_PREFIX)?;
        let valid = hex.len() == PIPE_HEX_LEN
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        valid.then(|| Self(text.to_owned()))
    }

    /// A new name from 128 random bits of the system generator.
    ///
    /// # Errors
    /// When the system generator fails.
    pub fn random() -> Result<Self, PortError> {
        let mut bytes = [0_u8; PIPE_HEX_LEN / 2];
        fill_random(&mut bytes)?;
        let mut name = String::from(PIPE_PREFIX);
        for byte in bytes {
            let _ = write!(name, "{byte:02x}");
        }
        Ok(Self(name))
    }

    /// The name without the `\\.\pipe\` prefix.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The full pipe path, `\\.\pipe\volt-...`.
    #[must_use]
    pub fn full_path(&self) -> String {
        format!(r"\\.\pipe\{}", self.0)
    }
}

impl fmt::Display for PipeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Fills `buffer` from the system's preferred random number generator.
fn fill_random(buffer: &mut [u8]) -> Result<(), PortError> {
    use windows::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    // SAFETY: `buffer` is valid for writes of its length for the call; no algorithm handle is
    // passed because the system-preferred generator flag is set.
    let status = unsafe { BCryptGenRandom(None, buffer, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status.is_ok() {
        Ok(())
    } else {
        Err(from_win32(status.0.cast_unsigned(), "BCryptGenRandom"))
    }
}

/// What a valid helper command line carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperArgs {
    /// The pipe to create.
    pub pipe: PipeName,
    /// Process id of the UI the helper is bound to.
    pub ui_pid: u32,
    /// Creation time of that process, in 100 ns ticks.
    pub ui_created_at_100ns: u64,
}

/// Why a command line was refused. The helper exits without saying more than this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgsError {
    /// Not exactly the expected number of arguments.
    WrongCount,
    /// The mode word is not `serve`.
    NotServe,
    /// A flag is missing, repeated, misspelled or out of order.
    BadFlag,
    /// The pipe name does not match `volt-<32 hex>`.
    BadPipeName,
    /// The process id is not a canonical positive decimal `u32`.
    BadPid,
    /// The creation time is not a canonical positive decimal `u64`.
    BadCreated,
}

impl fmt::Display for ArgsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WrongCount => "wrong number of arguments",
            Self::NotServe => "the mode must be serve",
            Self::BadFlag => "unexpected flag",
            Self::BadPipeName => "bad pipe name",
            Self::BadPid => "bad process id",
            Self::BadCreated => "bad creation time",
        })
    }
}

impl std::error::Error for ArgsError {}

/// A canonical positive decimal: digits only, no zero in front, not zero.
fn canonical_decimal<T: std::str::FromStr>(text: &str) -> Option<T> {
    let digits = text.bytes().all(|byte| byte.is_ascii_digit());
    if text.is_empty() || !digits || text.starts_with('0') {
        return None;
    }
    text.parse().ok()
}

/// Parses the full argument vector, program name first.
///
/// # Errors
/// An [`ArgsError`] for the first thing that is wrong.
pub fn parse_args<S: AsRef<str>>(argv: &[S]) -> Result<HelperArgs, ArgsError> {
    let argv: Vec<&str> = argv.iter().map(AsRef::as_ref).collect();
    // exe, serve, --pipe, NAME, --ui-pid, PID, --ui-created, TICKS
    if argv.len() != 8 {
        return Err(ArgsError::WrongCount);
    }
    if argv[1] != SERVE {
        return Err(ArgsError::NotServe);
    }
    if argv[2] != PIPE_FLAG || argv[4] != UI_PID_FLAG || argv[6] != UI_CREATED_FLAG {
        return Err(ArgsError::BadFlag);
    }
    let pipe = PipeName::parse(argv[3]).ok_or(ArgsError::BadPipeName)?;
    let ui_pid = canonical_decimal::<u32>(argv[5]).ok_or(ArgsError::BadPid)?;
    let ui_created_at_100ns = canonical_decimal::<u64>(argv[7]).ok_or(ArgsError::BadCreated)?;
    Ok(HelperArgs {
        pipe,
        ui_pid,
        ui_created_at_100ns,
    })
}

impl HelperArgs {
    /// The arguments after the program name, as the UI passes them to the helper.
    #[must_use]
    pub fn render(&self) -> Vec<String> {
        vec![
            SERVE.to_owned(),
            PIPE_FLAG.to_owned(),
            self.pipe.to_string(),
            UI_PID_FLAG.to_owned(),
            self.ui_pid.to_string(),
            UI_CREATED_FLAG.to_owned(),
            self.ui_created_at_100ns.to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "volt-0123456789abcdef0123456789abcdef";

    fn argv(parts: &[&str]) -> Vec<String> {
        std::iter::once("volt-helper.exe")
            .chain(parts.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    fn valid() -> Vec<String> {
        argv(&[
            "serve",
            "--pipe",
            NAME,
            "--ui-pid",
            "4242",
            "--ui-created",
            "133800000000000000",
        ])
    }

    fn with(index: usize, value: &str) -> Vec<String> {
        let mut args = valid();
        args[index] = value.to_owned();
        args
    }

    #[test]
    fn the_one_valid_shape_is_accepted() {
        let args = parse_args(&valid()).expect("valid");
        assert_eq!(args.pipe.as_str(), NAME);
        assert_eq!(args.ui_pid, 4242);
        assert_eq!(args.ui_created_at_100ns, 133_800_000_000_000_000);
        assert_eq!(args.pipe.full_path(), format!(r"\\.\pipe\{NAME}"));
    }

    #[test]
    fn it_refuses_every_other_shape() {
        let mut extra = valid();
        extra.push("--debug".into());
        let mut swapped = valid();
        swapped.swap(4, 2);
        swapped.swap(5, 3);
        let cases: Vec<(Vec<String>, ArgsError)> = vec![
            (vec![], ArgsError::WrongCount),
            (argv(&[]), ArgsError::WrongCount),
            (argv(&["serve"]), ArgsError::WrongCount),
            (argv(&["serve", "--pipe", NAME]), ArgsError::WrongCount),
            (extra, ArgsError::WrongCount),
            (with(1, "run"), ArgsError::NotServe),
            (with(1, "Serve"), ArgsError::NotServe),
            (swapped, ArgsError::BadFlag),
            (with(2, "--PIPE"), ArgsError::BadFlag),
            (with(4, "--pipe"), ArgsError::BadFlag),
            (with(3, "volt-0123"), ArgsError::BadPipeName),
            (with(3, &NAME.to_uppercase()), ArgsError::BadPipeName),
            (with(3, &format!(r"..\{NAME}")), ArgsError::BadPipeName),
            (with(3, &format!("{NAME}0")), ArgsError::BadPipeName),
        ];
        for (args, expected) in cases {
            assert_eq!(parse_args(&args), Err(expected), "{args:?}");
        }
    }

    #[test]
    fn numbers_must_be_canonical_decimals() {
        for bad in [
            "",
            "0",
            "01",
            "+5",
            "-5",
            " 5",
            "5 ",
            "5x",
            "0x5",
            "4294967296",
            "\u{661}\u{662}",
        ] {
            assert_eq!(
                parse_args(&with(5, bad)),
                Err(ArgsError::BadPid),
                "pid {bad:?}"
            );
        }
        for bad in ["", "0", "007", "+1", "1e3", "18446744073709551616"] {
            assert_eq!(
                parse_args(&with(7, bad)),
                Err(ArgsError::BadCreated),
                "ticks {bad:?}"
            );
        }
        let mut max = valid();
        max[5] = u32::MAX.to_string();
        max[7] = u64::MAX.to_string();
        assert!(parse_args(&max).is_ok());
    }

    #[test]
    fn render_writes_what_parse_accepts() {
        let args = parse_args(&valid()).expect("valid");
        let mut full = vec!["volt-helper.exe".to_owned()];
        full.extend(args.render());
        assert_eq!(full, valid());
    }

    #[test]
    fn a_random_pipe_name_is_valid_and_new() {
        let first = PipeName::random().expect("random");
        let second = PipeName::random().expect("random");
        assert_eq!(PipeName::parse(first.as_str()), Some(first.clone()));
        assert_ne!(first, second);
    }

    /// xorshift64: a tiny deterministic generator, so the property test needs no dependency and
    /// a failure repeats.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: usize) -> usize {
            usize::try_from(self.next() % bound as u64).unwrap_or(0)
        }
    }

    /// The property: the parser accepts a command line if and only if it is exactly what
    /// `render` writes for the values it parsed. Random mutations of the valid line, and random
    /// token soup, must never be accepted unless they are canonical (and must never panic).
    #[test]
    fn it_accepts_only_what_render_writes() {
        const POOL: [&str; 16] = [
            "serve",
            "--pipe",
            "--ui-pid",
            "--ui-created",
            NAME,
            "4242",
            "0",
            "007",
            "-1",
            "",
            " ",
            "--debug",
            "volt-",
            "SERVE",
            "18446744073709551615",
            "\u{0}",
        ];
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut accepted = 0_u32;
        for _ in 0..20_000 {
            let mut args = valid();
            match rng.below(6) {
                0 => {
                    let index = rng.below(args.len());
                    args[index] = POOL[rng.below(POOL.len())].to_owned();
                }
                1 => {
                    let index = rng.below(args.len());
                    args.remove(index);
                }
                2 => {
                    let index = rng.below(args.len());
                    args.insert(index, POOL[rng.below(POOL.len())].to_owned());
                }
                3 => {
                    let (a, b) = (rng.below(args.len()), rng.below(args.len()));
                    args.swap(a, b);
                }
                4 => {
                    let index = rng.below(args.len());
                    let text = args[index].clone();
                    if !text.is_empty() {
                        let cut = rng.below(text.len());
                        args[index] = text.chars().skip(cut).collect();
                    }
                }
                _ => {
                    args = (0..rng.below(10))
                        .map(|_| POOL[rng.below(POOL.len())].to_owned())
                        .collect();
                }
            }
            if let Ok(parsed) = parse_args(&args) {
                accepted += 1;
                let mut canonical = vec![args[0].clone()];
                canonical.extend(parsed.render());
                assert_eq!(canonical, args, "accepted a non-canonical line");
            }
        }
        // Replacing an element by itself (or swapping equal ones) leaves the line valid now and
        // then, so the property is not vacuous.
        assert!(accepted > 0);
    }
}
