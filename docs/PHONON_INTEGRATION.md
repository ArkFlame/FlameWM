# Phonon integration: zero-step speech setup

Status: decision recorded 2026-11-08. Scope: how the speech models reach a user who turns speech on.

## Decision

Keep the Python runtime (`fermion-research` for Phonon-2 STT, `ptts` for Phonon 31m TTS) in a private venv. Do not
build a no-Python runtime.

Evidence:

- Phonon-2 is distributed as `phonon-2.bps.tar.zst` (zstd tar holding `model.fermion`). Only the Python package
  `fermion-research` (PyPI 0.2.15, 1,462,526 B, single `py3-none-any` wheel) reads it on Linux and Windows CPUs.
  Its Linux CPU engine imports `torch` and `transformers` (`requires_dist` in the PyPI metadata).
- The Core ML build is macOS only (`FermionResearch/Phonon-2-CoreML`, `Phonon-2.mlpackage`).
- The Android arm64 C engine is a release asset for Android only. No Linux x86-64 native library is published.
- Writing a Rust or C decoder for the five-value FastConformer and TDT graph is a model port, not an integration. Rejected.

Consequence: the venv (about 1.4 GB, documented in `docs/INSTALL.md`, not measured here) remains. Zero-step therefore
means the user never runs a command: enabling speech starts the setup, and the mic uses the result when it is ready.

## Artifacts

| Role | Artifact (upstream) | Bytes | SHA-256 | Licence | Gated |
|---|---|---|---|---|---|
| STT | `FermionResearch/Phonon-2` `phonon-2.bps.tar.zst` at `main` | 163,515,201 | `98125795b6dda72f5c6eee9ba33d19815df65dcb18b50a357bf9f73c9935309e` | CC-BY-4.0 weights (derived from NVIDIA Parakeet TDT 0.6B v3); Apache-2.0 code | no |
| STT unpacked | `model.fermion` in the archive | 177,438,361 | `4b6bfa3a12cc3c4e0a54f2ab3ec4ca7a842b09e5c7ecfc8e7ca0ac6cc8c11468` | same | no |
| TTS | `gradium/phonon-31m-en` at `92741c5b` (4 files, pinned in `speech.rs`) | 65,051,089 | per file in `speech.rs` | Apache-2.0 | no |
| Runtime | `fermion-research` 0.2.15 wheel | 1,462,526 | `21ff7e5dfc6bba40850e82601c16224879f273ea9096a2274f95cec7e6e03ec8` | Apache-2.0 | n/a |

STT archive verification: `curl` of the `main` file gave 163,515,201 bytes; `sha256sum` matched the pinned value.
Upstream records (`SHA256SUMS.txt`, `packed_manifest.json`, `x-linked-etag`) agree. The archive is not extracted by
FlameWM.

## How STT is fetched

`fermion-research` downloads the archive itself through `huggingface_hub` into `FERMION_CACHE_DIR/speech`, which
setup points at `voice/stt-cache`. It checks the archive against its own pinned SHA-256 (`fermion/_catalog.py`) and
resumes interrupted transfers through the Hub's `.incomplete` files. FlameWM does not re-implement this path.
FlameWM's TTS files go through its own `DiskModelStore`: `.part` resume, size and SHA-256 check, atomic rename.

## Flow (zero-step)

0. Startup: `show()` starts `--voice-setup` when speech is on and voice is not ready (first run, restart, or after a failure).
1. Settings: turning on `assistant.speechToText` or `assistant.textToSpeech` runs
   `flamewm-assistant --voice-setup` (`crates/flamewm-settings/src/actions.rs` `start_voice_setup`).
2. Before this change the flag fell through to the GUI and never reached setup. It now dispatches to
   `voice_background::run()` (`crates/flamewm-assistant/src/main.rs`), which runs with 3 attempts and records every
   state in `$XDG_DATA_HOME/flamewm/assistant/voice/status`.
3. Setup: venv, then torch CPU, then `ptts` and `fermion-research`, then TTS files, then STT model, then wrappers,
   then the `ready` marker last. A `ready` marker is required before any feature uses the voice.
4. Mic press: if not ready, starts setup in the window. If ready, records and transcribes.

Progress: `voice/status` lines, and a notice in the assistant window (`voice_setup_ui`).

## Failure paths

| Failure | Detection | User-visible result | Retry |
|---|---|---|---|
| Data dir path contains a space | `Config` error before network | `failed (attempt 1 of 3): configuration error: ...` | no (configuration) |
| `python3` lacks `ensurepip` (Debian/Ubuntu without `python3-venv`) | preflight picks the `Bootstrap` route: `venv --without-pip`, then pinned `get-pip.py` (SHA-256 checked) | none needed; no root | yes, 3 attempts |
| `python3` missing or older than 3.10 | preflight step `checking python3` | one notice: `sudo apt install python3` | no (configuration) |
| `get-pip.py` does not match its pin | `ChecksumMismatch` before anything runs | pip is not installed; status names the file | yes |
| Network to pypi.org, files.pythonhosted.org, download.pytorch.org, huggingface.co down | a step fails | status names the step and the host list | yes, 60 s apart |
| TTS checksum mismatch | `ChecksumMismatch` in `DiskModelStore` | file discarded, retried | yes |
| Setup already running in another process | `flock` on `voice/setup.lock` | left to the first process | n/a |
| Process killed mid-setup | kernel releases `flock`; `ready` absent | next run repeats the unfinished steps | yes |
| `flamewm-assistant` not on `PATH` (dev build) | spawn error | Settings reports `Could not start the voice setup` | manual |

No root is needed on the normal path. Nothing is installed system-wide. Only a missing or too-old `python3` needs a
system package, and that case shows one notice with the command.

## Privacy

- No model is downloaded until a speech toggle is turned on or the mic is pressed.
- Network contacts: `pypi.org`, `files.pythonhosted.org`, `download.pytorch.org`, `huggingface.co`. No audio, transcripts
  or chat text leave the machine during setup. Transcription runs locally on CPU.
- Deleting `$XDG_DATA_HOME/flamewm/assistant/voice` removes the feature.

## Resource bounds

- Setup runs in a one-shot process that exits when done. No always-on thread or daemon is added.
- The venv is about 1.4 GB on disk (documented). Peak memory during STT decode is set by the runtime, not measured here.

## Verified

- Regression test `crates/flamewm-assistant/tests/voice_setup_arg.rs`: runs the real binary with `--voice-setup`.
  Before the fix: `status was ""`, stderr `XOpenDisplay failed; DISPLAY is unset`, exit 101. After: passes, status
  `failed (attempt 1 of 3): configuration error: ...must not contain spaces`.
- Existing unit tests in `voice_setup.rs` and `voice_background.rs` cover the state machine, retries, configuration
  errors and the busy lock, using fake runners and stores.
- The STT archive download and SHA-256 check above.

## Not verified

- A full end-to-end setup on a clean machine (torch and `ptts` installs, and `fermion transcribe` on a real clip).
  Not run here because it installs about 1.4 GB and needs the audio path. Earlier evidence is in `docs/INSTALL.md`
  (Intel i5-11400H, one generated clip).
- Settings toggle to background spawn, end to end in a running session.

## Gaps and follow-ups

- FlameWM does not pin the Phonon-2 archive in `speech.rs`; the pin lives in `fermion-research`. If the runtime version
  changes, the pin is wherever that wheel says.
- A missing `python3` cannot be replaced without root (a `uv`-managed Python would need its own pinned download). It is reported, not worked around.
- `get-pip.py` is pinned by SHA-256. When upstream changes the file, the Bootstrap route fails closed until the pin is updated.
