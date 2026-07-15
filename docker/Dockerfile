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

ENV CARGO_HOME=/cargo
WORKDIR /work
CMD ["bash"]
