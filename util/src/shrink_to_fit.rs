/// Shrinks the map `$map` when it reserves more than `$threhold` slots for future entries.
///
/// ## Examples
///
/// ```
/// use std::collections::HashMap;
/// use ckb_util::shrink_to_fit;
///
/// let mut h = HashMap::<u32, u32>::new();
/// // Shrink the map when it reserves more than 10 slots for future entries.
/// shrink_to_fit!(h, 10);
/// ```
#[macro_export]
macro_rules! shrink_to_fit {
    ($map:expr, $threhold:expr) => {{
        if $map.capacity() > ($map.len() + $threhold) {
            $map.shrink_to_fit();
        }
    }};
}
