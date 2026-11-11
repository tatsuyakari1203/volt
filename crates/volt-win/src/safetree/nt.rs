//! The raw file calls of the tree walker: open one name relative to a held directory handle, read
//! what a handle says about itself, list a directory, delete through a handle.
//!
//! Every open here is `NtCreateFile` with a `RootDirectory` and ONE name (no separators), so a
//! path is never re-resolved from the top, and with `FILE_OPEN_REPARSE_POINT`, so a link is opened
//! as itself and never followed. Handles are shared for read and write but not for delete, so while
//! one is held nobody can rename or delete what it names, which is what keeps an ancestor from being
//! swapped away under the walk.

use std::ffi::c_void;
use std::mem::{offset_of, size_of};

use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_DISPOSITION_DELETE, FILE_DISPOSITION_IGNORE_READONLY_ATTRIBUTE,
    FILE_DISPOSITION_POSIX_SEMANTICS, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT, FileBasicInformation, FileDispositionInformationEx,
    FileIdExtdDirectoryInformation, FileIdInformation, FileStandardInformation,
    NTCREATEFILE_CREATE_OPTIONS, NtCreateFile, NtQueryDirectoryFileEx, NtQueryInformationFile,
    NtSetInformationFile,
};
use windows::Win32::Foundation::{HANDLE, OBJECT_ATTRIBUTE_FLAGS, UNICODE_STRING};
use windows::Win32::Storage::FileSystem::{
    FILE_ACCESS_RIGHTS, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};
use windows::Win32::System::IO::IO_STATUS_BLOCK;
use windows::core::PWSTR;

use crate::helper::OwnedHandle;

pub(super) const STATUS_SUCCESS: i32 = 0;
pub(super) const STATUS_NO_MORE_FILES: i32 = 0x8000_0006_u32.cast_signed();
pub(super) const STATUS_OBJECT_NAME_NOT_FOUND: i32 = 0xC000_0034_u32.cast_signed();
pub(super) const STATUS_OBJECT_PATH_NOT_FOUND: i32 = 0xC000_003A_u32.cast_signed();
pub(super) const STATUS_NO_SUCH_FILE: i32 = 0xC000_000F_u32.cast_signed();
pub(super) const STATUS_ACCESS_DENIED: i32 = 0xC000_0022_u32.cast_signed();
pub(super) const STATUS_SHARING_VIOLATION: i32 = 0xC000_0043_u32.cast_signed();
pub(super) const STATUS_DELETE_PENDING: i32 = 0xC000_0056_u32.cast_signed();
pub(super) const STATUS_CANNOT_DELETE: i32 = 0xC000_0121_u32.cast_signed();
pub(super) const STATUS_DIRECTORY_NOT_EMPTY: i32 = 0xC000_0101_u32.cast_signed();
pub(super) const STATUS_NOT_A_DIRECTORY: i32 = 0xC000_0103_u32.cast_signed();
pub(super) const STATUS_FILE_IS_A_DIRECTORY: i32 = 0xC000_00BA_u32.cast_signed();
pub(super) const STATUS_INVALID_PARAMETER: i32 = 0xC000_000D_u32.cast_signed();

pub(super) const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
pub(super) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;
const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
const FILE_WRITE_ATTRIBUTES: u32 = 0x0000_0100;
const DELETE: u32 = 0x0001_0000;
const READ_CONTROL: u32 = 0x0002_0000;
const SYNCHRONIZE: u32 = 0x0010_0000;

/// `SL_RESTART_SCAN` of `NtQueryDirectoryFileEx`.
const RESTART_SCAN: u32 = 0x1;

/// Directory listing buffer: large enough for hundreds of entries per call.
const LIST_BUFFER_WORDS: usize = 8192;

/// 100 ns intervals between 1601-01-01 and 1970-01-01.
const UNIX_EPOCH_AS_FILETIME: i64 = 116_444_736_000_000_000;

/// What the kernel gave back: an `NTSTATUS` as its number.
pub(super) type Status = i32;

/// Identity of a file system object: volume serial number and the 128-bit file id. Two handles name
/// the same object exactly when these are equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FileId {
    pub(super) volume: u64,
    pub(super) id: [u8; 16],
}

/// What a handle says about its object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Meta {
    pub(super) attributes: u32,
    pub(super) id: FileId,
    pub(super) size: u64,
    /// Last write time, Unix milliseconds.
    pub(super) modified_ms: Option<i64>,
    pub(super) links: u32,
    pub(super) is_directory: bool,
}

