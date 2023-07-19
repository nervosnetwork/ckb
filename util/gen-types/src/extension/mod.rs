#[cfg(feature = "calc-hash")]
mod calc_hash;
#[cfg(feature = "std")]
mod capacity;
#[cfg(feature = "check-data")]
mod check_data;
#[cfg(feature = "serialized-size")]
mod serialized_size;
#[cfg(feature = "calc-hash")]
mod shortcut;
#[cfg(feature = "std")]
mod std_traits;

#[cfg(test)]
mod tests;
