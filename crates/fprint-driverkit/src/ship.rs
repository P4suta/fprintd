// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

// This module generates and tests driver files that carry their own SPDX headers; the identifiers
// in those string literals are data, not this file's licence.
// REUSE-IgnoreStart
//! `fpdev ship`: package a bring-up driver for contribution.
//!
//! The scaffold, the recordings, and the goldens live scattered across a working tree during
//! bring-up. This gathers them into the shape a PR wants: the driver integrated into
//! `fprint-backend-native`, or — when a port carries LGPL provenance — an isolated
//! `LGPL-2.1-or-later` crate kept out of the permissive core, as `docs/adding-a-driver.md`
//! §License discipline requires.
//!
//! The command is a plan over the working tree. `plan` turns the arguments into an ordered list of
//! `Step`s, each one an edit named in prose; [`--check`](ShipArgs::check) renders that plan and
//! writes nothing, while a real run applies each step and prints a PR-body draft. The two share the
//! one step list, so the dry run describes exactly what the real run performs.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Args;

/// The five files a `fpdev new-driver` scaffold renders, in the generator's order.
const SCAFFOLD_SIBLINGS: [&str; 4] = ["mod.rs", "proto.rs", "source.rs", "mock_tests.rs"];

/// The workspace crates an isolated driver crate may name, mirroring `fprint-driverkit`'s own row: a
/// driver reaches the seam types and the two kernels, nothing above them.
const ISOLATED_ALLOWED: [&str; 4] = [
    "fprint-core",
    "fprint-mindtct",
    "fprint-bozorth3",
    "fprint-backend-native",
];

/// Arguments for `fpdev ship`.
///
/// `driver` is the bring-up to package. `--isolated-crate` emits a standalone crate rather than
/// integrating into `fprint-backend-native`; `--lgpl` stamps the `LGPL-2.1-or-later` provenance a
/// ported driver needs. `--out` overrides the destination, and `--check` verifies the packaging
/// without writing.
#[derive(Args)]
pub struct ShipArgs {
    /// The bring-up driver to package, by its lowercase snake name.
    #[arg(long, value_name = "IDENT")]
    pub driver: String,
    /// Emit a standalone crate instead of integrating into `fprint-backend-native`.
    #[arg(long)]
    pub isolated_crate: bool,
    /// Stamp `LGPL-2.1-or-later` provenance, for a driver ported from LGPL code.
    #[arg(long)]
    pub lgpl: bool,
    /// The directory holding the scaffold's files (as `fpdev new-driver --out <dir>` wrote them).
    /// Defaults to the driver's place under `fprint-backend-native`.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// Verify the packaging without writing anything.
    #[arg(long)]
    pub check: bool,
}

/// A failure while packaging a bring-up.
#[derive(Debug)]
pub enum ShipError {
    /// `--lgpl` was given without `--isolated-crate`; an LGPL port may not touch the permissive core.
    LgplNeedsIsolation,
    /// The scaffold's device module was not found where the driver was expected.
    NoScaffold {
        /// The driver name asked for.
        driver: String,
        /// The device file that should have held the constants.
        expected: PathBuf,
    },
    /// The scaffold's device module did not declare a `VENDOR_ID`/`PRODUCT_ID` constant.
    NoDeviceIds {
        /// The device file read.
        file: PathBuf,
    },
    /// An I/O failure while applying a step.
    Io(String),
}

impl std::fmt::Display for ShipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LgplNeedsIsolation => write!(
                f,
                "--lgpl requires --isolated-crate: an LGPL-2.1-or-later port may not enter the \
                 permissive core, it must live in its own crate (docs/adding-a-driver.md §License \
                 discipline)"
            ),
            Self::NoScaffold { driver, expected } => write!(
                f,
                "no scaffold for `{driver}`: expected its device module at {}.\n\
                 Scaffold one first with `fpdev new-driver --name {driver} …`, or point `--out` at \
                 the directory that holds it.",
                expected.display()
            ),
            Self::NoDeviceIds { file } => write!(
                f,
                "{} declares no `VENDOR_ID`/`PRODUCT_ID` — is it a `fpdev new-driver` scaffold?",
                file.display()
            ),
            Self::Io(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ShipError {}

/// One edit the packaging performs, named in prose and locatable in the tree.
///
/// `headline` is a single imperative line; `detail` fills in the paths and the reasoning. `kind`
/// carries the data a real run applies. The dry run renders `headline`/`detail`; the real run reads
/// `kind`.
struct Step {
    headline: String,
    detail: Vec<String>,
    kind: StepKind,
}

/// The concrete edit behind a [`Step`], holding the paths and bytes a real run needs.
enum StepKind {
    /// Copy the scaffold's files to their home under the tree (a crate `src/`, or the driver dir).
    PlaceTree {
        to: PathBuf,
        /// Re-stamp each file's SPDX header to `LGPL-2.1-or-later` on the way.
        lgpl: bool,
    },
    /// Ensure a module file declares `line` among its `mod` declarations.
    EnsureModLine { path: PathBuf, line: String },
    /// Ensure `usb/drivers/mod.rs` exists and declares `pub mod <name>;`.
    EnsureDriversMod { path: PathBuf },
    /// Drop the scaffold's `#![allow(dead_code)]` (integration makes the items reachable).
    RemoveDeadCode { path: PathBuf },
    /// Write a fresh file (a crate manifest, a `lib.rs`, a `REUSE.toml`).
    WriteFile { path: PathBuf, contents: String },
    /// Insert `member` into the workspace `members` array of the root `Cargo.toml`.
    WorkspaceMember { path: PathBuf, member: String },
    /// Insert the crate's `ALLOWED` row into `xtask/src/deps.rs`.
    DepsRow { path: PathBuf, name: String },
    /// Append the crate's `release = false` block to `release-plz.toml`.
    ReleasePlzRow { path: PathBuf, name: String },
    /// Install the bring-up cassette under `tests/fixtures/<name>/`.
    InstallFixture {
        to_dir: PathBuf,
        /// The recording found beside the scaffold, if any.
        from: Option<PathBuf>,
    },
    /// A note the packaging states but does not write (a generated file to regenerate, a gate to run).
    Note,
}

/// The resolved context a plan is built from: the parsed driver identity and every path it edits.
struct Ctx {
    name: String,
    upper: String,
    vid: u16,
    pid: u16,
    isolated: bool,
    lgpl: bool,
    /// The repository root, so rendered paths are repo-relative.
    repo_root: PathBuf,
    /// Where the scaffold's files were read from.
    scaffold_dir: PathBuf,
    /// A `.cassette` sitting beside the scaffold, if the bring-up left one there.
    fixture: Option<PathBuf>,
}

impl Ctx {
    /// The path `p` rendered relative to the repository root, for a readable plan.
    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.repo_root)
            .unwrap_or(p)
            .display()
            .to_string()
            .replace('\\', "/")
    }

    /// `fprint-backend-native`'s crate root.
    fn native_root(&self) -> PathBuf {
        self.repo_root.join("crates").join("fprint-backend-native")
    }

    /// The isolated crate's root, `crates/fprint-driver-<name>`.
    fn isolated_root(&self) -> PathBuf {
        self.repo_root
            .join("crates")
            .join(format!("fprint-driver-{}", self.name))
    }
}

