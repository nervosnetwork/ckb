use std::fmt;

#[derive(Clone, Copy)]
pub struct Size(u64);

#[derive(Clone, Copy)]
pub enum HumanReadableSize {
    Bytes(u64),
    KiBytes(f64),
    MiBytes(f64),
    GiBytes(f64),
}

#[derive(Clone)]
pub enum PropertyValue<T> {
    Value(T),
    Null,
    Error(String),
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", HumanReadableSize::from(*self))
    }
}

impl From<u64> for Size {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl fmt::Display for HumanReadableSize {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Bytes(v) => write!(f, "{} Bytes", v),
            Self::KiBytes(v) => write!(f, "{:.2} KiB", v),
            Self::MiBytes(v) => write!(f, "{:.2} MiB", v),
            Self::GiBytes(v) => write!(f, "{:.2} GiB", v),
        }
    }
}

impl From<Size> for HumanReadableSize {
    fn from(sz: Size) -> Self {
        let v = sz.0;
        match v {
            _ if v < 1024 => Self::Bytes(v),
            _ if v < 1024 * 1024 => Self::KiBytes((v as f64) / 1024.0),
            _ if v < 1024 * 1024 * 1024 => Self::MiBytes((v as f64) / 1024.0 / 1024.0),
            _ => Self::GiBytes((v as f64) / 1024.0 / 1024.0 / 1024.0),
        }
    }
}

impl<T> fmt::Display for PropertyValue<T>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Value(v) => write!(f, "{}", v),
            Self::Null => write!(f, "null"),
            Self::Error(_) => write!(f, "err"),
        }
    }
}

impl<T> fmt::Debug for PropertyValue<T>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self, f)
    }
}

impl<T> Default for PropertyValue<T> {
    fn default() -> Self {
        Self::Null
    }
}

impl<T, V> From<Result<Option<V>, String>> for PropertyValue<T>
where
    V: Into<T>,
{
    fn from(res: Result<Option<V>, String>) -> Self {
        match res {
            Ok(Some(v)) => Self::Value(v.into()),
            Ok(None) => Self::Null,
            Err(e) => Self::Error(e),
        }
    }
}

impl<T> PropertyValue<T> {
    pub fn new<V>(v: V) -> Self
    where
        V: Into<T>,
    {
        Self::Value(v.into())
    }
}

pub fn sum_sizes(values: &[PropertyValue<Size>]) -> PropertyValue<Size> {
    let mut total = 0;
    let mut errors = 0;
    let mut nulls = 0;
    for value in values {
        match value {
            PropertyValue::Value(v) => {
                total += v.0;
            }
            PropertyValue::Null => {
                nulls += 1;
            }
            PropertyValue::Error(_) => {
                errors += 1;
            }
        }
    }
    if errors > 0 || nulls > 0 {
        PropertyValue::Error(format!("{} errors, {} nulls", errors, nulls))
    } else {
        PropertyValue::new(total)
    }
}
