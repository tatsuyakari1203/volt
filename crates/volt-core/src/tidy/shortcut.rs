//! Reading the target of a Windows shortcut (`.lnk`) from its bytes.
//!
//! Only what the file itself says is used: the `LinkInfo` block of the shell link format holds the
//! local path of the target and the type of drive it was on. Nothing is resolved, searched for or
//! started, so reading a shortcut never touches its target or the network. A shortcut that has no
//! such block (a link to a Control Panel item, an app, a special folder, or to a path on a share)
//! is [`Target::Other`]: its target cannot be judged from the file.

/// The size every shortcut header declares.
const HEADER_SIZE: u32 = 0x4C;
/// The header flag that says a target ID list follows the header.
const HAS_ID_LIST: u32 = 0x1;
/// The header flag that says a `LinkInfo` block follows.
const HAS_LINK_INFO: u32 = 0x2;
/// The header flag that says the `LinkInfo` block is to be ignored.
const FORCE_NO_LINK_INFO: u32 = 0x100;
/// The `LinkInfo` flag that says the target has a local path and a volume.
const VOLUME_AND_LOCAL_PATH: u32 = 0x1;
/// The `LinkInfo` header size from which the Unicode path offsets are present.
const UNICODE_HEADER_SIZE: u32 = 0x24;

/// The type of the drive a shortcut's target was on, as Windows reports it (`GetDriveType`).
pub(super) const DRIVE_FIXED: u32 = 3;

/// What a shortcut points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Target {
    /// A path on a local volume.
    Local {
        /// The full path, such as `C:\Tools\app.exe`.
        path: String,
        /// The drive type (`GetDriveType`): 3 is a fixed disk.
        drive_type: u32,
    },
    /// Anything the file does not say enough about: a share, a shell item, an unreadable block.
    Other,
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    let slice = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(slice.try_into().ok()?))
}

