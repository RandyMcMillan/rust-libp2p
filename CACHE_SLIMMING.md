# Cache Slimming Report

## Executive Summary

The workspace dependency tree currently contains **59 crates with multiple versions** in `Cargo.lock`. This bloats the `rust-cache` artifacts and slows CI cold-start times. The issue is transitive: most duplicates are not direct workspace dependencies but come from upstream crates pulling in incompatible versions.

| Metric | Value |
|--------|-------|
| Total duplicate crate names | 59 |
| Worst offender | `rand_core` — 4 versions |
| Second worst | `rand`, `getrandom` — 4 versions each |
| Proc-macro duplicates | `syn` (3), `thiserror-impl` (2), `logos-derive` (2), `proc-macro2` (2) |
| Crypto stack duplicates | `sha2`, `digest`, `curve25519-dalek`, `x25519-dalek`, `p256`, `ecdsa`, `elliptic-curve`, `sec1`, `spki`, `pkcs8`, `signature` — all have 2 versions |

## Full Duplicate Inventory

```
4  rand_core
4  rand
4  getrandom
3  syn
3  rand_chacha
3  itertools
2  yasna
2  x509-parser
2  x25519-dalek
2  tower-http
2  thiserror-impl
2  thiserror
2  spki
2  socket2
2  signature
2  sha2
2  sec1
2  rfc6979
2  regex-automata
2  proc-macro2
2  primeorder
2  pkcs8
2  pem-rfc7468
2  p256
2  oid-registry
2  memchr
2  logos-derive
2  logos-codegen
2  logos
2  log
2  libc
2  inout
2  hmac
2  hkdf
2  hashbrown
2  group
2  foldhash
2  ff
2  elliptic-curve
2  either
2  ecdsa
2  digest
2  der-parser
2  der
2  data-encoding
2  curve25519-dalek
2  crypto-common
2  crypto-bigint
2  cpufeatures
2  core-foundation
2  const-oid
2  cipher
2  chacha20
2  block-buffer
2  bitflags
2  base64
2  base16ct
2  asn1-rs-derive
2  asn1-rs
```

## Root Causes

### 1. The `rand` 0.10 ecosystem migration
The workspace bumped to `rand = "0.10"` in `Cargo.toml`, but several upstream crates still depend on `rand 0.8` or `0.7`. This cascades into `rand_core`, `rand_chacha`, and `getrandom` duplicates.

**Affected crates pulling old `rand`:**
- `webrtc` ecosystem (via `rtc-dtls`, `rtc-sctp`, etc.)
- `quickcheck` (workspace `quickcheck-ext` uses `quickcheck` which may still be on `rand 0.8`)
- Various crypto crates in the tree

### 2. The `thiserror` 1.x → 2.x split
Some workspace crates and upstream deps have moved to `thiserror 2.x`, while others remain on `1.x`. This duplicates both `thiserror` and `thiserror-impl`.

### 3. Crypto crate version splits (`signature`, `elliptic-curve`, `p256`, etc.)
The `aws-lc-rs` / TLS migration and general crypto ecosystem churn have caused multiple versions of:
- `signature` (2.x vs 3.x)
- `elliptic-curve` (0.13 vs 0.14)
- `p256`, `ecdsa`, `sec1`, `spki`, `pkcs8`
- `sha2` (0.10 vs 0.11)
- `digest` (0.10 vs 0.11)

### 4. `syn` / `proc-macro2` duplicates
Some proc-macro crates pin older `syn`/`proc-macro2` versions, causing duplicates even though the workspace itself uses recent versions.

### 5. `logos` 0.15 vs 0.16
The `logos` crate is used by `hickory-dns` / `hickory-proto`. Some deps are on 0.15, others on 0.16.

## Actionable Recommendations

### Quick Wins (Low Effort, Low Risk)

| Action | Expected Impact | Effort |
|--------|----------------|--------|
| Add `skip` entries in `deny.toml` for known-harmless duplicates | Cleans `cargo-deny` output only | 10 min |
| Run `cargo update` periodically | May resolve some duplicates automatically | 5 min |

### Medium Effort (Moderate Cache Impact)

| Action | Expected Impact | Effort |
|--------|----------------|--------|
| Bump `quickcheck-ext` to use `quickcheck` that supports `rand 0.10` | Removes `rand 0.8` branch | Medium |
| Update `hickory-proto` / `hickory-resolver` when upstream unifies `logos` | Removes `logos` 0.15 | Low (wait for upstream) |
| Align workspace `thiserror` to 2.x everywhere | Removes `thiserror` 1.x | Low (check if any crate still needs 1.x) |

### High Effort (Biggest Cache Impact, But Risky)

