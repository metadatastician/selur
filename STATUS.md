<!-- SPDX-License-Identifier: CC-BY-SA-4.0 -->
# Selur — Measured Status

**Last measured:** 2026-07-28  
**Honest completion:** ~30%  
**Languages:** Rust (core + compose) · Zig (wasm) · Idris2 (theorems)

> This document records **measured** state: every claim below is a file read, a build
> run, or a test executed on the dates shown. Where an existing document in this repo
> contradicts it, this one is correct and the other is stale. Full evidence and
> cross-repo context: `dev-notes/stapeln-ecosystem-COMPREHENSIVE-SITREP-2026-07-28.md`.

## Summary

~30%. The core library's headline claim is refuted by its own manifest; the working asset is selur-compose.

## What genuinely works

- `cargo check --workspace --all-targets` passes on the core library
- 48 tests pass, real assertions, zero `todo!()`/`unimplemented!()` across 197 functions
- `selur/zig` builds to a wasm32-freestanding artifact
- `compose/` is a genuine 30-subcommand CLI (up, down, ps, logs, exec, scale, watch, sbom, provenance, verify, policy, network, volume, ...) and its binary builds
- `compose/src/ct.rs` correctly shells out to cerro-torre's `ct` binary rather than reimplementing it

## What is broken, missing, or misreported

- **The zero-copy premise is refuted by the code.** The README claims IPC that 'replaces serialization (e.g. JSON/HTTP)' and is '~30-50% faster'. `Cargo.toml` depends on `ureq` (HTTP client) and `serde_json`; `src/lib.rs:54` maps every command to an HTTP verb + REST path; `:119` defaults to `http://127.0.0.1:4010`.
- **The core library is only 513 lines** — the other ~8k Rust lines are `compose/` (6,503) and tests.
- `compose/` tests have not compiled since an axum major bump (`E0308` at `tests/parity_stack.rs:186`, `E0382` at `:144`).
- 3 of the 4 headline Idris2 theorems are unproven holes (`bridgeCorrectness`, `bridgeSafety`, `bridgeLiveness` = `?rhs -- TODO`); `bridge` itself returns a `-- Placeholder`.
- `ephapax/*.eph` is pseudo-code — no Ephapax compiler exists anywhere; the 'Ephapax linear types' feature is unbuildable prose.
- `integrations/svalinn/` is ReScript (estate-banned) targeting a repo with zero `.res` files; `integrations/vordr/` is a Rustler NIF for `:selur_vordr` while vordr's app is `:vordr`.
- selur calls `POST /api/v1/containers` on vordr — **vordr serves no such API**.
- No build or test gate on GitHub at all.

## Notes and open rulings

- The bundle (`container/stapeln/compose.toml`) treats selur as TWO things: the orchestrator (`selur-compose up`) and a network driver (`[networks.default] driver = "selur"`). The 513-line HTTP core is neither.
- OPEN RULING R1: retire the core lib and promote selur-compose, or build the shared-memory transport for real, or drop the claim. See dev-notes.

## Next actions

1. RULING NEEDED (R1): what is selur — compose tool, transport, or both?
2. Either implement the zero-copy transport or remove the performance claim from the README
3. Fix the compose test suite (axum E0308/E0382) so it compiles again
4. Discharge or delete the three Idris2 theorem holes
5. Add a build/test gate — there is currently none on GitHub

## CI/CD status

As of 2026-07-28, post-merge: **4/4 workflows parse clean**, with zero
illegal `timeout-minutes` on reusable-call jobs and zero phantom `codeql-action` SHAs.
(Three sweep-introduced fault classes were repaired and merged on this date — see the
ecosystem sitrep for the taxonomy.)

**Gates that genuinely enforce something:**

- **None.** This repo has no build or test gate that can fail.

**Gates that run but cannot fail (or check nothing):**

- `codeql.yml` pinned to `language: actions` — zero application source analysed
- **no build or test gate on GitHub at all** — the 48 passing tests are not run by CI

> A gate is not done until it has been observed to **fail** on a deliberate defect.
> Every fake gate listed above passed its own review.

## Ecosystem position

This repo is part of the six-repo container stack designed by `stapeln`. The canonical
integration contract is the 8-file `container/stapeln/` bundle, in which each satellite
consumes its own file:

| File | Consumer |
|---|---|
| `compose.toml` | selur |
| `vordr.toml` | vordr |
| `rokur.toml` | rokur |
| `.gatekeeper.yaml` | svalinn |
| `manifest.toml` + `ct-build.sh` | cerro-torre |
| `deploy.k9.ncl` | K9 / k9-svc |

Runtime chain: `svalinn (443/80) -> rokur (8081) -> app`, with vordr watching all three,
cerro-torre signing each as a `.ctp`, and selur as the network driver.

**As of this measurement no repo emits or consumes that bundle**; five mutually
incompatible ad-hoc contracts exist instead, of which exactly one works.

