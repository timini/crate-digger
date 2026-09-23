//! Installing slskd on first use: a pinned release for this platform,
//! checked against its SHA-256 before anything is unpacked.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

pub const VERSION: &str = "0.26.0";
pub const SOURCE: &str = "https://github.com/slskd/slskd";
pub const LICENCE: &str = "AGPL-3.0";

pub struct Release {
    pub asset: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// Digests as published by GitHub for the 0.26.0 release assets.
const RELEASES: &[(&str, &str, Release)] = &[
    (
        "macos",
        "aarch64",
        Release {
            asset: "slskd-0.26.0-osx-arm64.zip",
            sha256: "53bd82e26224908abb30780f3e3a3ee58788d17379354b3138c85c6fe02cd5a0",
            size: 58_309_223,
        },
    ),
    (
        "macos",
        "x86_64",
        Release {
            asset: "slskd-0.26.0-osx-x64.zip",
            sha256: "3d624c53de73229caa090c395ee5eada9c7f54d59fd3a0e79a2597e8b467b448",
            size: 60_596_634,
        },
    ),
    (
        "linux",
        "x86_64",
        Release {
            asset: "slskd-0.26.0-linux-x64.zip",
            sha256: "9c19c04767ef036a47716404d097433e23fbdb41b339e0e8cbc2329c98b22583",
            size: 59_935_118,
        },
    ),
    (
        "linux",
        "aarch64",
        Release {
            asset: "slskd-0.26.0-linux-arm64.zip",
            sha256: "57d4b9dbb0ad34aa6e6aaaf79b0bf3347dec5efcb48ad2b12f9ed4ece42787aa",
            size: 57_786_451,
        },
    ),
    (
        "windows",
        "x86_64",
        Release {
            asset: "slskd-0.26.0-win-x64.zip",
            sha256: "942299d8c97da6cc1f6cd82dcd4a3662b97b82fbd1742df4bec165b79357268a",
            size: 60_777_709,
        },
    ),
    (
        "windows",
        "aarch64",
        Release {
            asset: "slskd-0.26.0-win-arm64.zip",
            sha256: "707ca26de16835d83ff980d9fee5a2c2fd8ec7bf8260be63d60805914a627e9d",
            size: 58_897_722,
        },
    ),
];

pub fn release() -> Option<&'static Release> {
    RELEASES
        .iter()
        .find(|(os, arch, _)| *os == std::env::consts::OS && *arch == std::env::consts::ARCH)
        .map(|(_, _, r)| r)
}

pub fn url(r: &Release) -> String {
    format!("{SOURCE}/releases/download/{VERSION}/{}", r.asset)
}

pub fn version_dir(root: &Path) -> PathBuf {
    root.join("versions").join(VERSION)
}

pub fn binary(root: &Path) -> PathBuf {
    version_dir(root).join(if cfg!(windows) { "slskd.exe" } else { "slskd" })
}

pub fn installed(root: &Path) -> bool {
    binary(root).is_file()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Unpacks a verified archive into `dest`, refusing entries that would
/// land outside it.
pub fn unpack(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("The slskd archive is unreadable: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(relative) = entry.enclosed_name() else {
            return Err("The slskd archive contains an unsafe path, so it was not installed.".into());
        };
        let out = dest.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut w).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o755))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Verify a downloaded archive and install it. The previous install, if
/// any, is replaced only after the new one unpacked completely.
pub fn install_archive(root: &Path, zip_path: &Path, r: &Release) -> Result<PathBuf, String> {
    let sum = sha256_file(zip_path).map_err(|e| e.to_string())?;
    if sum != r.sha256 {
        let _ = std::fs::remove_file(zip_path);
        return Err(format!(
            "The slskd download does not match the pinned checksum, so it was discarded. This version of Crate \
             Digger only accepts slskd {VERSION} ({}...).",
            &r.sha256[..12]
        ));
    }
    let target = version_dir(root);
    let staging = target.with_extension("partial");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    unpack(zip_path, &staging)?;
    let exe = staging.join(binary(root).file_name().unwrap());
    if !exe.is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("The slskd archive has no slskd program in it.".into());
    }
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(zip_path);
    Ok(binary(root))
}

/// Download this platform's pinned release and install it.
pub fn download(root: &Path, progress: &AtomicU64) -> Result<PathBuf, String> {
    let r = release().ok_or("slskd has no release for this platform. Point Settings at an slskd you run.")?;
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let part = root.join(format!("{}.part", r.asset));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .build()
        .into();
    let response = agent.get(&url(r)).call().map_err(|e| {
        format!("Downloading slskd failed: {e}. Check your internet connection and try again.")
    })?;
    let mut reader = response.into_body().into_reader();
    let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("The slskd download was interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        // The pinned size is known, so a longer response is not the release.
        if total > r.size {
            let _ = std::fs::remove_file(&part);
            return Err("The slskd download is larger than the pinned release, so it was discarded.".into());
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        progress.store(total, Ordering::Relaxed);
    }
    out.sync_all().map_err(|e| e.to_string())?;
    drop(out);
    install_archive(root, &part, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            for (name, data) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    fn release_for(bytes: &[u8]) -> Release {
        let sum = hex(&Sha256::digest(bytes));
        Release {
            asset: "test.zip",
            sha256: Box::leak(sum.into_boxed_str()),
            size: bytes.len() as u64,
        }
    }

    #[test]
    fn every_platform_pin_is_a_sha256() {
        for (_, _, r) in RELEASES {
            assert_eq!(r.sha256.len(), 64);
            assert!(r.asset.contains(VERSION));
        }
        assert!(url(&RELEASES[0].2).starts_with("https://github.com/slskd/slskd/releases/download/0.26.0/"));
    }

    #[test]
    fn verified_archive_installs_and_replaces_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) { "slskd.exe" } else { "slskd" };
        let bytes = zip_with(&[(exe, b"binary"), ("wwwroot/index.html", b"<html>")]);
        let zip_path = dir.path().join("a.zip");
        std::fs::write(&zip_path, &bytes).unwrap();
        std::fs::create_dir_all(version_dir(dir.path())).unwrap();
        std::fs::write(version_dir(dir.path()).join("stale"), b"x").unwrap();
        let bin = install_archive(dir.path(), &zip_path, &release_for(&bytes)).unwrap();
        assert!(installed(dir.path()));
        assert_eq!(std::fs::read(&bin).unwrap(), b"binary");
        assert!(!version_dir(dir.path()).join("stale").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&bin).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }
    }

    #[test]
    fn wrong_checksum_or_unsafe_paths_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = zip_with(&[("slskd", b"binary")]);
        let zip_path = dir.path().join("a.zip");
        std::fs::write(&zip_path, &bytes).unwrap();
        let mut wrong = release_for(b"something else");
        wrong.size = bytes.len() as u64;
        assert!(install_archive(dir.path(), &zip_path, &wrong)
            .unwrap_err()
            .contains("checksum"));
        assert!(!zip_path.exists(), "a mismatched download is deleted");
        assert!(!installed(dir.path()));

        let evil = zip_with(&[("../escaped", b"x")]);
        std::fs::write(&zip_path, &evil).unwrap();
        let err = install_archive(dir.path(), &zip_path, &release_for(&evil)).unwrap_err();
        assert!(err.contains("unsafe path"), "{err}");
        assert!(!dir.path().join("versions/escaped").exists());
    }
}
