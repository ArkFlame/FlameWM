# Unit p14-taskmanager-hidden-zero
STATUS: DONE

crates/flamewm-task-manager: verify zero timer wakeups while hidden or minimized, including the /proc sampler thread and any channel tick; fix any residual wake.

## Result

- `crates/flamewm-task-manager/src/lib.rs:1197-1204` — finding: the sampler re-checked `worker_hidden` before the `/proc` walk, but a hide landing *during* the walk still reached `sender.try_send` and `writer.write(&[1])`, producing one residual reactor wake after the window was obscured. change: added a second `worker_hidden` check after `sampler.collect()` that `continue`s before delivering the sample or writing the wake socket; the in-flight sample is dropped so no hidden sample point reaches the graphs. **UNBUILT**. risk: a sample finishing exactly at hide time is discarded; the next visible sample refreshes the table. Visible-state output and pixel layout are unchanged.
- `crates/flamewm-task-manager/src/sample_gate.rs:36-48,71-106` — finding: `wait_visible` parks on `Condvar::wait` with no clock; existing tests already prove an obscured sampler stays parked past a timeout and that `wake()` releases it on shutdown. change: none (verification evidence). risk: none.
- `crates/flamewm-task-manager/src/lib.rs:1205-1248` and `crates/flamewm-ui-x11/src/lib.rs:593-607,1162-1166` — finding: the only channel clock is `recv_timeout(1s)`, entered only while visible; `WorkerMsg::VisibilityChanged` parks the worker on the gate with no timer. The reactor registers read-only fds and has no `register_timer`, so there is no channel tick or reactor timer to suppress while hidden/minimized. change: none (verification evidence). risk: none.

Verification was by source inspection and the existing `SampleGate` tests; no cargo build or test was executed under this unit's no-build rule.

PASS — the hidden/minimized state now has zero timer wakeups: the sampler parks on a clockless condvar, the 1 s channel deadline is only armed while visible, and the one residual wake (a sample completing mid-hide) is suppressed by the post-collect guard. The single production edit is marked UNBUILT and needs the orchestrator's build/test pass.
