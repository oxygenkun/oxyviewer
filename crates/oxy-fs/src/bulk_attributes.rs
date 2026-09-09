//! Darwin's directory attribute batches avoid a separate stat syscall per photo.
//! Unsupported filesystems fall back to the portable complete scanner. Symlinks
//! and attributes not supplied by the filesystem retain ordinary stat semantics.

use super::{
    AssetSummary, FsError, ScanProgress, sidecar_key, sidecar_path, summary_from_attributes,
    summary_from_metadata,
};
use oxy_domain::AssetKind;
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs::{self, File},
    io,
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    path::Path,
    time::Instant,
};

// sys/attr.h: ERROR is packed immediately after RETURNED_ATTRS, before NAME.
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;
// File attributes are omitted for directories even with PACK_INVAL_ATTRS.
const HEADER_SIZE: usize = 56;
const COMMON: u32 = libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_RETURNED_ATTRS
    | ATTR_CMN_ERROR;

pub(super) fn scan(
    root: &Path,
    report: &mut impl FnMut(ScanProgress),
) -> Result<Option<Vec<AssetSummary>>, FsError> {
    let directory = File::open(root)?;
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: COMMON,
        volattr: 0,
        dirattr: 0,
        fileattr: libc::ATTR_FILE_DATALENGTH,
        forkattr: 0,
    };
    // u64 storage meets Darwin's 8-byte record alignment requirement.
    let mut buffer = vec![0_u64; 128 * 1024 / 8];
    let started = Instant::now();
    let mut progress = ScanProgress::default();
    report(progress);
    let mut assets = Vec::new();
    let mut sidecars = HashSet::new();
    loop {
        // SAFETY: directory owns a readable fd, attributes is fully initialized,
        // and the aligned output allocation has exactly the supplied byte size.
        let count = unsafe {
            libc::getattrlistbulk(
                directory.as_raw_fd(),
                std::ptr::from_mut(&mut attributes).cast(),
                buffer.as_mut_ptr().cast(),
                buffer.len() * 8,
                u64::from(libc::FSOPT_PACK_INVAL_ATTRS),
            )
        };
        if count < 0 {
            let error = io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(libc::ENOTSUP | libc::ENOSYS | libc::EINVAL)
            ) {
                return Ok(None);
            }
            return Err(error.into());
        }
        if count == 0 {
            break;
        }
        // SAFETY: every byte in the u64 allocation was initialized; the slice
        // is borrowed only until the next syscall mutates this buffer.
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 8) };
        let mut remaining = bytes;
        for _ in 0..count {
            let length = u32_at(remaining, 0)? as usize;
            if length < HEADER_SIZE || length % 8 != 0 || length > remaining.len() {
                return Err(invalid_record().into());
            }
            let (record, rest) = remaining.split_at(length);
            remaining = rest;
            let returned_common = u32_at(record, 4)?;
            let returned_file = u32_at(record, 16)?;
            let entry_error = u32_at(record, 24)?;
            if entry_error == libc::ENOENT as u32 {
                continue;
            }
            if returned_common & libc::ATTR_CMN_NAME == 0 {
                if entry_error != 0 {
                    return Err(io::Error::from_raw_os_error(entry_error as i32).into());
                }
                return Ok(None);
            }
            let name = record_name(record)?;
            let path = root.join(name);
            let extension = path.extension();
            let is_sidecar = extension.is_some_and(|ext| ext.eq_ignore_ascii_case("xmp"));
            if !is_sidecar
                && extension
                    .and_then(OsStr::to_str)
                    .and_then(AssetKind::from_extension)
                    .is_none()
            {
                continue;
            }
            let object_type = u32_at(record, 36)?;
            if returned_common & libc::ATTR_CMN_OBJTYPE != 0 && object_type == VDIR {
                continue;
            }
            let required_common = libc::ATTR_CMN_OBJTYPE | libc::ATTR_CMN_MODTIME;
            let needs_stat = entry_error != 0
                || returned_common & required_common != required_common
                || (returned_file & libc::ATTR_FILE_DATALENGTH == 0 && !is_sidecar)
                || object_type == VLNK;
            let summary = if needs_stat {
                let metadata = match fs::metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                if !metadata.is_file() {
                    continue;
                }
                if is_sidecar {
                    sidecars.insert(sidecar_key(&path));
                    continue;
                }
                summary_from_metadata(&path, &metadata, false)?
            } else {
                if object_type != VREG {
                    continue;
                }
                if is_sidecar {
                    sidecars.insert(sidecar_key(&path));
                    continue;
                }
                let seconds = i64_at(record, 40)?;
                let nanos = i64_at(record, 48)?;
                let length = i64_at(record, 56)?;
                if !(0..1_000_000_000).contains(&nanos) || length < 0 {
                    return Err(invalid_record().into());
                }
                let modified_ms = (i128::from(seconds) * 1000 + i128::from(nanos) / 1_000_000)
                    .clamp(0, i128::from(u64::MAX)) as u64;
                summary_from_attributes(&path, length as u64, modified_ms, false)?
            };
            if let Some(summary) = summary {
                assets.push(summary);
            }
        }
        progress.discovered_count = assets.len();
        // Darwin obtains enumeration and attributes in the same syscall.
        progress.enumeration_ms = started.elapsed().as_millis() as u64;
        report(progress);
    }
    progress.reading_attributes = true;
    progress.enumeration_ms = started.elapsed().as_millis() as u64;
    report(progress);
    let pairing_started = Instant::now();
    for asset in &mut assets {
        asset.has_sidecar = sidecars.contains(&sidecar_key(&sidecar_path(&asset.path)));
    }
    progress.attributes_ms = pairing_started.elapsed().as_millis() as u64;
    progress.discovered_count = assets.len();
    report(progress);
    Ok(Some(assets))
}

