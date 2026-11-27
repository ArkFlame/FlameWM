# Unit p24-fault-test-index
STATUS: OPEN

Write scripts/run-fault-tests (bash, no builds executed by you): a documented script listing and running with cargo test (for the orchestrator to use later) every fault-injection test module added this session (rg -l 'fault' crates --glob '*tests*.rs'), plus scripts/test-wrapper-faults. Document in docs/FAULT_TESTS.md.
