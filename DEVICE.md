# Building & running on the reMarkable 2

This is the RM2-first device target: mono Carta panel, **armv7 (32-bit Cortex-A7)**, driven
through the **rm2fb** shim. The device-facing code lives behind the `device` cargo feature
so the default workspace stays host-buildable and testable; only `--features device` pulls
in `libremarkable` and the real panel transport.

```
target triple : armv7-unknown-linux-gnueabihf
device feature : rmb-hal/device  (enables the rm2fb backend via libremarkable)
panel          : 1404 × 1872, RGB565, via rm2fb /swtfb.01 + SysV message queue
```

## 1. Toolchain options (pick one)

The RM2 runs an older OpenEmbedded glibc, so a binary linked against a *newer* glibc may
fail on-device with `version 'GLIBC_2.xx' not found`. Three ways to get a compatible build,
in order of convenience vs. safety:

1. **`cross` (quickest).** Docker-based, no host toolchain needed (`docker` is enough):
   ```
   cargo install cross
   cross build --release --target armv7-unknown-linux-gnueabihf --features device -p rmbrowser
   ```
   Uses the image pinned in `Cross.toml`. Watch for the glibc caveat above; if the device
   rejects the binary, use option 2 or 3.

2. **reMarkable OE SDK (most compatible).** Install the official toolchain (Codex / the
   `oecore-x86_64-…-toolchain` installer), source its environment, and point Cargo's linker
   at `arm-remarkable-linux-gnueabi-gcc`:
   ```
   # after sourcing the SDK environment-setup script:
   export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER="$CC"   # the SDK's gcc wrapper
   cargo build --release --target armv7-unknown-linux-gnueabihf --features device -p rmbrowser
   ```
   This links against the device's exact glibc and is the safest path for distribution.

3. **Static musl (no glibc dependency).** Build against `armv7-unknown-linux-musleabihf` for
   a fully static binary. Avoids glibc entirely, but native deps of `libremarkable` can be
   fiddly on musl — try this only if 1/2 are inconvenient.

The bundled `.cargo/config.toml` sets `-C target-cpu=cortex-a7` and the NEON/VFPv4 features
for whichever linker you supply.

## 2. Type-checking the device code without a full toolchain

You can verify the device backend *compiles* for armv7 without a linker (no native ARM gcc
required) — `cargo check` doesn't link:

```
rustup target add armv7-unknown-linux-gnueabihf
cargo check --target armv7-unknown-linux-gnueabihf --features device -p rmb-hal
```

## 3. Prerequisite on the device: the rm2fb server

The RM2 has no kernel framebuffer; our backend talks to the **rm2fb server** that owns the
panel. Install it via [Toltec](https://toltec-dev.org/) (the `rm2fb` / `display` package),
which runs the server (`LD_PRELOAD`ed into `xochitl`). Once it's running, `/dev/shm/swtfb.01`
and the message queue exist and `libremarkable` (built for RM2) drives the panel natively —
no per-app LD_PRELOAD client needed.

## 4. Deploy & run

```
# copy the binary to the tablet (USB networking is 10.11.99.1, or use Wi-Fi)
scp target/armv7-unknown-linux-gnueabihf/release/rmbrowser root@10.11.99.1:/home/root/

# on the device (rm2fb server already running via Toltec):
ssh root@10.11.99.1 /home/root/rmbrowser
```

The current `rmbrowser` binary is a bring-up smoke test: under `--features device` it renders
the grey ramp to the real panel via rm2fb instead of the in-memory backend, so a successful
run shows a dithered greyscale band on the display. Stop `xochitl` first if you want the
screen to yourself (`systemctl stop xochitl`), and restart it when done.

> Final on-glass validation happens on your hardware — the host CI only proves the software
> render path and that the device backend type-checks for armv7.
