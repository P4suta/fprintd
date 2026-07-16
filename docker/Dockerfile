# SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Linux dev + verification image for the pure-Rust fingerprint stack.
#
# It exists to build and test the Linux-only crates (fprint-backend-libfprint shim,
# fprintd daemon) and to run hardware-free enroll/verify against libfprint's
# *virtual* drivers. The repo itself is NOT copied in — it is bind-mounted at
# /work at run time (see docker/docker-compose.yml), so the image is reusable
# across code changes and the build context stays tiny.
#
# libfprint is built FROM SOURCE with only the virtual drivers, because distro
# packages do not reliably ship them — and the virtual drivers are our entire
# hardware-free test path. Pinned to match the vendored reference (1.94.10) so
# the shim's bindgen output is deterministic.

FROM rust:1-bookworm

ARG LIBFPRINT_REF=v1.94.10

# --- system build deps -------------------------------------------------------
# libfprint's own build deps (meson/ninja/glib/gusb/gudev/nss/pixman), the
# bindgen toolchain for the libfprint-sys FFI crate (clang/libclang), and a
# D-Bus stack for the daemon integration test (dbus-daemon, dbus-run-session,
# gdbus from libglib2.0-bin).
RUN apt-get update && apt-get install --no-install-recommends -y \
        meson ninja-build pkg-config \
        libglib2.0-dev libgusb-dev libgudev-1.0-dev libudev-dev udev libnss3-dev libpixman-1-dev \
        clang libclang-dev llvm-dev \
        git ca-certificates \
        dbus libglib2.0-bin \
    && rm -rf /var/lib/apt/lists/*

# --- libfprint from source, virtual drivers only -----------------------------
RUN git clone --depth 1 --branch "${LIBFPRINT_REF}" \
        https://gitlab.freedesktop.org/libfprint/libfprint.git /src/libfprint \
    && meson setup /src/libfprint/build /src/libfprint \
        -Ddrivers=virtual_image,virtual_device,virtual_device_storage \
        -Dgtk-examples=false -Ddoc=false -Dintrospection=false \
        -Dudev_rules=disabled \
        --prefix=/usr --buildtype=release \
    && meson compile -C /src/libfprint/build \
    && meson install -C /src/libfprint/build \
    && ldconfig \
    && rm -rf /src/libfprint

# clippy/rustfmt for the strict-lint gate (`cargo clippy -- -D warnings`). Placed after the
# libfprint build so editing it never invalidates that (slow) layer's cache.
RUN rustup component add clippy rustfmt

# sccache, for CI only — nothing here turns it on. `docker/docker-compose.ci.yml` sets
# RUSTC_WRAPPER, because CI is where a build starts from nothing: a runner shares no volume with
# the last one, so the 109 third-party crates behind the libfprint-sys bindgen spine are compiled
# from scratch on every pull request. Measured in this image: `cargo test --workspace
# --all-features --no-run` takes 84s cold and 36s against a warm cache, at a 167 MiB cache and a
# 130/130 hit rate. A dev box keeps its `fp-target` volume and needs none of it.
#
# Pinned: an unpinned tool is a moving dependency, and this one sits under every compile.
ARG SCCACHE_VERSION=0.8.2
RUN curl -sSfL \
        "https://github.com/mozilla/sccache/releases/download/v${SCCACHE_VERSION}/sccache-v${SCCACHE_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
        -o /tmp/sccache.tar.gz \
    && tar -xzf /tmp/sccache.tar.gz -C /tmp \
    && install -m 0755 "/tmp/sccache-v${SCCACHE_VERSION}-x86_64-unknown-linux-musl/sccache" /usr/local/bin/sccache \
    && rm -rf /tmp/sccache.tar.gz "/tmp/sccache-v${SCCACHE_VERSION}-x86_64-unknown-linux-musl" \
    && sccache --version

ENV CARGO_HOME=/cargo
WORKDIR /work
CMD ["bash"]