| Action | Expected Impact | Effort |
|--------|----------------|--------|
| Update `webrtc` ecosystem crates to `rand 0.10` | Removes **4** `rand_core`, **4** `rand`, **4** `getrandom`, **3** `rand_chacha` | High — requires upstream webrtc changes |
| Unify crypto crate versions (`signature`, `elliptic-curve`, `p256`, `sha2`, `digest`) | Removes ~15 duplicate crypto crates | High — may require `patch` entries or upstream PRs |
| Force `syn` / `proc-macro2` unification via `[patch]` | Removes proc-macro duplicates | Medium-High — may break proc-macro crates |

## Suggested `deny.toml` Additions

If the goal is to silence `cargo-deny` warnings for duplicates that are currently unavoidable, add `skip` entries:

```toml
[bans]
multiple-versions = "warn"
# Skip duplicates that are transitive and hard to unify
skip = [
    # rand ecosystem — will resolve once all upstream crates migrate to 0.10
    { name = "rand", version = "0.8" },
    { name = "rand", version = "0.7" },
    { name = "rand_core", version = "0.6" },
    { name = "rand_core", version = "0.5" },
    { name = "rand_chacha", version = "0.3" },
    { name = "getrandom", version = "0.2" },
    { name = "getrandom", version = "0.1" },
    # thiserror transition
    { name = "thiserror", version = "1" },
    { name = "thiserror-impl", version = "1" },
    # crypto crate transitions
    { name = "sha2", version = "0.10" },
    { name = "digest", version = "0.10" },
    { name = "signature", version = "2" },
    { name = "elliptic-curve", version = "0.13" },
    { name = "p256", version = "0.13" },
    { name = "ecdsa", version = "0.16" },
    { name = "sec1", version = "0.7" },
    { name = "spki", version = "0.7" },
    { name = "pkcs8", version = "0.10" },
    # logos transition
    { name = "logos", version = "0.15" },
    { name = "logos-codegen", version = "0.15" },
    { name = "logos-derive", version = "0.15" },
]
```

## Cache Impact Estimate

`rust-cache` stores:
- `~/.cargo/registry/cache/` — downloaded crate sources
- `target/` — compiled artifacts

Each duplicate version is downloaded and compiled independently. For the 59 duplicates:
- **Proc-macros** (`syn`, `proc-macro2`, `thiserror-impl`, `logos-derive`) are compiled for the host and each target. They are the most expensive per-byte.
- **Crypto crates** (`sha2`, `curve25519-dalek`, etc.) compile a lot of SIMD / assembly code and produce large `.rlib` files.
- **`rand` ecosystem** is pulled in by many crates; 4 versions of `rand` + `rand_core` + `getrandom` multiply across the dependency tree.

**Rough estimate**: eliminating the top 20 duplicates could reduce the cached `target/` directory by **200–400 MB** and cut cold-build cache-restore times by **30–60 seconds**.

## Conclusion

The cache bloat is a **transitive dependency problem**, not a workspace configuration problem. The biggest wins require upstream crates (`webrtc`, `quickcheck`, `hickory-dns`, various crypto crates) to migrate to unified versions. 

## Q&A: cargo-deny and Cache Factory

### Is the cargo-deny failure difficult to resolve?
**No.** The recent `cargo-deny` advisory failure ([RUSTSEC-2026-0258](https://rustsec.org/advisories/RUSTSEC-2026-0258)) was caused by an outdated `h2` crate (`0.4.15`). It was resolved with a single `cargo update -p h2` bumping it to `0.4.19`. This is a routine lockfile maintenance task, not a structural dependency problem.

### Does cargo-deny affect the cache-factory CI?
**No.** The `cargo-deny` job runs independently in `ci.yml`. It performs static analysis on `Cargo.lock` and `deny.toml` without compiling the workspace. The `cache-factory.yml` workflow is unaffected by cargo-deny results.

### Do duplicate crates bloat the cache-factory cache?
**Yes.** Every duplicate version is downloaded into `~/.cargo/registry/cache/` and compiled into `target/` during cache-factory runs. The 59 duplicate crate names significantly inflate the cache tarball that `rust-cache` uploads. However, the cache-factory job itself completes successfully; the bloat manifests as longer cache-restore times in downstream CI jobs and higher GitHub Actions storage usage.

**Recommended priority:**
1. Accept current duplicates and add `skip` entries to `deny.toml` to silence warnings.
2. Periodically run `cargo update` to pick up upstream unification.
3. File upstream issues / PRs for the `webrtc` and `quickcheck` ecosystems to move to `rand 0.10`.
4. Revisit this report after the next major dependency bump cycle.
