#[cfg(any(feature = "calc-hash", feature = "std"))]
mod calc_hash;
#[cfg(any(feature = "calc-hash", feature = "std"))]
mod shortcut;

#[cfg(any(feature = "check-data", feature = "std"))]
mod check_data;
#[cfg(any(feature = "serialized-size", feature = "std"))]
mod serialized_size;

#[cfg(feature = "std")]
mod capacity;
#[cfg(feature = "std")]
mod std_traits;

#[cfg(test)]
mod tests;
