# SPDX-License-Identifier: MPL-2.0
# Containerfile for selur — Ephapax-linear WASM sealant
#
# Multi-stage build: compile selur.wasm via Zig, package in a minimal runtime
# image. Idris2 proofs and the Rust host library are compile/verification-time
# artefacts and are NOT shipped — see vordr's Containerfile for the same
# pattern (verification artefacts stay out of the runtime image).
#
# The runtime image is an artefact-distribution container: it ships
# selur.wasm at a well-known path, and the default ENTRYPOINT prints the
# WASM to stdout so consumers can extract it with:
#
#   podman run --rm selur:latest > selur.wasm
#
# Or copy it out of a stopped container:
#
#   id=$(podman create selur:latest) && podman cp $id:/usr/local/lib/selur.wasm . && podman rm $id
#
# Build:  podman build -f Containerfile -t selur:latest .

# ── Stage 1: Build selur.wasm with Zig ────────────────────────────
FROM cgr.dev/chainguard/wolfi-base:latest AS zig-builder

RUN apk add --no-cache zig

WORKDIR /build

# Only the Zig sources are needed for the WASM artefact — the Rust host
# library (src/lib.rs, Cargo.toml) is for downstream embedders, not for
# producing the wasm. Idris2 (idris/) is for proof verification only.
COPY zig/ ./zig/

RUN cd zig && zig build wasm

# ── Stage 2: Runtime — artefact distribution ──────────────────────
FROM cgr.dev/chainguard/wolfi-base:latest

LABEL org.opencontainers.image.title="selur" \
      org.opencontainers.image.description="Ephapax-linear WASM sealant — zero-copy IPC bridge between Svalinn and Vörðr (WASM artefact)" \
      org.opencontainers.image.source="https://github.com/hyperpolymath/selur" \
      org.opencontainers.image.licenses="PMPL-1.0-or-later" \
      net.hyperpolymath.selur.artifact-path="/usr/local/lib/selur.wasm"

COPY --from=zig-builder /build/zig/zig-out/bin/selur.wasm /usr/local/lib/selur.wasm

ENTRYPOINT ["cat", "/usr/local/lib/selur.wasm"]
