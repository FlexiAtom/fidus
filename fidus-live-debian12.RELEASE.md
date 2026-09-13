# fidus 0.1.0-beta.1 Debian 12 live-container image

This release artifact is a Debian 12 slim runtime image containing the
release-built `fidus-test` and `fidus-live-calibrate` binaries plus the
Wayland/X11 runtime libraries required by the selected backend.

The image was built from:

- base: `docker.m.daocloud.io/library/debian:12-slim`
- base digest: `sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171`
- Debian apt mirror: `https://mirrors.ustc.edu.cn/debian/` (not Tsinghua)
- Rust toolchain: `1.86.0`, downloaded through `https://rsproxy.cn`
- image ID: see `fidus-live-debian12.image-id`
- archive SHA-256: see `fidus-live-debian12.tar.zst.sha256`
- release manifest: see `fidus-live-debian12.manifest.json` (SBOM, signature, attestation, and cross-machine validation remain unavailable)

## Import

```sh
zstd -dc fidus-live-debian12.tar.zst | docker load
sha256sum -c fidus-live-debian12.tar.zst.sha256
```

The loaded tag is `fidus-live:debian12`. Pass that tag to
`scripts/live_container.sh` with `--allow-unpinned-image`, or retag it with a
registry reference containing the image digest before normal use.

The host runner must still explicitly provide `--allow-live-container` and the
Wayland/X11 session. The image is **not display isolation**: a mounted display
socket can affect the host compositor. It contains no compositor control,
output-mutation authority, `/dev/dri`, host HOME, or display socket.

The archive was smoke-tested as a non-root user with `network=none`,
`cap-drop=ALL`, `no-new-privileges`, read-only rootfs, and `fidus-test parse`.
That smoke test does not replace a live-host-specific Wayland permission test.
