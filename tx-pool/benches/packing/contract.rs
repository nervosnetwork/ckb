//! Run the shared packing contract checks under Nextest process isolation.
#[path = "../allocation_observation/mod.rs"]
mod allocation_observation;
#[path = "../measurement_clock/mod.rs"]
mod measurement_clock;
#[path = "mod.rs"]
mod packing;
