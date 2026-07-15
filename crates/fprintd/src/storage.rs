// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Host-side print storage — the `/var/lib/fprint` file layout.
//!
//! fprintd stores each enrolled print at
//! `<root>/<user>/<driver>/<device_id>/<hex-finger>`, where `<hex-finger>` is the
//! [`Finger`] discriminant printed as a single lowercase hex digit and the file body is an
//! FP3 blob. Both the layout and the hex-finger naming are interoperability facts taken from
//! fprintd's `file_storage.c`; this module reproduces them and delegates the byte format to
//! [`fprint_fp3`], keeping the wire quirks at the edge (ARCHITECTURE.md principle 3).
//!
//! `<root>` follows systemd's `STATE_DIRECTORY` (first entry of a colon-separated list) and
//! falls back to `/var/lib/fprint`, exactly as upstream.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use fprint_core::{DeviceId, DriverId, Finger, Print};

use crate::error::DaemonError;

/// Directory permissions for the per-user store (`0700`), matching `file_storage.c`.
const DIR_MODE: u32 = 0o700;
/// Print-file permissions (`0600`): only the daemon (root) may read enrolled templates.
const FILE_MODE: u32 = 0o600;

/// Write `bytes` to `path` atomically: a sibling temp file, `fsync`ed, then `rename`d over the
/// target. A crash leaves either the old print or the new one, never a truncated one.
///
/// This matters because upstream fprintd reads the same store, and its writer
/// (`file_storage.c`'s `g_file_set_contents`) is atomic; a truncate-and-write here would leave
/// a corrupt print for either daemon to read.
///
/// The temp file is a sibling so the rename cannot cross a filesystem, and its name cannot be
/// mistaken for a print — [`Store::list_fingers`] only accepts single-character names.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let (dir, name) = match (path.parent(), path.file_name().and_then(|n| n.to_str())) {
        (Some(dir), Some(name)) => (dir, name),
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "print path has no parent directory or no file name",
            ))
        }
    };
    let tmp = dir.join(format!(".{name}.tmp"));

    let write = || -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            // Created 0600, so a template is never briefly world-readable. umask can only clear
            // bits, so this fails closed.
            .mode(FILE_MODE)
            .open(&tmp)?;
        file.write_all(bytes)?;
        // `rename` orders metadata, not contents: without this a crash can leave the directory
        // entry in place and the file empty.
        file.sync_all()
    };

    let result = write().and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// The on-disk print store rooted at a state directory.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Build a store from the environment: `STATE_DIRECTORY` (first colon-separated path) or
    /// `/var/lib/fprint`.
    pub fn from_env() -> Store {
        let root = match std::env::var("STATE_DIRECTORY") {
            Ok(v) => v
                .split(':')
                .next()
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            Err(_) => None,
        }
        .unwrap_or_else(|| PathBuf::from("/var/lib/fprint"));
        Store { root }
    }

    /// Build a store rooted at an explicit path (used by tests).
    pub fn with_root(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    /// `<root>/<user>/<driver>/<device_id>`.
    fn dir(&self, user: &str, driver: &DriverId, device_id: &DeviceId) -> PathBuf {
        self.root.join(user).join(&driver.0).join(&device_id.0)
    }

    /// `<root>/<user>/<driver>/<device_id>/<hex-finger>`.
    fn print_path(
        &self,
        user: &str,
        driver: &DriverId,
        device_id: &DeviceId,
        finger: Finger,
    ) -> PathBuf {
        self.dir(user, driver, device_id)
            .join(format!("{:x}", finger.as_u8()))
    }

    /// Serialize `print` to FP3 and write it to its canonical path (creating the directory
    /// tree with `0700`). The print must carry `username`, `driver`, `device_id` and
    /// `finger`; enrolment fills these in before calling.
    pub fn save(&self, print: &Print) -> Result<(), DaemonError> {
        let user = print
            .username
            .as_deref()
            .ok_or_else(|| DaemonError::Internal("print has no username".into()))?;
        let driver = print
            .driver
            .as_ref()
            .ok_or_else(|| DaemonError::Internal("print has no driver".into()))?;
        let device_id = print
            .device_id
            .as_ref()
            .ok_or_else(|| DaemonError::Internal("print has no device id".into()))?;
        let finger = print
            .finger
            .ok_or_else(|| DaemonError::Internal("print has no finger".into()))?;

        let bytes = fprint_fp3::to_bytes(print)
            .map_err(|e| DaemonError::Internal(format!("FP3 serialize failed: {e}")))?;

        let dir = self.dir(user, driver, device_id);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(&dir)
            .map_err(|e| DaemonError::Internal(format!("mkdir {}: {e}", dir.display())))?;

        let path = dir.join(format!("{:x}", finger.as_u8()));
        write_atomically(&path, &bytes)
            .map_err(|e| DaemonError::Internal(format!("write {}: {e}", path.display())))?;
        Ok(())
    }

    /// Load one print, or `None` if it is absent, unreadable, or fails the same
    /// finger/username/driver compatibility checks fprintd performs on load.
    pub fn load(
        &self,
        user: &str,
        driver: &DriverId,
        device_id: &DeviceId,
        finger: Finger,
    ) -> Option<Print> {
        let path = self.print_path(user, driver, device_id, finger);
        let bytes = fs::read(&path).ok()?;
        let print = fprint_fp3::from_bytes(&bytes).ok()?;

        if print.finger != Some(finger) {
            return None;
        }
        if print.username.as_deref() != Some(user) {
            return None;
        }
        if !print.is_compatible_with_driver(driver) {
            return None;
        }
        Some(print)
    }

    /// The fingers enrolled for `user` on this device: entries whose name is a single hex
    /// digit naming a valid real finger (`1..=10`).
    pub fn list_fingers(&self, user: &str, driver: &DriverId, device_id: &DeviceId) -> Vec<Finger> {
        let dir = self.dir(user, driver, device_id);
        let mut fingers = Vec::new();
        let Ok(entries) = fs::read_dir(&dir) else {
            return fingers;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.len() != 1 {
                continue;
            }
            let Ok(byte) = u8::from_str_radix(name, 16) else {
                continue;
            };
            match Finger::from_u8(byte) {
                Some(f) if f != Finger::Unknown => fingers.push(f),
                _ => {}
            }
        }
        fingers
    }

    /// Remove one print, then prune now-empty parent directories up to (but not including)
    /// the store root — mirroring `file_storage_print_data_delete`.
    pub fn delete(
        &self,
        user: &str,
        driver: &DriverId,
        device_id: &DeviceId,
        finger: Finger,
    ) -> Result<(), DaemonError> {
        let path = self.print_path(user, driver, device_id, finger);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(DaemonError::PrintsNotDeleted(format!(
                    "unlink {}: {e}",
                    path.display()
                )))
            }
        }

        // Prune empty directories walking up toward the root.
        let mut dir = path.parent().map(Path::to_path_buf);
        while let Some(d) = dir {
            if d == self.root || !d.starts_with(&self.root) {
                break;
            }
            if fs::remove_dir(&d).is_err() {
                break; // non-empty or gone; stop pruning
            }
            dir = d.parent().map(Path::to_path_buf);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fprint_core::Template;
    use std::os::unix::fs::PermissionsExt;

    const USER: &str = "tester";

    fn ids() -> (DriverId, DeviceId) {
        (
            DriverId("virtual_image".into()),
            DeviceId("virtual_image".into()),
        )
    }

    /// A store rooted in a fresh temp directory, removed on the way in so repeat runs are clean.
    fn store(tag: &str) -> Store {
        let root =
            std::env::temp_dir().join(format!("fprintd-storage-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        Store::with_root(root)
    }

    fn print_of(bytes_tag: &str) -> Print {
        let (driver, device_id) = ids();
        let mut p = Print::new_for_enroll(Finger::RightIndex);
        p.username = Some(USER.to_string());
        p.driver = Some(driver);
        p.device_id = Some(device_id);
        p.template = Template::Raw(bytes_tag.as_bytes().to_vec());
        p
    }

    #[test]
    fn save_leaves_no_temp_file_and_is_0600() {
        let store = store("clean");
        let (driver, device_id) = ids();
        store.save(&print_of("first")).expect("save");

        let dir = store.dir(USER, &driver, &device_id);
        let names: Vec<String> = fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        // The rename consumed the temp file.
        assert_eq!(names, vec![format!("{:x}", Finger::RightIndex.as_u8())]);

        let path = store.print_path(USER, &driver, &device_id, Finger::RightIndex);
        let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, FILE_MODE, "a template must not be readable by others");
    }

    /// A failed save must not destroy what was already enrolled. This is the observable half of
    /// what `write_atomically` guarantees; the crash-mid-write case cannot be provoked from a
    /// unit test. Failure is forced with a directory on the temp path, which root cannot open
    /// as a file either.
    #[test]
    fn a_failed_write_leaves_the_previous_print_intact() {
        let store = store("survives");
        let (driver, device_id) = ids();
        store.save(&print_of("original")).expect("first save");

        let dir = store.dir(USER, &driver, &device_id);
        let name = format!("{:x}", Finger::RightIndex.as_u8());
        fs::create_dir(dir.join(format!(".{name}.tmp"))).expect("block the temp path");

        store
            .save(&print_of("replacement"))
            .expect_err("the write must fail, not silently half-succeed");

        // The original is still there, still whole, still loadable.
        let loaded = store
            .load(USER, &driver, &device_id, Finger::RightIndex)
            .expect("the previously enrolled print must survive a failed overwrite");
        assert_eq!(loaded.template, Template::Raw(b"original".to_vec()));
    }

    /// The temp file's name is longer than one character, so `list_fingers` cannot see it. That
    /// is what makes a sibling temp file safe here, so pin it.
    #[test]
    fn a_stale_temp_file_is_not_mistaken_for_a_finger() {
        let store = store("stale");
        let (driver, device_id) = ids();
        store.save(&print_of("first")).expect("save");

        let dir = store.dir(USER, &driver, &device_id);
        let name = format!("{:x}", Finger::RightIndex.as_u8());
        fs::write(dir.join(format!(".{name}.tmp")), b"debris").expect("leave debris");

        assert_eq!(
            store.list_fingers(USER, &driver, &device_id),
            vec![Finger::RightIndex]
        );
    }
}
