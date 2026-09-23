//! File moves that never overwrite.
//!
//! A same-volume move uses the operating system's "rename unless the
//! target exists" call. Across volumes the file is copied to a temporary
//! name next to the target, flushed, verified, renamed into place with the
//! same no-replace call, and only then is the source removed.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Temporary suffix for copies in progress. Recovery removes stale ones.
pub const PART_SUFFIX: &str = ".cdpart";

pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(PART_SUFFIX);
    dest.with_file_name(name)
}

/// Rename `src` to `dst`, failing with `AlreadyExists` if `dst` exists.
#[cfg(target_vendor = "apple")]
pub fn rename_no_replace(src: &Path, dst: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let s = CString::new(src.as_os_str().as_bytes())?;
    let d = CString::new(dst.as_os_str().as_bytes())?;
    // SAFETY: both arguments are valid NUL-terminated paths.
    let rc = unsafe { libc::renamex_np(s.as_ptr(), d.as_ptr(), libc::RENAME_EXCL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn rename_no_replace(src: &Path, dst: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let s = CString::new(src.as_os_str().as_bytes())?;
    let d = CString::new(dst.as_os_str().as_bytes())?;
    // SAFETY: both arguments are valid NUL-terminated paths.
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            s.as_ptr(),
            libc::AT_FDCWD,
            d.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        // Filesystem without RENAME_NOREPLACE support: a hard link fails if
        // the target exists, so link then unlink gives the same guarantee.
        Some(libc::EINVAL) | Some(libc::ENOSYS) => {
            std::fs::hard_link(src, dst)?;
            std::fs::remove_file(src)
        }
        _ => Err(err),
    }
}

#[cfg(windows)]
pub fn rename_no_replace(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(std::iter::once(0)).collect() };
    let (s, d) = (wide(src), wide(dst));
    // No MOVEFILE_REPLACE_EXISTING: fails if the target exists. No
    // MOVEFILE_COPY_ALLOWED: cross-volume moves fail and take the copy path.
    // SAFETY: both arguments are valid NUL-terminated UTF-16 paths.
    let ok = unsafe { MoveFileExW(s.as_ptr(), d.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if ok != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android", windows)))]
pub fn rename_no_replace(src: &Path, dst: &Path) -> io::Result<()> {
    std::fs::hard_link(src, dst)?;
    std::fs::remove_file(src)
}

fn is_cross_device(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::CrossesDevices {
        return true;
    }
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(libc::EXDEV)
    }
    #[cfg(windows)]
    {
        // ERROR_NOT_SAME_DEVICE
        e.raw_os_error() == Some(17)
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

pub fn hash_file(path: &Path) -> io::Result<blake3::Hash> {
    let mut hasher = blake3::Hasher::new();
    let mut f = File::open(path)?;
    io::copy(&mut f, &mut hasher)?;
    Ok(hasher.finalize())
}

fn sync_dir(dir: &Path) {
    // Directory fsync makes the rename durable on Unix. Not available (or
    // needed) on Windows.
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Copy `src` to `dst` via a temporary file, verifying the bytes. The
/// source is left in place. Fails if `dst` exists.
pub fn copy_verified(src: &Path, dst: &Path) -> io::Result<()> {
    let part = part_path(dst);
    let _ = std::fs::remove_file(&part);
    {
        let mut out = OpenOptions::new().write(true).create_new(true).open(&part)?;
        let mut input = File::open(src)?;
        io::copy(&mut input, &mut out)?;
        out.sync_all()?;
    }
    if hash_file(src)? != hash_file(&part)? {
        let _ = std::fs::remove_file(&part);
        return Err(io::Error::other("copy verification failed: bytes differ"));
    }
    if let Err(e) = rename_no_replace(&part, dst) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if let Some(parent) = dst.parent() {
        sync_dir(parent);
    }
    Ok(())
}

/// How a move was done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moved {
    /// Renamed on the same volume; the source is gone.
    Renamed,
    /// Copied and verified; the source still exists and must be removed by
    /// the caller once the new location is recorded.
    Copied,
}

/// Move without ever overwriting `dst`. `force_copy` exercises the
/// cross-volume path in tests.
pub fn move_no_replace(src: &Path, dst: &Path, force_copy: bool) -> io::Result<Moved> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !force_copy {
        match rename_no_replace(src, dst) {
            Ok(()) => {
                if let Some(parent) = dst.parent() {
                    sync_dir(parent);
                }
                return Ok(Moved::Renamed);
            }
            Err(e) if is_cross_device(&e) => {}
            Err(e) => return Err(e),
        }
    }
    copy_verified(src, dst)?;
    Ok(Moved::Copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"precious").unwrap();
        let err = rename_no_replace(&a, &b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&b).unwrap(), b"precious");
        assert_eq!(std::fs::read(&a).unwrap(), b"new");
    }

    #[test]
    fn same_volume_move_renames() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::write(&a, b"x").unwrap();
        let dst = dir.path().join("sub/dir/b");
        assert_eq!(move_no_replace(&a, &dst, false).unwrap(), Moved::Renamed);
        assert!(!a.exists());
        assert_eq!(std::fs::read(&dst).unwrap(), b"x");
    }

    #[test]
    fn copy_path_verifies_and_keeps_source_until_told() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::write(&a, vec![7u8; 100_000]).unwrap();
        let dst = dir.path().join("b");
        assert_eq!(move_no_replace(&a, &dst, true).unwrap(), Moved::Copied);
        assert!(a.exists());
        assert_eq!(hash_file(&a).unwrap(), hash_file(&dst).unwrap());
        assert!(!part_path(&dst).exists());
    }

    #[test]
    fn copy_path_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"precious").unwrap();
        assert!(move_no_replace(&a, &b, true).is_err());
        assert_eq!(std::fs::read(&b).unwrap(), b"precious");
        assert!(!part_path(&b).exists());
    }
}
