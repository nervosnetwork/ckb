//! Finite production block-template transaction selection benchmark.
mod allocation_observation;
mod measurement_clock;
mod packing;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    packing::run(&std::env::args().skip(1).collect::<Vec<_>>())
        .map_err(|error| std::io::Error::other(error).into())
}
