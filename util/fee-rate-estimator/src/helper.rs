use std::{convert::TryInto, fmt, time::Duration};
use time::{macros::format_description, OffsetDateTime};

pub(crate) struct PrettyDisplayNewType<T>(T);

pub(crate) trait PrettyDisplay
where
    Self: Sized,
    PrettyDisplayNewType<Self>: fmt::Display,
{
    fn pretty(self) -> PrettyDisplayNewType<Self> {
        PrettyDisplayNewType(self)
    }
}

impl<T> AsRef<T> for PrettyDisplayNewType<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl PrettyDisplay for Duration {}

impl fmt::Display for PrettyDisplayNewType<Duration> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let format = format_description!(
            "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3] \
            [offset_hour sign:mandatory]:[offset_minute]"
        );
        let ts = self.as_ref().as_nanos().try_into().unwrap();
        let dt = OffsetDateTime::from_unix_timestamp_nanos(ts).unwrap();
        write!(f, "{}", dt.format(format).unwrap())
    }
}