/// `fpdev ship`: package the bring-up named by `args`.
///
/// # Errors
/// Returns [`ShipError`] when `--lgpl` is asked without `--isolated-crate`, when no scaffold is
/// found for the driver, or when a step's I/O fails.
pub fn run(args: &ShipArgs) -> Result<(), Box<dyn std::error::Error>> {
    let ctx = resolve(args)?;
    let steps = plan(&ctx);

    if args.check {
        print!("{}", render_dry_run(&ctx, &steps));
        print!("{}", pr_body(&ctx, &steps, None, None));
        return Ok(());
    }

    println!("fpdev ship: packaging `{}` …\n", ctx.name);
    for step in &steps {
        let touched = apply(&ctx, &step.kind)?;
        println!("  {}", step.headline);
        for path in touched {
            println!("    wrote {}", ctx.rel(&path));
        }
    }

    // The PR body records the two acceptance oracles' current word. They read the tree and write
    // nothing, so gathering them is safe on a dry run too — but they cost a build, so the real run
    // is where they earn their place.
    let hw = tool_output(&ctx, &["hw-checklist", "--json", &ctx.name]);
    let acceptance = tool_outcome(&ctx, &["driver-check", &ctx.name]);
    println!();
    print!(
        "{}",
        pr_body(&ctx, &steps, hw.as_deref(), acceptance.as_ref())
    );
    Ok(())
}

/// Resolve the arguments into a [`Ctx`], parsing the scaffold's device identity.
fn resolve(args: &ShipArgs) -> Result<Ctx, ShipError> {
    if args.lgpl && !args.isolated_crate {
        return Err(ShipError::LgplNeedsIsolation);
    }
    let name = args.driver.clone();
    let repo_root = repo_root();
    let scaffold_dir = args.out.clone().unwrap_or_else(|| {
        repo_root
            .join("crates")
            .join("fprint-backend-native")
            .join("src")
            .join("usb")
            .join("drivers")
            .join(&name)
    });

    let device_file = scaffold_dir.join(format!("{name}.rs"));
    let device_src = std::fs::read_to_string(&device_file).map_err(|_| ShipError::NoScaffold {
        driver: name.clone(),
        expected: device_file.clone(),
    })?;
    let (Some(vid), Some(pid)) = (
        parse_hex_const(&device_src, "VENDOR_ID"),
        parse_hex_const(&device_src, "PRODUCT_ID"),
    ) else {
        return Err(ShipError::NoDeviceIds { file: device_file });
    };

    Ok(Ctx {
        upper: upper_camel(&name),
        vid,
        pid,
        isolated: args.isolated_crate,
        lgpl: args.lgpl,
        fixture: find_cassette(&scaffold_dir),
        name,
        repo_root,
        scaffold_dir,
    })
}

/// Build the ordered step list for the requested packaging.
fn plan(ctx: &Ctx) -> Vec<Step> {
    if ctx.isolated {
        plan_isolated(ctx)
    } else {
        plan_integrated(ctx)
    }
}

/// The module-integration plan: wire the scaffold into `fprint-backend-native`.
fn plan_integrated(ctx: &Ctx) -> Vec<Step> {
    let native = ctx.native_root();
    let usb = native.join("src").join("usb");
    let driver_dir = usb.join("drivers").join(&ctx.name);
    let usb_mod = usb.join("mod.rs");
    let drivers_mod = usb.join("drivers").join("mod.rs");
    let device_db = native.join("src").join("device_db.rs");
    let fixtures = native.join("tests").join("fixtures").join(&ctx.name);

    let mut steps = Vec::new();

    // A scaffold read from `--out` is a standalone tree the author placed elsewhere; move it into
    // the drivers directory. A scaffold already sitting in that directory needs no move.
    if ctx.scaffold_dir != driver_dir {
        steps.push(Step {
            headline: format!("place the scaffold under {}", ctx.rel(&driver_dir)),
            detail: vec![format!(
                "copy {} → {}",
                ctx.rel(&ctx.scaffold_dir),
                ctx.rel(&driver_dir)
            )],
            kind: StepKind::PlaceTree {
                to: driver_dir.clone(),
                lgpl: false,
            },
        });
    }

    steps.push(Step {
        headline: format!("declare the driver tree in {}", ctx.rel(&usb_mod)),
        detail: vec!["ensure `pub mod drivers;` is present".to_string()],
        kind: StepKind::EnsureModLine {
            path: usb_mod,
            line: "pub mod drivers;".to_string(),
        },
    });

    steps.push(Step {
        headline: format!(
            "register `pub mod {}` in {}",
            ctx.name,
            ctx.rel(&drivers_mod)
        ),
        detail: vec![format!(
            "ensure `pub mod {};` is declared, sorted, in the drivers module list",
            ctx.name
        )],
        kind: StepKind::EnsureDriversMod { path: drivers_mod },
    });

    steps.push(Step {
        headline: format!("remove the scaffold's `#![allow(dead_code)]` from {}", {
            let m = driver_dir.join("mod.rs");
            ctx.rel(&m)
        }),
        detail: vec![
            "integration makes the frame source reachable, so the justified allow is no longer \
             needed"
                .to_string(),
        ],
        kind: StepKind::RemoveDeadCode {
            path: driver_dir.join("mod.rs"),
        },
    });

    steps.push(Step {
        headline: format!("register {:04x}:{:04x} in the device DB", ctx.vid, ctx.pid),
        detail: device_db_detail(ctx, &device_db),
        kind: StepKind::Note,
    });

    steps.push(fixture_step(ctx, &fixtures));

    steps.push(Step {
        headline: format!(
            "run the acceptance gate: `cargo xtask driver-check {}`",
            ctx.name
        ),
        detail: vec![
            "the `docs/adding-a-driver.md` acceptance criteria, mechanized as one command"
                .to_string(),
        ],
        kind: StepKind::Note,
    });

    steps
}

