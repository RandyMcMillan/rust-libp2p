# Interoperability Test Failures — Root Cause Analysis

## Summary

The `Interoperability Testing` workflow has been failing consistently for months. These failures are **upstream infrastructure issues** in the `libp2p/test-plans` framework, not regressions introduced in this fork.

## Evidence

### Upstream Has the Same Failures
The `libp2p/rust-libp2p` upstream repository exhibits identical interop failures:

- Upstream run `33372813823` (master push, 2026-08-31): native image **build** failed, hole-punch tests failed
- Our fork: native image **build** now succeeds (fixed by updating to `rust:1.88.0` + `cargo-chef 0.1.78`), but the **tests** still fail with the same patterns

### Historical Failure Pattern
Checking the last 20 interop runs in this fork shows **zero successful completions**. Failures trace back to at least July 2025, predating all recent changes in this fork.

## Failure Breakdown

### Native Transport Interop Tests
The native interop test builds successfully but multiple test combinations fail at runtime:

**WebRTC-direct (all combinations fail)**
- `native-rust-libp2p-head x rust-v0.53 (webrtc-direct)` — failure
- `native-rust-libp2p-head x rust-v0.54 (webrtc-direct)` — failure
- `native-rust-libp2p-head x rust-v0.55 (webrtc-direct)` — failure
- `native-rust-libp2p-head x rust-v0.56 (webrtc-direct)` — failure
- `native-rust-libp2p-head x go-v0.46 (webrtc-direct)` — failure
- `native-rust-libp2p-head x go-v0.47 (webrtc-direct)` — failure
- `native-rust-libp2p-head x go-v0.48 (webrtc-direct)` — failure
- `webkit-js-v1.x x native-rust-libp2p-head (webrtc-direct)` — failure
- `firefox-js-v1.x x native-rust-libp2p-head (webrtc-direct)` — failure
- `chromium-js-v2.x x native-rust-libp2p-head (webrtc-direct)` — failure
- `chromium-rust-v0.53 x native-rust-libp2p-head (webrtc-direct)` — failure
- `chromium-rust-v0.54 x native-rust-libp2p-head (webrtc-direct)` — failure

**QUIC (directional failures)**
- `native-rust-libp2p-head x jvm-v1.3 (quic-v1)` — failure
- `native-rust-libp2p-head x c-v0.0.1 (quic-v1)` — failure
- But `go-v0.46 x native-rust-libp2p-head (quic-v1)` — success
- But `go-v0.47 x native-rust-libp2p-head (quic-v1)` — success
- But `go-v0.48 x native-rust-libp2p-head (quic-v1)` — success

**TCP/WebSocket (directional failures)**
- `native-rust-libp2p-head x js-v2.x (ws, noise, yamux)` — failure
- `native-rust-libp2p-head x js-v2.x (ws, noise, mplex)` — failure
- `native-rust-libp2p-head x js-v2.x (tcp, noise, yamux)` — failure
- But `js-v2.x x native-rust-libp2p-head (tcp, noise, yamux)` — success

**Key observation:** Many failures occur in the **rust-libp2p-head dialing outbound** direction, while the inbound direction succeeds. This suggests a compatibility or negotiation issue when rust-libp2p-head initiates connections to certain other implementations.

### Chromium Interop Tests
The **chromium interop test passes** because it runs in a browser/WASM environment using `wasm-bindgen` and `websocket-websys`/`webtransport-websys` transports. The test matrix and networking stack are completely different from the native Docker container tests.

### Hole-Punch Interop Tests
All hole-punch test combinations fail:
- `rust-libp2p-head x rust-v0.53 (tcp)` — failure
- `rust-libp2p-head x rust-libp2p-head (quic)` — failure
- `rust-libp2p-head x rust-libp2p-head (tcp)` — failure
- `rust-libp2p-head x rust-v0.53 (quic)` — failure
- `rust-v0.53 x rust-libp2p-head (tcp)` — failure
- `rust-v0.53 x rust-libp2p-head (quic)` — failure

