#[cfg(feature = "enable_faketime")]
#[cfg(test)]
mod tests_faketime {
    use crate::{faketime, system_time_as_millis, unix_time_as_millis};

    #[test]
    fn test_basic() {
        assert!(cfg!(feature = "enable_faketime"));

        let faketime_guard = faketime();

        faketime_guard.set_faketime(123);
        assert!(unix_time_as_millis() == 123);

        faketime_guard.set_faketime(100);
        assert!(unix_time_as_millis() == 100);

        faketime_guard.disable_faketime();

        let now = system_time_as_millis();
        assert!(unix_time_as_millis() >= now);

        // The faketime_guard was dropped at the end of the scope,
        // then FAKETIME_ENABLED will be set to false
    }

    #[test]
    fn test_get_system_real_timestamp() {
        let now = system_time_as_millis();
        assert!(unix_time_as_millis() >= now);
    }

    #[test]
    fn test_faketime_will_disabled_when_faketime_guard_is_dropped() {
        let now = system_time_as_millis();
        {
            let faketime_guard = faketime();

            faketime_guard.set_faketime(1);
            assert_eq!(unix_time_as_millis(), 1);
        }
        assert!(unix_time_as_millis() >= now);
        {
            let faketime_guard = faketime();

            faketime_guard.set_faketime(2);
            assert_eq!(unix_time_as_millis(), 2);
        }
    }
}