/// The isolated-crate plan: scaffold a standalone `publish = false` crate and register it.
fn plan_isolated(ctx: &Ctx) -> Vec<Step> {
    let crate_root = ctx.isolated_root();
    let src = crate_root.join("src");
    let cargo = crate_root.join("Cargo.toml");
    let lib = src.join("lib.rs");
    let root_cargo = ctx.repo_root.join("Cargo.toml");
    let deps_rs = ctx.repo_root.join("xtask").join("src").join("deps.rs");
    let release_plz = ctx.repo_root.join("release-plz.toml");
    let fixtures = crate_root.join("tests").join("fixtures").join(&ctx.name);

    let mut steps = Vec::new();

    steps.push(Step {
        headline: format!("scaffold the crate {}", ctx.rel(&crate_root)),
        detail: vec![
            format!("write {} (publish = false)", ctx.rel(&cargo)),
            format!("write {}", ctx.rel(&lib)),
            format!("place the driver modules under {}", ctx.rel(&src)),
        ],
        kind: StepKind::WriteFile {
            path: cargo.clone(),
            contents: isolated_cargo_toml(ctx),
        },
    });

    steps.push(Step {
        headline: format!("write the crate root {}", ctx.rel(&lib)),
        detail: vec!["declare the driver modules and the crate's provenance note".to_string()],
        kind: StepKind::WriteFile {
            path: lib,
            contents: isolated_lib_rs(ctx),
        },
    });

    steps.push(Step {
        headline: format!("place the driver modules under {}", ctx.rel(&src)),
        detail: vec![format!(
            "copy {} → {}{}",
            ctx.rel(&ctx.scaffold_dir),
            ctx.rel(&src),
            if ctx.lgpl {
                ", re-stamped LGPL-2.1-or-later"
            } else {
                ""
            }
        )],
        kind: StepKind::PlaceTree {
            to: src,
            lgpl: ctx.lgpl,
        },
    });

    if ctx.lgpl {
        steps.push(Step {
            headline: "stamp LGPL-2.1-or-later provenance".to_string(),
            detail: vec![
                "each `.rs` carries an `SPDX-License-Identifier: LGPL-2.1-or-later` header"
                    .to_string(),
                format!(
                    "write {} so `reuse lint` reads the crate as LGPL, isolated from the \
                     permissive core",
                    ctx.rel(&crate_root.join("REUSE.toml"))
                ),
            ],
            kind: StepKind::WriteFile {
                path: crate_root.join("REUSE.toml"),
                contents: lgpl_reuse_toml(ctx),
            },
        });
    }

    steps.push(Step {
        headline: format!("add the workspace member in {}", ctx.rel(&root_cargo)),
        detail: vec![format!(
            "insert `\"crates/fprint-driver-{}\"` into the `members` array",
            ctx.name
        )],
        kind: StepKind::WorkspaceMember {
            path: root_cargo,
            member: format!("crates/fprint-driver-{}", ctx.name),
        },
    });

    steps.push(Step {
        headline: format!("insert the ALLOWED row in {}", ctx.rel(&deps_rs)),
        detail: vec![format!(
            "`fprint-driver-{}` may name {:?} — the one rule reaches the new crate",
            ctx.name, ISOLATED_ALLOWED
        )],
        kind: StepKind::DepsRow {
            path: deps_rs,
            name: format!("fprint-driver-{}", ctx.name),
        },
    });

    steps.push(Step {
        headline: format!("hold the crate back in {}", ctx.rel(&release_plz)),
        detail: vec![format!(
            "add `release = false` for `fprint-driver-{}` so a release neither versions nor \
             publishes it",
            ctx.name
        )],
        kind: StepKind::ReleasePlzRow {
            path: release_plz,
            name: format!("fprint-driver-{}", ctx.name),
        },
    });

    steps.push(fixture_step(ctx, &fixtures));

    steps.push(Step {
        headline: format!(
            "run the acceptance gate: `cargo xtask driver-check {}`",
            ctx.name
        ),
        detail: vec![
            "the `docs/adding-a-driver.md` acceptance criteria, mechanized as one command"
                .to_string(),
        ],
        kind: StepKind::Note,
    });

    steps
}

/// The device-DB step's detail: the exact record to register, and how, since the file is generated.
fn device_db_detail(ctx: &Ctx, device_db: &Path) -> Vec<String> {
    vec![
        format!(
            "{} is generated from the libfprint id-tables (`cargo xtask device-db`), so it is not \
             hand-edited.",
            ctx.rel(device_db)
        ),
        "Register the driver's ids by adding them to the generator's source, then regenerate. The \
         record this driver claims is:"
            .to_string(),
        String::new(),
        "    DeviceRecord {".to_string(),
        format!("        vid: 0x{:04x},", ctx.vid),
        format!("        pid: 0x{:04x},", ctx.pid),
        format!("        driver: {:?},", ctx.name),
        "        family: Family::HostImage,".to_string(),
        "    },".to_string(),
    ]
}