fn record_name(record: &[u8]) -> io::Result<&OsStr> {
    let relative = i32::from_ne_bytes(
        record
            .get(28..32)
            .ok_or_else(invalid_record)?
            .try_into()
            .unwrap(),
    );
    let start = 28_i64
        .checked_add(i64::from(relative))
        .ok_or_else(invalid_record)?;
    let length = u32_at(record, 32)? as usize;
    let start = usize::try_from(start).map_err(|_| invalid_record())?;
    let end = start.checked_add(length).ok_or_else(invalid_record)?;
    if start < HEADER_SIZE || length < 2 {
        return Err(invalid_record());
    }
    let bytes = record.get(start..end).ok_or_else(invalid_record)?;
    let Some((&0, name)) = bytes.split_last() else {
        return Err(invalid_record());
    };
    if name.contains(&0) || name.contains(&b'/') || name == b"." || name == b".." {
        return Err(invalid_record());
    }
    Ok(OsStr::from_bytes(name))
}

fn u32_at(bytes: &[u8], offset: usize) -> io::Result<u32> {
    Ok(u32::from_ne_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(invalid_record)?
            .try_into()
            .unwrap(),
    ))
}

fn i64_at(bytes: &[u8], offset: usize) -> io::Result<i64> {
    Ok(i64::from_ne_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or_else(invalid_record)?
            .try_into()
            .unwrap(),
    ))
}

fn invalid_record() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid Darwin bulk attribute record",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::FileTimes,
        os::unix::fs::symlink,
        time::{Duration, UNIX_EPOCH},
    };

    #[test]
    fn bulk_and_portable_scans_have_identical_complete_attributes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for (name, bytes) in [
            ("paired.CR3", b"raw".as_slice()),
            ("paired.XMP", b"sidecar"),
            ("b.jpg", b"jpeg"),
            ("ignored.txt", b"text"),
        ] {
            fs::write(root.join(name), bytes).unwrap();
        }
        let stamp = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789);
        File::options()
            .write(true)
            .open(root.join("b.jpg"))
            .unwrap()
            .set_times(FileTimes::new().set_modified(stamp))
            .unwrap();
        symlink(root.join("b.jpg"), root.join("alias.jpg")).unwrap();
        symlink(root.join("missing.jpg"), root.join("broken.jpg")).unwrap();
        fs::create_dir(root.join("nested.png")).unwrap();
        fs::write(root.join("nested.png/ignored.jpg"), b"nested").unwrap();
        let mut bulk = scan(root, &mut |_| {})
            .unwrap()
            .expect("APFS bulk attributes");
        let mut portable = super::super::scan_assets_portable(root, |_| {}).unwrap();
        bulk.sort_by(|a, b| a.path.cmp(&b.path));
        portable.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(bulk, portable);
        assert!(
            bulk.iter()
                .find(|a| a.name == "paired.CR3")
                .unwrap()
                .has_sidecar
        );
        assert_eq!(
            bulk.iter()
                .find(|a| a.name == "b.jpg")
                .unwrap()
                .modified_at_ms,
            1_700_000_000_123
        );
    }

    #[test]
    fn invalid_name_references_never_escape_the_record() {
        let mut record = [0_u8; 72];
        record[28..32].copy_from_slice(&36_i32.to_ne_bytes());
        record[32..36].copy_from_slice(&6_u32.to_ne_bytes());
        record[64..70].copy_from_slice(b"a.jpg\0");
        assert_eq!(record_name(&record).unwrap(), "a.jpg");
        record[28..32].copy_from_slice(&i32::MAX.to_ne_bytes());
        assert!(record_name(&record).is_err());
        record[28..32].copy_from_slice(&(-28_i32).to_ne_bytes());
        assert!(record_name(&record).is_err());
    }
}
