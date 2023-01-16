#[cfg(not(feature = "enable_faketime"))]
#[cfg(test)]
mod tests_realtime {
    use crate::{system_time_as_millis, unix_time_as_millis};

    #[test]
    fn test_get_system_real_timestamp() {
        assert!(cfg!(not(feature = "enable_faketime")));

        let now = system_time_as_millis();
        assert!(unix_time_as_millis() >= now);
    }
}