/// The fixture-install step, found recording or guidance.
fn fixture_step(ctx: &Ctx, fixtures: &Path) -> Step {
    let dest = fixtures.join(format!("{}.cassette", ctx.name));
    match &ctx.fixture {
        Some(from) => Step {
            headline: format!("install the capture fixture under {}", ctx.rel(fixtures)),
            detail: vec![format!("move {} → {}", ctx.rel(from), ctx.rel(&dest))],
            kind: StepKind::InstallFixture {
                to_dir: fixtures.to_path_buf(),
                from: Some(from.clone()),
            },
        },
        None => Step {
            headline: format!("install a capture fixture under {}", ctx.rel(fixtures)),
            detail: vec![
                "no `.cassette` was found beside the scaffold.".to_string(),
                format!(
                    "Freeze a bring-up recording with `cargo xtask capture-golden {} <recording>`, \
                     and place it at {}.",
                    ctx.name,
                    ctx.rel(&dest)
                ),
            ],
            kind: StepKind::InstallFixture {
                to_dir: fixtures.to_path_buf(),
                from: None,
            },
        },
    }
}

/// Render the dry-run report: the header, then each step numbered with its detail.
fn render_dry_run(ctx: &Ctx, steps: &[Step]) -> String {
    let mut s = String::new();
    let mode = if ctx.isolated {
        if ctx.lgpl {
            "isolated LGPL-2.1-or-later crate"
        } else {
            "isolated crate"
        }
    } else {
        "module integration into fprint-backend-native"
    };
    let _ = writeln!(
        s,
        "fpdev ship --check: `{}` ({:04x}:{:04x}) — {mode}",
        ctx.name, ctx.vid, ctx.pid
    );
    let _ = writeln!(s, "DRY RUN — nothing is written. The plan is:\n");
    for (i, step) in steps.iter().enumerate() {
        let _ = writeln!(s, "  {}. {}", i + 1, step.headline);
        for line in &step.detail {
            if line.is_empty() {
                let _ = writeln!(s);
            } else {
                let _ = writeln!(s, "       {line}");
            }
        }
    }
    let _ = writeln!(s);
    s
}

/// Render the PR-body draft: provenance, the HW checklist, the acceptance outcome, and the changes.
fn pr_body(
    ctx: &Ctx,
    steps: &[Step],
    hw: Option<&str>,
    acceptance: Option<&ToolOutcome>,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "--- PR body draft (markdown) ---\n");
    let _ = writeln!(s, "# `{}`: a native host-image driver\n", ctx.name);
    let mode = if ctx.isolated {
        format!(
            "This adds `fprint-driver-{}`, a standalone `publish = false`{} crate, kept out of the \
             permissive core.",
            ctx.name,
            if ctx.lgpl {
                " LGPL-2.1-or-later"
            } else {
                ""
            }
        )
    } else {
        format!(
            "This integrates the `{}` driver into `fprint-backend-native` behind the existing \
             `FrameSource` seam.",
            ctx.name
        )
    };
    let _ = writeln!(s, "{mode}\n");

    let _ = writeln!(s, "## Cassette-fixture provenance\n");
    match &ctx.fixture {
        Some(from) => {
            let _ = writeln!(
                s,
                "The capture fixture is `{}`, recorded during bring-up and frozen as the driver's \
                 golden. It carries only interoperability facts (USB traffic for {:04x}:{:04x}); no \
                 finger image is committed.\n",
                ctx.rel(from),
                ctx.vid,
                ctx.pid
            );
        }
        None => {
            let _ = writeln!(
                s,
                "No cassette fixture is committed yet. Freeze a bring-up recording with `cargo \
                 xtask capture-golden {} <recording>` so a later change that breaks decoding fails \
                 an ordinary test.\n",
                ctx.name
            );
        }
    }

    let _ = writeln!(s, "## HW-verification checklist (remaining)\n");
    match hw {
        Some(json) if !json.trim().is_empty() => {
            let _ = writeln!(s, "```json\n{}\n```\n", json.trim());
        }
        _ => {
            let _ = writeln!(
                s,
                "Run `cargo xtask hw-checklist {}` for the device values still asserted on faith \
                 (the scaffold marks each `HW-verified: required`).\n",
                ctx.name
            );
        }
    }

    let _ = writeln!(s, "## Acceptance\n");
    match acceptance {
        Some(outcome) if outcome.ran => {
            let verdict = if outcome.ok { "passed" } else { "did not pass" };
            let _ = writeln!(s, "`cargo xtask driver-check {}` {verdict}.\n", ctx.name);
            if !outcome.text.trim().is_empty() {
                let _ = writeln!(s, "```\n{}\n```\n", outcome.text.trim());
            }
        }
        _ => {
            let _ = writeln!(
                s,
                "Run `cargo xtask driver-check {}` — the `docs/adding-a-driver.md` acceptance \
                 criteria as one command.\n",
                ctx.name
            );
        }
    }

    let _ = writeln!(s, "## What this changes\n");
    for step in steps {
        let _ = writeln!(s, "- {}", step.headline);
    }
    let _ = writeln!(s);
    s
}

/// Apply one step to the working tree, returning the paths it wrote.
fn apply(ctx: &Ctx, kind: &StepKind) -> Result<Vec<PathBuf>, ShipError> {
    match kind {
        StepKind::PlaceTree { to, lgpl } => place_tree(ctx, to, *lgpl),
        StepKind::EnsureModLine { path, line } => {
            ensure_mod_line(path, line)?;
            Ok(vec![path.clone()])
        }
        StepKind::EnsureDriversMod { path } => {
            ensure_drivers_mod(path, &ctx.name)?;
            Ok(vec![path.clone()])
        }
        StepKind::RemoveDeadCode { path } => {
            let text = read(path)?;
            let stripped = strip_dead_code_allow(&text);
            if stripped != text {
                write(path, &stripped)?;
                Ok(vec![path.clone()])
            } else {
                Ok(Vec::new())
            }
        }
        StepKind::WriteFile { path, contents } => {
            if let Some(parent) = path.parent() {
                mkdirs(parent)?;
            }
            write(path, contents)?;
            Ok(vec![path.clone()])
        }
        StepKind::WorkspaceMember { path, member } => {
            let changed = insert_workspace_member(path, member)?;
            Ok(changed.then(|| path.clone()).into_iter().collect())
        }
        StepKind::DepsRow { path, name } => {
            let changed = insert_deps_row(path, name)?;
            Ok(changed.then(|| path.clone()).into_iter().collect())
        }
        StepKind::ReleasePlzRow { path, name } => {
            let changed = insert_release_plz(path, name)?;
            Ok(changed.then(|| path.clone()).into_iter().collect())
        }
        StepKind::InstallFixture { to_dir, from } => match from {
            Some(from) => {
                mkdirs(to_dir)?;
                let dest = to_dir.join(format!("{}.cassette", ctx.name));
                std::fs::rename(from, &dest)
                    .or_else(|_| std::fs::copy(from, &dest).map(|_| ()))
                    .map_err(|e| {
                        ShipError::Io(format!("install fixture {}: {e}", dest.display()))
                    })?;
                Ok(vec![dest])
            }
            None => Ok(Vec::new()),
        },
        StepKind::Note => Ok(Vec::new()),
    }
}