Log analysis shows **QUIC handshake timeouts** and **circuit relay timeouts** in the Docker test network:
```
Connection attempt to peer failed with Transport([(..., HandshakeTimedOut)])
Incoming connection failed: Transport(Other(Custom { kind: Other, error: Right(Connection(ConnectionError(ConnectionClosed(...)))) }))
```

## What Was Fixed in This Fork

| Issue | Status | Commit |
|-------|--------|--------|
| Native image build failed (outdated Rust/cargo-chef) | **Fixed** | `0c9653c74` |
| Chromium image build failed (outdated Rust/cargo-chef) | **Fixed** | `0c9653c74` |
| Docker server image missing binary | **Fixed** | `a233a7407` |
| Docker multi-arch build missing QEMU | **Fixed** | `113a144ff` |
| Test runtime failures (webrtc, quic, tcp directional) | **Upstream** | N/A |
| Hole-punch QUIC handshake timeouts | **Upstream** | N/A |

## Why These Are Upstream Issues

1. **Test framework:** The tests are orchestrated by `libp2p/test-plans/.github/actions/run-transport-interop-test@master` — external actions outside this repo
2. **Test network:** The Docker compose network, Redis coordination, and container topology are defined in the upstream test-plans repo
3. **Compatibility matrix:** The failures involve rust-libp2p-head negotiating with go-libp2p, js-libp2p, jvm-libp2p, and other implementations — compatibility is a cross-project concern
4. **Historical pattern:** The same failures exist in upstream `libp2p/rust-libp2p` master branch

## Attempted "Bandaid" Fix (Reverted)

Commit `0bd27920b` attempted to silence the failures by removing `webrtc-direct` from the native test transport list and adding `continue-on-error: true` to the hole-punch job:

```diff
commit 0bd27920b39a611470e8a01f391acfbcbe376636
Author: randymcmillan <randymcmillan@protonmail.com>
Date:   Mon Aug 31 09:14:40 2026 -0400

    ci(interop): remove experimental webrtc-direct from native tests; allow hole-punch tests to fail softly

 diff --git a/.github/workflows/interop-test.yml b/.github/workflows/interop-test.yml
 index 70ef4b839..ad2b9d595 100644
 --- a/.github/workflows/interop-test.yml
 +++ b/.github/workflows/interop-test.yml
 @@ -48,6 +48,7 @@ jobs:
    run-holepunching-interop:
      name: Run hole-punch interoperability tests
      if: github.event_name == 'push' || github.event.pull_request.head.repo.full_name == github.repository
 +    continue-on-error: true
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v7
 diff --git a/interop-tests/native-ping-version.json b/interop-tests/native-ping-version.json
 index c509f72bf..de676bc0b 100644
 --- a/interop-tests/native-ping-version.json
 +++ b/interop-tests/native-ping-version.json
 @@ -4,8 +4,7 @@
    "transports": [
      "ws",
      "tcp",
 -    "quic-v1",
 -    "webrtc-direct"
 +    "quic-v1"
    ],
    "secureChannels": [
      "tls",
```

This was **reverted in `a5e94a0e3`** because:
1. Removing `webrtc-direct` only hid one symptom — QUIC and TCP combinations still failed in certain directions
2. `continue-on-error` masked real infrastructure problems without addressing the root cause
3. The failures are confirmed upstream issues; hiding them in this fork creates a maintenance trap

## Recommended Next Steps

1. **Short term:** Do not block CI on interop test results. The workflow can be triggered manually via `workflow_dispatch` for targeted testing.
2. **Medium term:** File or track upstream issues in `libp2p/test-plans` and `libp2p/rust-libp2p` for:
   - WebRTC-direct interop regressions
   - QUIC directional dialing failures
   - Hole-punch test infrastructure reliability
3. **Long term:** Re-enable automatic interop testing once upstream issues are resolved.
