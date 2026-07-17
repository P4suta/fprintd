// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Running things in containers, without going through a shell.
//!
//! No argument built here is parsed by `sh`, which keeps a bind-mount path portable: a path handed
//! to [`std::process::Command`] reaches Docker as written, on any host.
//!
//! Two shapes, because `docker run --rm` keeps nothing:
//!
//! * [`Run`] — one command in one container, discarded afterwards.
//! * [`Session`] — a container held open so several commands can build on each other. Each step
//!   is its own process with its own exit status and its own stderr, so a failure says which
//!   step failed.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the repository is mounted inside every container this module starts.
pub const WORK: &str = "/work";

/// What a command in a container said.
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// stdout with trailing whitespace removed — the common case for a one-line answer.
    pub fn line(&self) -> &str {
        self.stdout.trim()
    }
}

/// A container invocation with the repository mounted at [`WORK`].
pub struct Run {
    image: &'static str,
    env: Vec<(String, String)>,
    argv: Vec<String>,
}

impl Run {
    pub fn new(image: &'static str) -> Self {
        Run {
            image,
            env: Vec::new(),
            argv: Vec::new(),
        }
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.argv.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.argv.extend(args.into_iter().map(Into::into));
        self
    }

    /// Run it with `root` mounted at [`WORK`], returning stdout.
    ///
    /// stderr is passed through to the terminal rather than captured: nothing here parses it.
    pub fn output(self, root: &Path) -> Result<String, String> {
        let mut cmd = Command::new("docker");
        cmd.arg("run").arg("--rm");
        cmd.arg("-v").arg(format!("{}:{WORK}", root.display()));
        cmd.arg("-w").arg(WORK);
        for (k, v) in &self.env {
            cmd.arg("-e").arg(format!("{k}={v}"));
        }
        cmd.arg(self.image);
        cmd.args(&self.argv);

        let output = cmd
            .output()
            .map_err(|e| format!("spawn docker: {e} (is Docker running?)"))?;

        // Show the container's stderr as it would have appeared, then judge by status.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            eprint!("{stderr}");
        }
        if !output.status.success() {
            return Err(format!(
                "`{} {}` failed in {} ({})",
                self.argv.first().map(String::as_str).unwrap_or("?"),
                self.argv
                    .iter()
                    .skip(1)
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" "),
                self.image,
                output.status
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// A container held open, so a sequence of commands can share its filesystem.
///
/// Removed on drop, so a failure leaves nothing behind for the next run.
pub struct Session {
    name: String,
}

impl Session {
    /// Start `image` doing nothing, under a name unique to this process.
    ///
    /// `sleep` rather than the image's entrypoint: the task needs only a filesystem to work in. The
    /// duration is a timeout, not a schedule — the container is removed when the task is done, and
    /// the ceiling matters only if the task is killed.
    pub fn start(image: &str) -> Result<Session, String> {
        let name = format!("fprintd-xtask-{}", std::process::id());
        // A leftover from a killed run would collide; clear it without caring if it is there.
        let _ = Command::new("docker").args(["rm", "-f", &name]).output();

        let out = Command::new("docker")
            .args(["run", "-d", "--name", &name, image, "sleep", "600"])
            .output()
            .map_err(|e| format!("spawn docker: {e} (is Docker running?)"))?;
        if !out.status.success() {
            return Err(format!(
                "could not start {image}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(Session { name })
    }

    /// Run one command in the container. `Err` if it could not be run or exited non-zero.
    pub fn exec(&self, argv: &[&str]) -> Result<Output, String> {
        let (ok, out) = self.try_exec(argv)?;
        if !ok {
            let mut msg = format!("`{}` failed in the container", argv.join(" "));
            if !out.stderr.trim().is_empty() {
                msg.push_str(&format!("\n{}", out.stderr.trim()));
            }
            return Err(msg);
        }
        Ok(out)
    }

    /// As [`Session::exec`], but a non-zero exit is an answer rather than an error — for commands
    /// whose failure means something (`readlink` on a path that is not a link, say).
    pub fn try_exec(&self, argv: &[&str]) -> Result<(bool, Output), String> {
        let mut cmd = Command::new("docker");
        cmd.arg("exec").arg(&self.name).args(argv);
        let out = cmd
            .output()
            .map_err(|e| format!("docker exec {}: {e}", argv.join(" ")))?;
        Ok((
            out.status.success(),
            Output {
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
        ))
    }

    /// Copy a host file into the container.
    ///
    /// `docker cp` rather than a bind mount: a mount from a Windows host presents every file as
    /// executable, and some things care about a file's mode.
    pub fn copy_in(&self, host: &Path, dest: &str) -> Result<(), String> {
        let out = Command::new("docker")
            .arg("cp")
            .arg(host)
            .arg(format!("{}:{dest}", self.name))
            .output()
            .map_err(|e| format!("docker cp: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "could not copy {} into the container: {}",
                host.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

/// A repository-relative path as the container sees it under [`WORK`].
///
/// Always `/`-separated: the host may be Windows, the container never is.
pub fn container_path(rel: &Path) -> String {
    let rel = rel
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    format!("{WORK}/{rel}")
}

/// `path` relative to `root`, for handing to [`container_path`].
pub fn relative_to(root: &Path, path: &Path) -> Result<PathBuf, String> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| format!("{} is not inside the repository", path.display()))
}