/// Copy the scaffold's files to `to`, optionally re-stamping their SPDX header.
fn place_tree(ctx: &Ctx, to: &Path, lgpl: bool) -> Result<Vec<PathBuf>, ShipError> {
    mkdirs(to)?;
    let device = format!("{}.rs", ctx.name);
    let mut written = Vec::new();
    for name in SCAFFOLD_SIBLINGS.iter().copied().chain([device.as_str()]) {
        let from = ctx.scaffold_dir.join(name);
        if !from.is_file() {
            continue;
        }
        let mut body = read(&from)?;
        if lgpl {
            body = restamp_lgpl(&body);
        }
        let dest = to.join(name);
        write(&dest, &body)?;
        written.push(dest);
    }
    Ok(written)
}

/// Ensure `path` (an existing module file) contains `line` among its module declarations.
fn ensure_mod_line(path: &Path, line: &str) -> Result<(), ShipError> {
    let body = read(path)?;
    if body.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    let mut lines: Vec<String> = body.lines().map(str::to_owned).collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("mod ") || l.starts_with("pub mod "))
        .unwrap_or(lines.len());
    lines.insert(at, line.to_owned());
    write(path, &(lines.join("\n") + "\n"))
}

/// Ensure `usb/drivers/mod.rs` exists and declares `pub mod <name>;`, sorted.
fn ensure_drivers_mod(path: &Path, name: &str) -> Result<(), ShipError> {
    let decl = format!("pub mod {name};");
    if !path.exists() {
        let body = format!(
            "// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors\n\
             //\n\
             // SPDX-License-Identifier: MIT OR Apache-2.0\n\n\
             //! Contributed native drivers, each a self-contained [`crate::FrameSource`] tree \
             generated by\n\
             //! `fpdev new-driver` and finalized against hardware. See `docs/adding-a-driver.md`.\n\n\
             {decl}\n"
        );
        return write(path, &body);
    }
    let body = read(path)?;
    if body.lines().any(|l| l.trim() == decl) {
        return Ok(());
    }
    let mut decls: Vec<String> = body
        .lines()
        .filter(|l| l.starts_with("pub mod "))
        .map(str::to_owned)
        .collect();
    decls.push(decl);
    decls.sort();
    let header: Vec<&str> = body
        .lines()
        .take_while(|l| !l.starts_with("pub mod "))
        .collect();
    write(
        path,
        &format!("{}\n{}\n", header.join("\n"), decls.join("\n")),
    )
}