impl Meta {
    pub(super) fn is_reparse_point(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
}

/// One entry of a directory listing.
#[derive(Debug, Clone)]
pub(super) struct Entry {
    /// The name as the file system stores it, raw UTF-16.
    pub(super) name: Vec<u16>,
    pub(super) attributes: u32,
    pub(super) id_bytes: [u8; 16],
    pub(super) size: u64,
    pub(super) modified_ms: Option<i64>,
}

impl Entry {
    pub(super) fn is_reparse_point(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    pub(super) fn is_directory(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }
}

/// What to ask for when a name is opened.
#[derive(Debug, Clone, Copy)]
pub(super) enum Want {
    /// List a directory and read its attributes and access list.
    Directory,
    /// As `Directory`, and the right to delete it.
    DirectoryToDelete,
    /// A file to read the attributes of and delete.
    FileToDelete,
}

fn filetime_to_unix_ms(filetime: i64) -> Option<i64> {
    // 0 is "not set"; anything before the Unix epoch cannot be a file time of this machine's files.
    if filetime <= UNIX_EPOCH_AS_FILETIME {
        return None;
    }
    Some((filetime - UNIX_EPOCH_AS_FILETIME) / 10_000)
}

/// Opens `name` (a single component, raw UTF-16) below `parent`, or the volume root when `parent`
/// is `None` (then `name` is the NT path of the volume, `\??\Volume{...}\`). A link is opened as
/// itself.
pub(super) fn open_at(
    parent: Option<HANDLE>,
    name: &[u16],
    want: Want,
) -> Result<OwnedHandle, Status> {
    let (access, options) = match want {
        Want::Directory => (
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
            FILE_DIRECTORY_FILE.0 | FILE_OPEN_REPARSE_POINT.0 | FILE_SYNCHRONOUS_IO_NONALERT.0,
        ),
        Want::DirectoryToDelete => (
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL | DELETE | SYNCHRONIZE,
            FILE_DIRECTORY_FILE.0 | FILE_OPEN_REPARSE_POINT.0 | FILE_SYNCHRONOUS_IO_NONALERT.0,
        ),
        Want::FileToDelete => (
            FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | DELETE | SYNCHRONIZE,
            FILE_NON_DIRECTORY_FILE.0 | FILE_OPEN_REPARSE_POINT.0 | FILE_SYNCHRONOUS_IO_NONALERT.0,
        ),
    };
    let Ok(bytes) = u16::try_from(name.len() * 2) else {
        return Err(STATUS_INVALID_PARAMETER);
    };
    let unicode = UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes,
        Buffer: PWSTR(name.as_ptr().cast_mut()),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>()).unwrap_or(0),
        RootDirectory: parent.unwrap_or_default(),
        ObjectName: &raw const unicode,
        // Case-sensitive on purpose: the name is the one the listing returned, and exactly that
        // entry is opened.
        Attributes: OBJECT_ATTRIBUTE_FLAGS(0),
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut handle = HANDLE::default();
    let mut block = IO_STATUS_BLOCK::default();
    // SAFETY: every pointer refers to a local that outlives the call (`unicode` points at `name`,
    // which the caller keeps alive); the out pointers are valid. Sharing is read and write only, so
    // no other opener can rename or delete the object while the handle is held.
    let status = unsafe {
        NtCreateFile(
            &raw mut handle,
            FILE_ACCESS_RIGHTS(access),
            &raw const attributes,
            &raw mut block,
            None,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0),
            FILE_OPEN,
            NTCREATEFILE_CREATE_OPTIONS(options),
            None,
            0,
        )
    };
    if status.0 == STATUS_SUCCESS {
        Ok(OwnedHandle(handle))
    } else {
        Err(status.0)
    }
}