/// A zero-terminated ASCII string. Anything else (a code page the reader does not know) is
/// `None`, so a name is never guessed.
fn ascii_at(bytes: &[u8], at: usize) -> Option<String> {
    let rest = bytes.get(at..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    let text = &rest[..end];
    text.is_ascii()
        .then(|| String::from_utf8_lossy(text).into_owned())
}

/// A zero-terminated UTF-16 string.
fn utf16_at(bytes: &[u8], at: usize) -> Option<String> {
    let mut units = Vec::new();
    let mut position = at;
    loop {
        let unit = le16(bytes, position)?;
        if unit == 0 {
            break;
        }
        units.push(unit);
        position += 2;
    }
    String::from_utf16(&units).ok()
}

fn join(base: String, suffix: &str) -> String {
    if suffix.is_empty() {
        base
    } else if base.ends_with('\\') || base.ends_with('/') {
        format!("{base}{suffix}")
    } else {
        format!("{base}\\{suffix}")
    }
}

/// The target a shortcut's bytes name, or [`Target::Other`] when they do not name a local one.
pub(super) fn parse(bytes: &[u8]) -> Target {
    parse_local(bytes).unwrap_or(Target::Other)
}

fn parse_local(bytes: &[u8]) -> Option<Target> {
    if le32(bytes, 0)? != HEADER_SIZE {
        return None;
    }
    let flags = le32(bytes, 20)?;
    if flags & HAS_LINK_INFO == 0 || flags & FORCE_NO_LINK_INFO != 0 {
        return None;
    }
    let mut position = HEADER_SIZE as usize;
    if flags & HAS_ID_LIST != 0 {
        position = position
            .checked_add(2)?
            .checked_add(usize::from(le16(bytes, position)?))?;
    }
    let info_size = le32(bytes, position)? as usize;
    let info = bytes.get(position..position.checked_add(info_size)?)?;
    let header_size = le32(info, 4)?;
    if le32(info, 8)? & VOLUME_AND_LOCAL_PATH == 0 {
        return None;
    }
    let volume = le32(info, 12)? as usize;
    let base_offset = le32(info, 16)? as usize;
    let suffix_offset = le32(info, 24)? as usize;
    let drive_type = le32(info, volume.checked_add(4)?)?;

    let unicode = if header_size >= UNICODE_HEADER_SIZE {
        let base = le32(info, 28)? as usize;
        let suffix = le32(info, 32)? as usize;
        (base != 0).then_some((base, suffix))
    } else {
        None
    };
    let path = match unicode {
        Some((base, suffix)) => {
            let suffix = if suffix == 0 {
                String::new()
            } else {
                utf16_at(info, suffix)?
            };
            join(utf16_at(info, base)?, &suffix)
        }
        None => join(
            ascii_at(info, base_offset)?,
            &ascii_at(info, suffix_offset)?,
        ),
    };
    Some(Target::Local { path, drive_type })
}

/// A shortcut file with a local target, for tests: the smallest bytes this module reads.
#[cfg(test)]
pub(super) fn build(path: &str, drive_type: u32, unicode: bool) -> Vec<u8> {
    let mut info = vec![0_u8; 0x24];
    let header = if unicode { 0x24_u32 } else { 0x1C };
    info[4..8].copy_from_slice(&header.to_le_bytes());
    info[8..12].copy_from_slice(&VOLUME_AND_LOCAL_PATH.to_le_bytes());
    let volume_at = if unicode { 0x24_u32 } else { 0x1C };
    info.truncate(volume_at as usize);
    info[12..16].copy_from_slice(&volume_at.to_le_bytes());
    // Volume: size, drive type, serial, label offset.
    info.extend_from_slice(&16_u32.to_le_bytes());
    info.extend_from_slice(&drive_type.to_le_bytes());
    info.extend_from_slice(&[0; 8]);
    let base_at = u32::try_from(info.len()).unwrap_or(0);
    if unicode {
        info.extend_from_slice(&[0]);
        let wide_at = u32::try_from(info.len()).unwrap_or(0);
        for unit in path.encode_utf16() {
            info.extend_from_slice(&unit.to_le_bytes());
        }
        info.extend_from_slice(&[0, 0]);
        info[16..20].copy_from_slice(&base_at.to_le_bytes());
        info[24..28].copy_from_slice(&base_at.to_le_bytes());
        info[28..32].copy_from_slice(&wide_at.to_le_bytes());
        info[32..36].copy_from_slice(&0_u32.to_le_bytes());
    } else {
        info.extend_from_slice(path.as_bytes());
        info.push(0);
        let suffix_at = u32::try_from(info.len()).unwrap_or(0);
        info.push(0);
        info[16..20].copy_from_slice(&base_at.to_le_bytes());
        info[24..28].copy_from_slice(&suffix_at.to_le_bytes());
    }
    let size = u32::try_from(info.len()).unwrap_or(0);
    info[0..4].copy_from_slice(&size.to_le_bytes());
    let mut bytes = vec![0_u8; HEADER_SIZE as usize];
    bytes[0..4].copy_from_slice(&HEADER_SIZE.to_le_bytes());
    bytes[20..24].copy_from_slice(&HAS_LINK_INFO.to_le_bytes());
    bytes.extend_from_slice(&info);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_ascii_and_a_unicode_local_target() {
        for unicode in [false, true] {
            assert_eq!(
                parse(&build("D:\\Tools\\app.exe", DRIVE_FIXED, unicode)),
                Target::Local {
                    path: "D:\\Tools\\app.exe".to_owned(),
                    drive_type: DRIVE_FIXED
                }
            );
        }
    }

    #[test]
    fn a_shortcut_without_a_local_path_is_not_judged() {
        assert_eq!(parse(&[]), Target::Other);
        assert_eq!(parse(&[0_u8; 100]), Target::Other);
        let mut no_info = build("C:\\a", DRIVE_FIXED, false);
        no_info[20..24].copy_from_slice(&0_u32.to_le_bytes());
        assert_eq!(parse(&no_info), Target::Other);
        let mut cut = build("C:\\a", DRIVE_FIXED, false);
        cut.truncate(90);
        assert_eq!(parse(&cut), Target::Other);
        let mut accented = build("C:\\a", DRIVE_FIXED, false);
        let at = accented.len() - 3;
        accented[at] = 0xE9;
        assert_eq!(parse(&accented), Target::Other);
    }
}