/// Insert `member` into the workspace `members` array, returning whether an edit was needed.
fn insert_workspace_member(path: &Path, member: &str) -> Result<bool, ShipError> {
    let body = read(path)?;
    let needle = format!("\"{member}\"");
    if body.contains(&needle) {
        return Ok(false);
    }
    let mut out = String::new();
    let mut inserted = false;
    let mut in_members = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("members") && trimmed.contains('[') {
            in_members = true;
        }
        if in_members && !inserted && trimmed == "]" {
            out.push_str(&format!("  \"{member}\",\n"));
            inserted = true;
            in_members = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !inserted {
        return Err(ShipError::Io(format!(
            "{}: could not find the workspace `members` array to add `{member}`",
            path.display()
        )));
    }
    write(path, &out)?;
    Ok(true)
}

/// Insert the crate's `ALLOWED` row into `deps.rs`, returning whether an edit was needed.
fn insert_deps_row(path: &Path, name: &str) -> Result<bool, ShipError> {
    let body = read(path)?;
    // The crate's name is unique in the matrix, so its quoted form witnesses an existing row
    // whether that row is written on one line or several.
    if body.contains(&format!("{name:?}")) {
        return Ok(false);
    }
    let Some(anchor) = body.find("const ALLOWED") else {
        return Err(ShipError::Io(format!(
            "{}: no ALLOWED matrix to add `{name}` to",
            path.display()
        )));
    };
    // The outer array closes with `];`; nested driver rows close with `],`. The first `];` after
    // the matrix's declaration is the outer terminator.
    let Some(rel) = body[anchor..].find("];") else {
        return Err(ShipError::Io(format!(
            "{}: ALLOWED matrix has no terminator",
            path.display()
        )));
    };
    let close = anchor + rel;
    let mut row = String::from("    (\n");
    let _ = writeln!(row, "        {name:?},");
    row.push_str("        &[\n");
    for dep in ISOLATED_ALLOWED {
        let _ = writeln!(row, "            {dep:?},");
    }
    row.push_str("        ],\n    ),\n");
    let mut out = String::with_capacity(body.len() + row.len());
    out.push_str(&body[..close]);
    out.push_str(&row);
    out.push_str(&body[close..]);
    write(path, &out)?;
    Ok(true)
}

/// Append the crate's `release = false` block to `release-plz.toml`, returning whether it was added.
fn insert_release_plz(path: &Path, name: &str) -> Result<bool, ShipError> {
    let body = read(path)?;
    if body.contains(&format!("name = \"{name}\"")) {
        return Ok(false);
    }
    let mut out = body;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    let _ = write!(out, "\n[[package]]\nname = \"{name}\"\nrelease = false\n");
    write(path, &out)?;
    Ok(true)
}

/// The isolated crate's `Cargo.toml`.
fn isolated_cargo_toml(ctx: &Ctx) -> String {
    let license = if ctx.lgpl {
        "license = \"LGPL-2.1-or-later\" # a verbatim libfprint port; isolated from the permissive core"
    } else {
        "license.workspace = true"
    };
    format!(
        "# SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors\n\
         #\n\
         # SPDX-License-Identifier: {spdx}\n\n\
         [package]\n\
         name = \"fprint-driver-{name}\"\n\
         description = \"Native host-image fingerprint driver for {name}, a standalone \
         FrameSource behind fprint-backend-native's capture seam.\"\n\
         version.workspace = true\n\
         edition.workspace = true\n\
         rust-version.workspace = true\n\
         {license}\n\
         repository.workspace = true\n\
         authors.workspace = true\n\
         # A contributed driver crate, not part of the core stack: it makes no semver promise and \
         stays off crates.io.\n\
         publish = false\n\n\
         [dependencies]\n\
         fprint-core = {{ workspace = true }}\n\
         fprint-mindtct = {{ workspace = true }}\n\
         fprint-bozorth3 = {{ workspace = true }}\n\
         fprint-backend-native = {{ workspace = true, features = [\"serde\"] }}\n\n\
         [lints]\n\
         workspace = true\n\n\
         [dev-dependencies]\n\
         fprint-testkit = {{ workspace = true }}\n",
        spdx = if ctx.lgpl {
            "LGPL-2.1-or-later"
        } else {
            "MIT OR Apache-2.0"
        },
        name = ctx.name,
        license = license,
    )
}

/// The isolated crate's `src/lib.rs`.
fn isolated_lib_rs(ctx: &Ctx) -> String {
    let spdx = if ctx.lgpl {
        "LGPL-2.1-or-later"
    } else {
        "MIT OR Apache-2.0"
    };
    let provenance = if ctx.lgpl {
        format!(
            "//! ## Provenance\n\
             //!\n\
             //! This crate is a verbatim port of libfprint's `{name}` driver, so it is \
             `LGPL-2.1-or-later`\n\
             //! and lives outside the permissive `MIT OR Apache-2.0` core, as `ARCHITECTURE.md` \
             §Provenance &\n\
             //! licensing requires. It reaches `fprint-backend-native` only for the `FrameSource` \
             seam types.\n",
            name = ctx.name
        )
    } else {
        "//! ## Provenance\n\
         //!\n\
         //! This crate is original Rust stating interoperability facts, kept standalone so its \
         driver estate\n\
         //! evolves without weighing on the core. It reaches `fprint-backend-native` only for the \
         `FrameSource`\n\
         //! seam types.\n"
            .to_string()
    };
    format!(
        "// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors\n\
         //\n\
         // SPDX-License-Identifier: {spdx}\n\n\
         //! `fprint-driver-{name}`: a standalone host-image driver for the {upper} sensor.\n\
         //!\n\
         {provenance}\n\
         #![forbid(unsafe_code)]\n\n\
         pub mod proto;\n\
         pub mod source;\n\n\
         #[allow(clippy::module_inception)]\n\
         pub mod {name};\n\n\
         #[cfg(test)]\n\
         mod mock_tests;\n",
        spdx = spdx,
        name = ctx.name,
        upper = ctx.upper,
        provenance = provenance,
    )
}

/// The isolated LGPL crate's `REUSE.toml`, declaring the tree LGPL for `reuse lint`.
fn lgpl_reuse_toml(ctx: &Ctx) -> String {
    format!(
        "# SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors\n\
         # SPDX-License-Identifier: LGPL-2.1-or-later\n\
         #\n\
         # This crate is a verbatim libfprint port; every file it carries is LGPL-2.1-or-later, \
         isolated\n\
         # from the permissive core. The manifest and any data fixtures declare it in bulk here.\n\n\
         version = 1\n\n\
         [[annotations]]\n\
         path = [\"Cargo.toml\", \"tests/fixtures/{name}/**\"]\n\
         precedence = \"aggregate\"\n\
         SPDX-FileCopyrightText = \"2026 fprintd (pure-Rust) contributors\"\n\
         SPDX-License-Identifier = \"LGPL-2.1-or-later\"\n",
        name = ctx.name
    )
}

/// Re-stamp a source file's `MIT OR Apache-2.0` SPDX header as `LGPL-2.1-or-later`.
fn restamp_lgpl(body: &str) -> String {
    body.replace(
        "// SPDX-License-Identifier: MIT OR Apache-2.0",
        "// SPDX-License-Identifier: LGPL-2.1-or-later",
    )
}

/// Drop the scaffold's `#![allow(dead_code)]` and the justification block that introduces it.
fn strip_dead_code_allow(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(idx) = lines
        .iter()
        .position(|l| l.trim() == "#![allow(dead_code)]")
    else {
        return text.to_string();
    };
    // The scaffold precedes the attribute with a contiguous `//` block explaining it; drop that too.
    let mut start = idx;
    while start > 0 && lines[start - 1].trim_start().starts_with("//") {
        start -= 1;
    }
    // Drop one blank line after the attribute so the block leaves no double blank behind it.
    let mut end = idx + 1;
    if lines.get(end).is_some_and(|l| l.trim().is_empty()) {
        end += 1;
    }
    let kept: Vec<&str> = lines[..start]
        .iter()
        .chain(lines[end..].iter())
        .copied()
        .collect();
    let mut out = kept.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Parse `pub const <name>: u16 = 0x…;` out of a scaffold's device module.
fn parse_hex_const(text: &str, name: &str) -> Option<u16> {
    let prefix = format!("pub const {name}: u16 = ");
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix(&prefix) {
            let value = rest.trim().trim_end_matches(';').trim();
            let digits = value
                .strip_prefix("0x")
                .or_else(|| value.strip_prefix("0X"))
                .unwrap_or(value);
            return u16::from_str_radix(digits, 16).ok();
        }
    }
    None
}

/// The first `.cassette` sitting in `dir`, if the bring-up left a recording beside the scaffold.
fn find_cassette(dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("cassette"))
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Convert a lowercase snake name into UpperCamelCase (`acme_x` → `AcmeX`).
fn upper_camel(name: &str) -> String {
    name.split('_')
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// The repository root: this crate's directory, minus `crates/<crate>`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// The outcome of an acceptance tool run: whether it ran, whether it passed, and what it said.
struct ToolOutcome {
    ran: bool,
    ok: bool,
    text: String,
}

/// Run an xtask task and capture its stdout, or `None` if it could not run or failed.
fn tool_output(ctx: &Ctx, args: &[&str]) -> Option<String> {
    let out = run_xtask(ctx, args)?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run an xtask task, capturing whether it passed and its combined output.
fn tool_outcome(ctx: &Ctx, args: &[&str]) -> Option<ToolOutcome> {
    let out = run_xtask(ctx, args)?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(ToolOutcome {
        ran: true,
        ok: out.status.success(),
        text,
    })
}

/// Invoke `cargo run -p xtask -q -- <args>` from the repository root.
fn run_xtask(ctx: &Ctx, args: &[&str]) -> Option<std::process::Output> {
    Command::new("cargo")
        .current_dir(&ctx.repo_root)
        .args(["run", "-p", "xtask", "-q", "--"])
        .args(args)
        .output()
        .ok()
}

// -- small filesystem wrappers that report the offending path -------------------------------------

fn read(path: &Path) -> Result<String, ShipError> {
    std::fs::read_to_string(path)
        .map_err(|e| ShipError::Io(format!("read {}: {e}", path.display())))
}

fn write(path: &Path, contents: &str) -> Result<(), ShipError> {
    std::fs::write(path, contents)
        .map_err(|e| ShipError::Io(format!("write {}: {e}", path.display())))
}

fn mkdirs(path: &Path) -> Result<(), ShipError> {
    std::fs::create_dir_all(path)
        .map_err(|e| ShipError::Io(format!("create {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::newdriver::{self, Family, NewDriverOptions};

    /// A unique scratch directory for one test, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "fpdev-ship-{tag}-{}-{:p}",
                std::process::id(),
                &tag
            ));
            std::fs::create_dir_all(&base).unwrap();
            Scratch(base)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Render a `new-driver` scaffold into `dir`, the flat tree `fpdev new-driver --out` produces.
    fn scaffold_into(dir: &Path, name: &str) {
        let opts = NewDriverOptions::from_args(
            name,
            "1c7a",
            "0570",
            Family::HostImage,
            "vfs5011",
            Some(dir.to_path_buf()),
            false,
        )
        .unwrap();
        for file in newdriver::render(&opts) {
            std::fs::write(dir.join(&file.name), &file.contents).unwrap();
        }
    }

    fn ctx_for(scaffold_dir: &Path, name: &str, isolated: bool, lgpl: bool) -> Ctx {
        let device = std::fs::read_to_string(scaffold_dir.join(format!("{name}.rs"))).unwrap();
        Ctx {
            name: name.to_string(),
            upper: upper_camel(name),
            vid: parse_hex_const(&device, "VENDOR_ID").unwrap(),
            pid: parse_hex_const(&device, "PRODUCT_ID").unwrap(),
            isolated,
            lgpl,
            repo_root: PathBuf::from("/repo"),
            scaffold_dir: scaffold_dir.to_path_buf(),
            fixture: find_cassette(scaffold_dir),
        }
    }

    #[test]
    fn lgpl_requires_isolation() {
        let args = ShipArgs {
            driver: "acme".to_string(),
            isolated_crate: false,
            lgpl: true,
            out: None,
            check: true,
        };
        assert!(matches!(resolve(&args), Err(ShipError::LgplNeedsIsolation)));
    }

    #[test]
    fn a_missing_scaffold_is_named() {
        let scratch = Scratch::new("missing");
        let args = ShipArgs {
            driver: "ghost".to_string(),
            isolated_crate: false,
            lgpl: false,
            out: Some(scratch.0.clone()),
            check: true,
        };
        assert!(matches!(resolve(&args), Err(ShipError::NoScaffold { .. })));
    }

    #[test]
    fn device_ids_are_parsed_from_the_scaffold() {
        let scratch = Scratch::new("ids");
        scaffold_into(&scratch.0, "acme");
        let ctx = ctx_for(&scratch.0, "acme", false, false);
        assert_eq!(ctx.vid, 0x1c7a);
        assert_eq!(ctx.pid, 0x0570);
    }

    #[test]
    fn integrated_plan_names_wiring_devicedb_and_fixture() {
        let scratch = Scratch::new("integrated");
        scaffold_into(&scratch.0, "acme");
        let ctx = ctx_for(&scratch.0, "acme", false, false);
        let report = render_dry_run(&ctx, &plan(&ctx));

        assert!(report.contains("DRY RUN"));
        assert!(report.contains("pub mod drivers;"));
        assert!(report.contains("pub mod acme"));
        assert!(report.contains("drivers/mod.rs"));
        assert!(report.contains("device DB"));
        assert!(report.contains("DeviceRecord"));
        assert!(report.contains("Family::HostImage"));
        assert!(report.contains("fixtures/acme"));
        assert!(report.contains("allow(dead_code)"));
        assert!(report.contains("driver-check acme"));
        // A permissive integration never mentions the isolated-crate machinery.
        assert!(!report.contains("release-plz"));
        assert!(!report.contains("LGPL"));
    }

    #[test]
    fn isolated_lgpl_plan_names_deps_releaseplz_and_lgpl() {
        let scratch = Scratch::new("isolated");
        scaffold_into(&scratch.0, "acme");
        let ctx = ctx_for(&scratch.0, "acme", true, true);
        let report = render_dry_run(&ctx, &plan(&ctx));

        assert!(report.contains("fprint-driver-acme"));
        assert!(report.contains("xtask/src/deps.rs"));
        assert!(report.contains("ALLOWED"));
        assert!(report.contains("release-plz.toml"));
        assert!(report.contains("release = false"));
        assert!(report.contains("LGPL-2.1-or-later"));
        assert!(report.contains("REUSE.toml"));
        assert!(report.contains("Cargo.toml"));
        assert!(report.contains("driver-check acme"));
    }

    #[test]
    fn pr_body_carries_provenance_checklist_and_acceptance() {
        let scratch = Scratch::new("prbody");
        scaffold_into(&scratch.0, "acme");
        let ctx = ctx_for(&scratch.0, "acme", false, false);
        let steps = plan(&ctx);
        let acceptance = ToolOutcome {
            ran: true,
            ok: true,
            text: "acme: acceptance checks pass".to_string(),
        };
        let body = pr_body(&ctx, &steps, Some("[]"), Some(&acceptance));

        assert!(body.contains("# `acme`"));
        assert!(body.contains("Cassette-fixture provenance"));
        assert!(body.contains("HW-verification checklist"));
        assert!(body.contains("Acceptance"));
        assert!(body.contains("passed"));
        assert!(body.contains("What this changes"));
    }

    #[test]
    fn strip_dead_code_removes_attribute_and_justification() {
        let scratch = Scratch::new("deadcode");
        scaffold_into(&scratch.0, "acme");
        let before = std::fs::read_to_string(scratch.0.join("mod.rs")).unwrap();
        assert!(before.contains("#![allow(dead_code)]"));
        let after = strip_dead_code_allow(&before);
        assert!(!after.contains("#![allow(dead_code)]"));
        assert!(!after.contains("No USB enumerator constructs"));
        // The surrounding module stays intact and gains no double blank at the seam.
        assert!(after.contains("pub mod proto;"));
        assert!(!after.contains("\n\n\n"));
        // Idempotent: a second pass is a no-op.
        assert_eq!(strip_dead_code_allow(&after), after);
    }

    #[test]
    fn integrated_apply_wires_a_sandbox_tree() {
        // A minimal backend tree under a sandbox root: the two module files ship applies to, and the
        // scaffold already sitting in its drivers directory.
        let scratch = Scratch::new("apply");
        let root = &scratch.0;
        let usb = root.join("crates/fprint-backend-native/src/usb");
        let driver_dir = usb.join("drivers/acme");
        std::fs::create_dir_all(&driver_dir).unwrap();
        std::fs::write(
            usb.join("mod.rs"),
            "//! usb\n\npub mod proto;\nmod source;\n",
        )
        .unwrap();
        scaffold_into(&driver_dir, "acme");

        let ctx = Ctx {
            name: "acme".to_string(),
            upper: "Acme".to_string(),
            vid: 0x1c7a,
            pid: 0x0570,
            isolated: false,
            lgpl: false,
            repo_root: root.clone(),
            scaffold_dir: driver_dir.clone(),
            fixture: None,
        };
        for step in plan(&ctx) {
            apply(&ctx, &step.kind).unwrap();
        }

        let usb_mod = std::fs::read_to_string(usb.join("mod.rs")).unwrap();
        assert!(usb_mod.contains("pub mod drivers;"));
        let drivers_mod = std::fs::read_to_string(usb.join("drivers/mod.rs")).unwrap();
        assert!(drivers_mod.contains("pub mod acme;"));
        let mod_rs = std::fs::read_to_string(driver_dir.join("mod.rs")).unwrap();
        assert!(!mod_rs.contains("#![allow(dead_code)]"));
    }

    #[test]
    fn isolated_apply_registers_deps_and_release() {
        let scratch = Scratch::new("isoapply");
        let root = &scratch.0;
        let scaffold = root.join("scaffold");
        std::fs::create_dir_all(&scaffold).unwrap();
        scaffold_into(&scaffold, "acme");

        // The three shared files the isolated plan patches, in their real repo locations.
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\n  \"crates/fprint-core\",\n]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("xtask/src")).unwrap();
        std::fs::write(
            root.join("xtask/src/deps.rs"),
            "const ALLOWED: &[(&str, &[&str])] = &[\n    (\"fprint-core\", &[]),\n];\n",
        )
        .unwrap();
        std::fs::write(
            root.join("release-plz.toml"),
            "[workspace]\n\n[[package]]\nname = \"fprintd\"\nrelease = false\n",
        )
        .unwrap();

        let ctx = Ctx {
            name: "acme".to_string(),
            upper: "Acme".to_string(),
            vid: 0x1c7a,
            pid: 0x0570,
            isolated: true,
            lgpl: true,
            repo_root: root.clone(),
            scaffold_dir: scaffold.clone(),
            fixture: None,
        };
        for step in plan(&ctx) {
            apply(&ctx, &step.kind).unwrap();
        }

        let deps = std::fs::read_to_string(root.join("xtask/src/deps.rs")).unwrap();
        assert!(deps.contains("\"fprint-driver-acme\","));
        assert!(deps.contains("\"fprint-backend-native\","));
        // The insertion stays inside the ALLOWED matrix, before its terminator.
        assert!(deps.find("fprint-driver-acme").unwrap() < deps.find("];").unwrap());

        let release = std::fs::read_to_string(root.join("release-plz.toml")).unwrap();
        assert!(release.contains("name = \"fprint-driver-acme\""));

        let members = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(members.contains("\"crates/fprint-driver-acme\""));

        // The LGPL crate carries LGPL headers, isolated from the permissive core.
        let lib =
            std::fs::read_to_string(root.join("crates/fprint-driver-acme/src/lib.rs")).unwrap();
        assert!(lib.contains("SPDX-License-Identifier: LGPL-2.1-or-later"));
        let device =
            std::fs::read_to_string(root.join("crates/fprint-driver-acme/src/acme.rs")).unwrap();
        assert!(device.contains("SPDX-License-Identifier: LGPL-2.1-or-later"));
        assert!(root.join("crates/fprint-driver-acme/REUSE.toml").is_file());

        // Idempotent: a second application changes nothing.
        for step in plan(&ctx) {
            assert!(apply(&ctx, &step.kind).is_ok());
        }
        let deps_again = std::fs::read_to_string(root.join("xtask/src/deps.rs")).unwrap();
        assert_eq!(deps_again.matches("fprint-driver-acme").count(), 1);
    }
}
// REUSE-IgnoreEnd
