use std::fmt;

pub enum HumanReadableSize {
    Bytes(u64),
    KiBytes(f64),
    MiBytes(f64),
    GiBytes(f64),
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

impl From<u64> for HumanReadableSize {
    fn from(v: u64) -> Self {
        match v {
            _ if v < 1024 => Self::Bytes(v),
            _ if v < 1024 * 1024 => Self::KiBytes((v as f64) / 1024.0),
            _ if v < 1024 * 1024 * 1024 => Self::MiBytes((v as f64) / 1024.0 / 1024.0),
            _ => Self::GiBytes((v as f64) / 1024.0 / 1024.0 / 1024.0),
        }
    }
}

#[derive(Debug, Clone)]
pub enum PropertyValue<T> {
    Value(T),
    Null,
    Error(String),
}

impl PropertyValue<u64> {
    pub(crate) fn as_i64(&self) -> i64 {
        match self {
            Self::Value(v) => *v as i64,
            Self::Null => -1,
            Self::Error(_) => -2,
        }
    }
}

impl fmt::Display for PropertyValue<u64> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Value(v) => write!(f, "{}", HumanReadableSize::from(*v)),
            Self::Null => write!(f, "null"),
            Self::Error(_) => write!(f, "err"),
        }
    }
}

impl<T> From<Result<Option<T>, String>> for PropertyValue<T> {
    fn from(res: Result<Option<T>, String>) -> Self {
        match res {
            Ok(Some(v)) => Self::Value(v),
            Ok(None) => Self::Null,
            Err(e) => Self::Error(e),
        }
    }
}

pub fn sum_int_values(values: &[PropertyValue<u64>]) -> PropertyValue<u64> {
    let mut total = 0;
    let mut errors = 0;
    let mut nulls = 0;
    for value in values {
        match value {
            PropertyValue::Value(v) => {
                total += v;
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
        PropertyValue::Value(total)
    }
}
