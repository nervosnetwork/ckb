//! Exercise the executor's terminal and measurement contracts under Nextest.
#[expect(
    dead_code,
    reason = "Contract tests compile the full executor without starting its node or network."
)]
#[path = "../profile_one_shot.rs"]
mod executor;
