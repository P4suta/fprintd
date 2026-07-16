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
//!
//! # The trust boundary
//!
//! `<user>` arrives from a D-Bus caller (`device::resolve_user`), `<driver>` and `<device_id>`
//! from the backend. None of the three is a trusted file name, so [`component`] checks all
//! three: **`Path::join` replaces the base when the component is absolute, and `..` steps out
//! of the tree** — either would aim this module's root-privileged `mkdir`, write and unlink at
//! a path the caller picked. Reaching that through `<user>` requires the `setusername`
//! authorization, so it is an escalation from an authorized administrator to arbitrary root
//! write, not an unauthenticated hole. The store owns the layout, so the store decides what a
//! legal component is (ARCHITECTURE.md principle 3).
//!
//! The store follows symlinks: `<root>/eve -> <root>/alice` makes a save as `eve` overwrite
//! alice's print, which `a_symlinked_user_directory_is_followed` pins. Planting that link means
//! creating an entry in a root-owned `0700` tree, so the mitigation is that mode — set here and
//! pinned by `a_directory_is_created_0700` — and not `O_NOFOLLOW` on every open, which would
//! defend a root-owned directory against root.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

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

/// The one spelling this store accepts for a path component: a single, ordinary file name.
///
/// Everything else is refused — absolute paths, `.`, `..`, embedded separators, the empty
/// string, interior NUL. Each of those either escapes the store root or cannot be a file name.
fn component(s: &str) -> Result<&str, DaemonError> {
    let mut parts = Path::new(s).components();
    // `Components` yields `/` as `RootDir`, `.` as `CurDir` and `..` as `ParentDir`, so one
    // lone `Normal` is already none of those, and anything with a separator inside yields two.
    // What is left is that `Components` normalizes: `a/` parses as the single `Normal` `a`.
    // Demanding the component be spelled exactly as it parses is what refuses that.
    let legal = match (parts.next(), parts.next()) {
        (Some(Component::Normal(only)), None) => only.to_str() == Some(s),
        _ => false,
    };
    // A NUL parses as an ordinary `Normal` but cannot cross a syscall. Refusing it here makes
    // every rejection one `DaemonError`, not an `InvalidInput` from some later `open`.
    if !legal || s.contains('\0') {
        return Err(DaemonError::Internal(format!(
            "illegal path component {s:?}"
        )));
    }
    Ok(s)
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

    /// `<root>/<user>/<driver>/<device_id>`, or an error if any of the three is not a legal
    /// file name. Every path this store touches is built here, so this is the only place the
    /// check has to hold.
    fn dir(
        &self,
        user: &str,
        driver: &DriverId,
        device_id: &DeviceId,
    ) -> Result<PathBuf, DaemonError> {
        Ok(self
            .root
            .join(component(user)?)
            .join(component(driver.as_str())?)
            .join(component(device_id.as_str())?))
    }

    /// `<root>/<user>/<driver>/<device_id>/<hex-finger>`.
    fn print_path(
        &self,
        user: &str,
        driver: &DriverId,
        device_id: &DeviceId,
        finger: Finger,
    ) -> Result<PathBuf, DaemonError> {
        Ok(self
            .dir(user, driver, device_id)?
            .join(format!("{:x}", finger.as_u8())))
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

        let dir = self.dir(user, driver, device_id)?;
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
        let path = self.print_path(user, driver, device_id, finger).ok()?;
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
        let mut fingers = Vec::new();
        let Ok(dir) = self.dir(user, driver, device_id) else {
            return fingers;
        };
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
        let path = self.print_path(user, driver, device_id, finger)?;
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

    /// Spellings that must never reach the filesystem as a path component. Each one either
    /// escapes the store root, names something that is not a single directory, or cannot be a
    /// file name at all.
    const HOSTILE: &[&str] = &[
        "/tmp/pwned",
        "../../../../tmp/pwned",
        "a/../../b",
        "..",
        ".",
        "",
        "a/b",
        "al\0ice",
    ];

    fn ids() -> (DriverId, DeviceId) {
        (
            DriverId::new("virtual_image"),
            DeviceId::new("virtual_image"),
        )
    }

    /// A store rooted in a fresh temp directory, removed on the way in so repeat runs are clean.
    fn store(tag: &str) -> Store {
        let root =
            std::env::temp_dir().join(format!("fprintd-storage-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        Store::with_root(root)
    }

    fn print_for(user: &str, driver: &DriverId, finger: Finger, bytes_tag: &str) -> Print {
        let (_, device_id) = ids();
        let mut p = Print::new_for_enroll(finger);
        p.username = Some(user.to_string());
        p.driver = Some(driver.clone());
        p.device_id = Some(device_id);
        p.template = Template::Raw(bytes_tag.as_bytes().to_vec());
        p
    }

    fn print_of(bytes_tag: &str) -> Print {
        print_for(USER, &ids().0, Finger::RightIndex, bytes_tag)
    }

    /// Assert that `dir_of` refuses every hostile spelling, and — stated independently of the
    /// refusal, because it is the property the refusal exists to protect — that anything it
    /// does accept stays under `root`.
    fn assert_refuses_hostile(
        what: &str,
        root: &Path,
        mut dir_of: impl FnMut(&str) -> Result<PathBuf, DaemonError>,
    ) {
        for hostile in HOSTILE {
            let built = dir_of(hostile);
            if let Ok(path) = &built {
                assert!(
                    path.starts_with(root),
                    "{what} {hostile:?} escaped the store root: {}",
                    path.display()
                );
            }
            assert!(built.is_err(), "{what} {hostile:?} must be refused");
        }
    }

    /// `<user>` is caller-supplied, so this is the boundary that matters: `save` runs `mkdir`
    /// and an atomic write as root, and `delete` unlinks as root, all at `dir`'s result.
    #[test]
    fn a_hostile_username_cannot_escape_the_store_root() {
        let store = store("hostile-user");
        let (driver, device_id) = ids();
        let root = store.root.clone();

        assert_refuses_hostile("username", &root, |user| {
            store.dir(user, &driver, &device_id)
        });
        // `print_path` is the one `load` and `delete` open, so pin it too rather than trusting
        // that it still goes through `dir`.
        assert_refuses_hostile("username", &root, |user| {
            store.print_path(user, &driver, &device_id, Finger::RightIndex)
        });
    }

    /// `<driver>` and `<device_id>` come from the backend, which derives them from strings the
    /// device reports and from `FP_VIRTUAL_DEVICE`. Defence in depth: the store does not model
    /// its backend as trusted.
    #[test]
    fn a_hostile_driver_or_device_id_cannot_escape_the_store_root() {
        let store = store("hostile-ids");
        let (driver, device_id) = ids();
        let root = store.root.clone();

        assert_refuses_hostile("driver", &root, |d| {
            store.dir(USER, &DriverId::new(d), &device_id)
        });
        assert_refuses_hostile("device id", &root, |d| {
            store.dir(USER, &driver, &DeviceId::new(d))
        });
    }

    /// The directory layout is not by itself what separates one user's prints from another's:
    /// `load` re-checks the username the blob carries. Without that, a print planted in — or
    /// left behind in — the wrong directory would return as this user's.
    #[test]
    fn a_print_belonging_to_another_user_does_not_load() {
        let store = store("cross-user");
        let (driver, device_id) = ids();
        store
            .save(&print_for("alice", &driver, Finger::RightIndex, "alice's"))
            .expect("save alice");

        // Alice's blob, byte for byte, in bob's directory.
        let alice = store
            .print_path("alice", &driver, &device_id, Finger::RightIndex)
            .expect("alice's path");
        let bob = store
            .print_path("bob", &driver, &device_id, Finger::RightIndex)
            .expect("bob's path");
        fs::create_dir_all(bob.parent().expect("bob's dir")).expect("create bob's dir");
        fs::copy(&alice, &bob).expect("plant alice's print on bob");

        assert!(
            store
                .load("bob", &driver, &device_id, Finger::RightIndex)
                .is_none(),
            "a print naming alice must not load as bob's"
        );
        assert!(
            store
                .load("alice", &driver, &device_id, Finger::RightIndex)
                .is_some(),
            "the guard must reject the planted copy, not the format"
        );
    }

    /// The file name encodes the finger, and `load` re-checks it against the blob.
    #[test]
    fn a_print_stored_under_another_fingers_name_does_not_load() {
        let store = store("cross-finger");
        let (driver, device_id) = ids();
        store.save(&print_of("right index")).expect("save");

        let right = store
            .print_path(USER, &driver, &device_id, Finger::RightIndex)
            .expect("right index path");
        let left = store
            .print_path(USER, &driver, &device_id, Finger::LeftIndex)
            .expect("left index path");
        fs::copy(&right, &left).expect("plant it as the left index");

        assert!(
            store
                .load(USER, &driver, &device_id, Finger::LeftIndex)
                .is_none(),
            "a right-index print must not load as the left index"
        );
    }

    /// The directory encodes the driver, and `load` re-checks it: a template from one driver is
    /// not meaningful to another.
    #[test]
    fn a_print_from_another_driver_does_not_load() {
        let store = store("cross-driver");
        let (driver, device_id) = ids();
        let other = DriverId::new("upektc_img");
        store.save(&print_of("virtual")).expect("save");

        let src = store
            .print_path(USER, &driver, &device_id, Finger::RightIndex)
            .expect("source path");
        let dst = store
            .print_path(USER, &other, &device_id, Finger::RightIndex)
            .expect("other driver's path");
        fs::create_dir_all(dst.parent().expect("other driver's dir")).expect("create it");
        fs::copy(&src, &dst).expect("plant it under the other driver");

        assert!(
            store
                .load(USER, &other, &device_id, Finger::RightIndex)
                .is_none(),
            "a print naming another driver must not load"
        );
    }

    /// Every directory the store creates is `0700`. The file mode alone is not enough: a
    /// traversable tree lets another local user list which fingers a user enrolled.
    ///
    /// The root itself is not asserted — systemd's `StateDirectory` owns its mode, not this
    /// module.
    #[test]
    fn a_directory_is_created_0700() {
        let store = store("dir-mode");
        let (driver, device_id) = ids();
        store.save(&print_of("first")).expect("save");

        let user_dir = store.root.join(USER);
        let driver_dir = user_dir.join(driver.as_str());
        let device_dir = driver_dir.join(device_id.as_str());
        for level in [&user_dir, &driver_dir, &device_dir] {
            let mode = fs::metadata(level).expect("stat").permissions().mode() & 0o777;
            assert_eq!(
                mode,
                DIR_MODE,
                "{} must not be readable by other local users",
                level.display()
            );
        }
    }

    /// Deleting the last print prunes the emptied tree, and stops at the root: the store does
    /// not remove the state directory it was handed.
    #[test]
    fn delete_prunes_no_further_than_the_root() {
        let store = store("prune");
        let (driver, device_id) = ids();
        store.save(&print_of("only")).expect("save");

        store
            .delete(USER, &driver, &device_id, Finger::RightIndex)
            .expect("delete");

        assert!(
            !store.root.join(USER).exists(),
            "the emptied user directory must be pruned"
        );
        assert!(
            store.root.is_dir(),
            "the store root must survive its last print"
        );
    }

    /// The store follows symlinks — this pins that, it does not endorse it. `root/eve ->
    /// root/alice` makes a save as eve land on alice's print. Planting the link requires
    /// creating an entry in a root-owned `0700` tree, so the mode asserted by
    /// `a_directory_is_created_0700` is the mitigation; `O_NOFOLLOW` here would defend a
    /// root-owned directory against root.
    #[test]
    fn a_symlinked_user_directory_is_followed() {
        let store = store("symlink");
        let (driver, device_id) = ids();
        store
            .save(&print_for("alice", &driver, Finger::RightIndex, "alice's"))
            .expect("save alice");

        std::os::unix::fs::symlink(store.root.join("alice"), store.root.join("eve"))
            .expect("link eve at alice");
        store
            .save(&print_for("eve", &driver, Finger::RightIndex, "eve's"))
            .expect("save eve");

        let alice = store
            .print_path("alice", &driver, &device_id, Finger::RightIndex)
            .expect("alice's path");
        let landed = fprint_fp3::from_bytes(&fs::read(&alice).expect("read alice's file"))
            .expect("parse what landed there");
        assert_eq!(
            landed.username.as_deref(),
            Some("eve"),
            "the save followed the link onto alice's print"
        );
        assert!(
            store
                .load("alice", &driver, &device_id, Finger::RightIndex)
                .is_none(),
            "alice's print is gone; the username guard is what catches the overwrite"
        );
    }

    #[test]
    fn save_leaves_no_temp_file_and_is_0600() {
        let store = store("clean");
        let (driver, device_id) = ids();
        store.save(&print_of("first")).expect("save");

        let dir = store.dir(USER, &driver, &device_id).expect("dir");
        let names: Vec<String> = fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        // The rename consumed the temp file.
        assert_eq!(names, vec![format!("{:x}", Finger::RightIndex.as_u8())]);

        let path = store
            .print_path(USER, &driver, &device_id, Finger::RightIndex)
            .expect("print path");
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

        let dir = store.dir(USER, &driver, &device_id).expect("dir");
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

        let dir = store.dir(USER, &driver, &device_id).expect("dir");
        let name = format!("{:x}", Finger::RightIndex.as_u8());
        fs::write(dir.join(format!(".{name}.tmp")), b"debris").expect("leave debris");

        assert_eq!(
            store.list_fingers(USER, &driver, &device_id),
            vec![Finger::RightIndex]
        );
    }
}