/// What the handle's object is: attributes, identity, size, last write time, link count.
pub(super) fn meta(handle: &OwnedHandle) -> Result<Meta, Status> {
    #[repr(C)]
    #[derive(Default)]
    struct Basic {
        creation: i64,
        access: i64,
        write: i64,
        change: i64,
        attributes: u32,
        padding: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Standard {
        allocation: i64,
        end_of_file: i64,
        links: u32,
        delete_pending: u8,
        directory: u8,
        padding: u16,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Id {
        volume: u64,
        file: [u8; 16],
    }

    fn query<T: Default>(
        handle: &OwnedHandle,
        class: windows::Wdk::Storage::FileSystem::FILE_INFORMATION_CLASS,
    ) -> Result<T, Status> {
        let mut value = T::default();
        let mut block = IO_STATUS_BLOCK::default();
        // SAFETY: `value` is a plain-data buffer of exactly the size passed, valid for writes; the
        // handle is open; `block` is a valid out pointer.
        let status = unsafe {
            NtQueryInformationFile(
                handle.raw(),
                &raw mut block,
                (&raw mut value).cast::<c_void>(),
                u32::try_from(size_of::<T>()).unwrap_or(0),
                class,
            )
        };
        if status.0 == STATUS_SUCCESS {
            Ok(value)
        } else {
            Err(status.0)
        }
    }

    let basic: Basic = query(handle, FileBasicInformation)?;
    let standard: Standard = query(handle, FileStandardInformation)?;
    let id: Id = query(handle, FileIdInformation)?;
    Ok(Meta {
        attributes: basic.attributes,
        id: FileId {
            volume: id.volume,
            id: id.file,
        },
        size: u64::try_from(standard.end_of_file).unwrap_or(0),
        modified_ms: filetime_to_unix_ms(basic.write),
        links: standard.links,
        is_directory: standard.directory != 0,
    })
}

/// The entries of an open directory, `.` and `..` left out.
#[allow(
    clippy::cast_ptr_alignment,
    reason = "the buffer is 8-aligned and the file system puts every entry, and so every name, at an \n              even offset; the name units are copied out, never referenced"
)]
pub(super) fn list(handle: &OwnedHandle) -> Result<Vec<Entry>, Status> {
    #[repr(C)]
    struct Raw {
        next_entry_offset: u32,
        file_index: u32,
        creation: i64,
        access: i64,
        write: i64,
        change: i64,
        end_of_file: i64,
        allocation: i64,
        attributes: u32,
        name_length: u32,
        ea_size: u32,
        reparse_tag: u32,
        file_id: [u8; 16],
        name: [u16; 1],
    }

    let mut buffer = vec![0_u64; LIST_BUFFER_WORDS];
    let mut entries = Vec::new();
    let mut flags = RESTART_SCAN;
    loop {
        let mut block = IO_STATUS_BLOCK::default();
        // SAFETY: `buffer` is valid for writes of the length passed (and aligned to 8, as the
        // entries require); the handle is an open directory; `block` is a valid out pointer.
        let status = unsafe {
            NtQueryDirectoryFileEx(
                handle.raw(),
                None,
                None,
                None,
                &raw mut block,
                buffer.as_mut_ptr().cast::<c_void>(),
                u32::try_from(buffer.len() * size_of::<u64>()).unwrap_or(0),
                FileIdExtdDirectoryInformation,
                flags,
                None,
            )
        };
        flags = 0;
        if status.0 == STATUS_NO_MORE_FILES {
            return Ok(entries);
        }
        if status.0 != STATUS_SUCCESS {
            return Err(status.0);
        }
        let base = buffer.as_ptr().cast::<u8>();
        let mut offset = 0_usize;
        loop {
            // SAFETY: the file system wrote complete entries at 8-aligned offsets, each one
            // `next_entry_offset` after the previous, inside the buffer; `read_unaligned` copies the
            // fixed part and the name is read from the `name_length` bytes that follow it.
            let (raw, name) = unsafe {
                let entry = base.add(offset);
                let raw = entry.cast::<Raw>().read_unaligned();
                let name_at = entry.add(offset_of!(Raw, name)).cast::<u16>();
                let units = usize::try_from(raw.name_length / 2).unwrap_or(0);
                let name = std::slice::from_raw_parts(name_at, units).to_vec();
                (raw, name)
            };
            let dot = name == [u16::from(b'.')];
            let dot_dot = name == [u16::from(b'.'), u16::from(b'.')];
            if !dot && !dot_dot {
                entries.push(Entry {
                    name,
                    attributes: raw.attributes,
                    id_bytes: raw.file_id,
                    size: u64::try_from(raw.end_of_file).unwrap_or(0),
                    modified_ms: filetime_to_unix_ms(raw.write),
                });
            }
            if raw.next_entry_offset == 0 {
                break;
            }
            offset += usize::try_from(raw.next_entry_offset).unwrap_or(usize::MAX);
            if offset >= buffer.len() * size_of::<u64>() {
                return Err(STATUS_INVALID_PARAMETER);
            }
        }
    }
}

/// Marks the object of the handle deleted (POSIX semantics: the name goes at once, even while other
/// handles are open), ignoring the read-only attribute. A folder must be empty.
pub(super) fn delete(handle: &OwnedHandle) -> Result<(), Status> {
    let attempt = |flags: u32| -> Status {
        let info = flags;
        let mut block = IO_STATUS_BLOCK::default();
        // SAFETY: `info` is the 4-byte flags word the class expects, valid for the call; the handle
        // was opened with DELETE access; `block` is a valid out pointer.
        unsafe {
            NtSetInformationFile(
                handle.raw(),
                &raw mut block,
                (&raw const info).cast::<c_void>(),
                4,
                FileDispositionInformationEx,
            )
        }
        .0
    };
    let with_posix = FILE_DISPOSITION_DELETE.0
        | FILE_DISPOSITION_POSIX_SEMANTICS.0
        | FILE_DISPOSITION_IGNORE_READONLY_ATTRIBUTE.0;
    let mut status = attempt(with_posix);
    if status == STATUS_INVALID_PARAMETER {
        // A file system without POSIX semantics: an ordinary delete-on-close.
        status = attempt(FILE_DISPOSITION_DELETE.0 | FILE_DISPOSITION_IGNORE_READONLY_ATTRIBUTE.0);
    }
    if status == STATUS_SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}
