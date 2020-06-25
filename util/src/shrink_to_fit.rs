#[macro_export]
macro_rules! shrink_to_fit {
    ($map:expr, $threhold:expr) => {{
        if $map.capacity() > ($map.len() + $threhold) {
            $map.shrink_to_fit();
        }
    }};
}
