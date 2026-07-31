// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Driving a real sensor through libfprint alone, with none of our code in the path.
//!
//! This is the first question a bring-up has to answer — is it the sensor, or is it us? — so it
//! runs libfprint's own example clients rather than anything from this workspace.

use std::process::{Command, Stdio};

/// The compose file that passes the USB bus into the container.
const COMPOSE: &str = "docker/docker-compose.hw.yml";
/// Where the enrolled template is written.
///
/// Not a path under the repository, and not negotiable. libfprint's example clients save to a
/// **relative** path (`test-storage.variant`, `examples/storage.c`) and the container's default
/// working directory is the bind-mounted worktree, so the default would drop a real, irrevocable
/// biometric into the source tree. `/run` is the container's own filesystem and dies with `--rm`,
/// which is also why enroll and verify have to share one container.
const SCRATCH: &str = "/run/bringup";

/// Enroll and then verify a finger through libfprint's example clients.
///
/// `finger` is an index into the prompt's list, 0 through 9; 6 is the right index finger. Any
/// finger works so long as the same one is presented throughout — the label is metadata the sensor
/// never sees.
pub fn ground_truth(finger: &str) -> Result<(), String> {
    let index: u8 = finger
        .parse()
        .map_err(|_| format!("finger must be 0 through 9, got `{finger}`"))?;
    if index > 9 {
        return Err(format!("finger must be 0 through 9, got `{finger}`"));
    }

    println!("=== ENROLL: present the same finger when the sensor asks ===");
    example(index, "fp-enroll")?;
    println!("\n=== VERIFY: present that same finger once more ===");
    example(index, "fp-verify")
}

/// Run one example client, answering its two prompts and leaving the rest to the sensor.
///
/// The clients read exactly two things from standard input — the finger index, and a yes/no — and
/// wait on the hardware for everything else. `stdbuf` is not decoration: piping standard input
/// makes their standard output block-buffered, and the "scan your finger" prompts would then only
/// appear after the scan they are asking for.
fn example(finger: u8, client: &str) -> Result<(), String> {
    let mut child = Command::new("docker")
        .args(["compose", "-f", COMPOSE, "run", "--rm", "-T", "-w", SCRATCH])
        // The service sets this to `all`, which is right for diagnosing a stalled transfer and
        // wrong for a human who has to know when to present a finger: it buries the prompts under
        // a thousand lines of driver trace.
        .args(["-e", "G_MESSAGES_DEBUG="])
        .args(["hw", "stdbuf", "-oL", "-eL", client])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn docker compose: {e}"))?;

    let stdin = child
        .stdin
        .as_mut()
        .ok_or_else(|| "docker compose did not provide a standard input pipe".to_string())?;
    std::io::Write::write_all(stdin, format!("{finger}\nn\n").as_bytes())
        .map_err(|e| format!("write to {client}: {e}"))?;

    let status = child
        .wait()
        .map_err(|e| format!("wait for {client}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{client} failed ({status})"))
    }
}
