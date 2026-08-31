#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Defaults
WORKFLOW=""
JOB=""
EVENT="workflow_dispatch"
SECRETS_FILE="${REPO_ROOT}/interop-tests/.secrets"
EVENT_FILE="${REPO_ROOT}/interop-tests/act-event.json"
ARCH="linux/amd64"
VERBOSE=""
LIST_JOBS=""

# Workflow dispatch inputs (for interop)
RUN_CHROMIUM="false"
RUN_NATIVE="false"
RUN_HOLE_PUNCH="false"
WORKER_COUNT="16"
REF_TAG="head"
RUNNER_OS="ubuntu-latest"

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Run GitHub Actions workflows locally using act.

Options:
  -w, --workflow NAME     Workflow to run: interop, ci, cache, docker, docs, macos
  -j, --job JOB_ID        Specific job to run (optional)
  -e, --event EVENT       GitHub event type (default: workflow_dispatch)
  -a, --arch ARCH         Container architecture (default: linux/amd64)
  -s, --secrets FILE      Secrets file path (default: interop-tests/.secrets)
  -v, --verbose           Verbose act output
  -l, --list              List available jobs in the workflow
  -h, --help              Show this help message

Interop-specific options (only with -w interop):
  --chromium              Run chromium interop tests
  --native                Run native interop tests
  --hole-punch            Run hole-punch interop tests
  --worker-count N        Number of test workers (default: 16)
  --ref-tag TAG           Image tag suffix (default: head)
  --runner-os OS          Runner OS: ubuntu-latest, ubuntu-24.04, ubuntu-22.04

Examples:
  $(basename "$0") -w ci -j rustfmt                    # Run rustfmt job from ci.yml
  $(basename "$0") -w interop --native                 # Build and run native interop tests
  $(basename "$0") -w interop --chromium               # Build and run chromium interop tests
  $(basename "$0") -w ci --list                        # List all jobs in ci.yml
  $(basename "$0") -w interop --native --ref-tag v0.1  # Run with custom tag

Note: The interop TEST execution requires upstream libp2p/test-plans
infrastructure (Redis, Docker compose, other implementations). Only
the Docker BUILD steps work reliably locally.
EOF
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        -w|--workflow)
            WORKFLOW="$2"
            shift 2
            ;;
        -j|--job)
            JOB="$2"
            shift 2
            ;;
        -e|--event)
            EVENT="$2"
            shift 2
            ;;
        -a|--arch)
            ARCH="$2"
            shift 2
            ;;
        -s|--secrets)
            SECRETS_FILE="$2"
            shift 2
            ;;
        -v|--verbose)
            VERBOSE="-v"
            shift
            ;;
        -l|--list)
            LIST_JOBS="1"
            shift
            ;;
        --chromium)
            RUN_CHROMIUM="true"
            shift
            ;;
        --native)
            RUN_NATIVE="true"
            shift
            ;;
        --hole-punch)
            RUN_HOLE_PUNCH="true"
            shift
            ;;
        --worker-count)
            WORKER_COUNT="$2"
            shift 2
            ;;
        --ref-tag)
            REF_TAG="$2"
            shift 2
            ;;
        --runner-os)
            RUNNER_OS="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# Map workflow names to files
case "${WORKFLOW}" in
    interop)
        WORKFLOW_FILE="${REPO_ROOT}/.github/workflows/interop-test.yml"
        ;;
    ci)
        WORKFLOW_FILE="${REPO_ROOT}/.github/workflows/ci.yml"
        ;;
    cache)
        WORKFLOW_FILE="${REPO_ROOT}/.github/workflows/cache-factory.yml"
        ;;
    docker)
        WORKFLOW_FILE="${REPO_ROOT}/.github/workflows/docker-image.yml"
        ;;
    docs)
        WORKFLOW_FILE="${REPO_ROOT}/.github/workflows/docs.yml"
        ;;
    macos)
        WORKFLOW_FILE="${REPO_ROOT}/.github/workflows/macos-cross-architecture-ci.yml"
        ;;
    "")
        echo "Error: No workflow specified. Use -w or --workflow."
        usage
        exit 1
        ;;
    *)
        echo "Error: Unknown workflow '${WORKFLOW}'."
        usage
        exit 1
        ;;
esac

# Build act command
ACT_CMD=(act "${EVENT}" -W "${WORKFLOW_FILE}")

# Add job filter if specified
if [[ -n "${JOB}" ]]; then
    ACT_CMD+=(-j "${JOB}")
fi

# Add architecture
ACT_CMD+=(--container-architecture "${ARCH}")

# Add secrets file if it exists
if [[ -f "${SECRETS_FILE}" ]]; then
    ACT_CMD+=(--secret-file "${SECRETS_FILE}")
fi

# Add verbose flag
if [[ -n "${VERBOSE}" ]]; then
    ACT_CMD+=(-v)
fi

# Handle list mode
if [[ -n "${LIST_JOBS}" ]]; then
    echo "Available jobs in ${WORKFLOW_FILE}:"
    "${ACT_CMD[@]}" --list
    exit 0
fi

# For interop workflow, generate event JSON with inputs
if [[ "${WORKFLOW}" == "interop" && "${EVENT}" == "workflow_dispatch" ]]; then
    EVENT_FILE="${REPO_ROOT}/interop-tests/act-event.json"
    cat > "${EVENT_FILE}" <<EOF
{
  "inputs": {
    "run_chromium": ${RUN_CHROMIUM},
    "run_native": ${RUN_NATIVE},
    "run_hole_punch": ${RUN_HOLE_PUNCH},
    "worker_count": ${WORKER_COUNT},
    "ref_tag": "${REF_TAG}",
    "runner_os": "${RUNNER_OS}"
  }
}
EOF
    ACT_CMD+=(-e "${EVENT_FILE}")
fi

# For CI workflow, generate event JSON with test selections if any dispatch inputs are needed
if [[ "${WORKFLOW}" == "ci" && "${EVENT}" == "workflow_dispatch" && -z "${JOB}" ]]; then
    EVENT_FILE="${REPO_ROOT}/interop-tests/act-event-ci.json"
    cat > "${EVENT_FILE}" <<EOF
{
  "inputs": {
    "run_tests": true,
    "run_wasm_tests": true,
    "run_cross": true,
    "run_msrv": true,
    "run_features": true,
    "run_rustdoc": true,
    "run_clippy": true,
    "run_ipfs": true,
    "run_examples": true,
    "run_semver": true,
    "run_rustfmt": true,
    "run_manifest": true,
    "run_proto": true,
    "run_lockfile": true,
    "run_cargo_deny": true
  }
}
EOF
    ACT_CMD+=(-e "${EVENT_FILE}")
fi

echo "Running: ${ACT_CMD[*]}"
"${ACT_CMD[@]}"
