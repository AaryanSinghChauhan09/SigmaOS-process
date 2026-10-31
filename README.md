# SigmaOS Process

Rust library for process-related models and APIs. The crate exports process
identity allocation, lifecycle states, process tables, and manager types; its
modules also cover scheduling, spawning, IPC, resource limits, and Linux process
interfaces.

## API and integration limits

The Rust models do not by themselves establish that SigmaOS can create or
isolate host processes, enforce scheduling policy, or perform kernel syscalls.
The process lifecycle tracker is an in-memory state model. Treat OS integration,
resource enforcement, PID synchronization, and crash recovery as unverified
until each path has implementation evidence and target-level validation.

Keep the lifecycle transitions and ownership rules explicit. Process creation,
exit, wait/reap, and resource cleanup need defined failure behavior; scheduling
policy should remain separate from process identity and table bookkeeping.

## Development

The crate depends on `SigmaOS-kernel` through a sibling path dependency. CI
resolves SigmaOS path dependencies and runs `cargo test --all-targets --locked`.
