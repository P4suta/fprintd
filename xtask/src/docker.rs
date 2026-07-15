// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Running things in containers, without going through a shell.
//!
//! The shell is the whole point. `mise.toml`'s Docker tasks carry `MSYS_NO_PATHCONV=1` and
//! `$(pwd -W 2>/dev/null || pwd)` because a bind-mount path routed through Git Bash gets
//! rewritten on its way to Docker — and those tasks then fail anyway on this project's Windows
//! dev box, because mise runs them through `cmd.exe`, which understands neither. Spawning
//! Docker with [`std::process::Command`] enters no shell at all, so a path is just a path.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the repository is mounted inside every container this module starts.
pub const WORK: &str = "/work";

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
    /// stderr is passed through to the terminal rather than captured: a compiler's warnings and
    /// a clone's progress are for the human watching, and nothing here parses them.
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
