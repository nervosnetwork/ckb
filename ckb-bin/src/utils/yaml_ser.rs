use std::convert::From;
use std::error::Error;
use std::fmt::{self, Display};
use std::{io, num, str};

use super::printer::TypedStr;
use serde::{de, ser};
use yaml_rust::yaml::{self, Hash, Yaml};

#[derive(Copy, Clone, Debug)]
pub enum EmitError {
    FmtError(fmt::Error),
    #[allow(dead_code)]
    BadHashmapKey,
}

impl Error for EmitError {
    fn description(&self) -> &str {
        match *self {
            EmitError::FmtError(ref err) => err.description(),
            EmitError::BadHashmapKey => "bad hashmap key",
        }
    }

    fn cause(&self) -> Option<&Error> {
        None
    }
}

impl Display for EmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            EmitError::FmtError(ref err) => Display::fmt(err, formatter),
            EmitError::BadHashmapKey => formatter.write_str("bad hashmap key"),
        }
    }
}

impl From<fmt::Error> for EmitError {
    fn from(f: fmt::Error) -> Self {
        EmitError::FmtError(f)
    }
}

// Copy from:
//   https://github.com/chyh1990/yaml-rust/blob/1d29d211e9214f2fe0efaf9379efd998fafdb2de/src/emitter.rs#L40
pub struct YamlEmitter<'a> {
    writer: &'a mut fmt::Write,
    color: bool,
    best_indent: usize,
    compact: bool,

    is_key: bool,
    level: isize,
}

pub type EmitResult = Result<(), EmitError>;

// from serialize::json
fn escape_str(wr: &mut fmt::Write, v: &str, color: bool) -> Result<(), fmt::Error> {
    wr.write_str("\"")?;

    let mut start = 0;

    for (i, byte) in v.bytes().enumerate() {
        let escaped = match byte {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\x00' => "\\u0000",
            b'\x01' => "\\u0001",
            b'\x02' => "\\u0002",
            b'\x03' => "\\u0003",
            b'\x04' => "\\u0004",
            b'\x05' => "\\u0005",
            b'\x06' => "\\u0006",
            b'\x07' => "\\u0007",
            b'\x08' => "\\b",
            b'\t' => "\\t",
            b'\n' => "\\n",
            b'\x0b' => "\\u000b",
            b'\x0c' => "\\f",
            b'\r' => "\\r",
            b'\x0e' => "\\u000e",
            b'\x0f' => "\\u000f",
            b'\x10' => "\\u0010",
            b'\x11' => "\\u0011",
            b'\x12' => "\\u0012",
            b'\x13' => "\\u0013",
            b'\x14' => "\\u0014",
            b'\x15' => "\\u0015",
            b'\x16' => "\\u0016",
            b'\x17' => "\\u0017",
            b'\x18' => "\\u0018",
            b'\x19' => "\\u0019",
            b'\x1a' => "\\u001a",
            b'\x1b' => "\\u001b",
            b'\x1c' => "\\u001c",
            b'\x1d' => "\\u001d",
            b'\x1e' => "\\u001e",
            b'\x1f' => "\\u001f",
            b'\x7f' => "\\u007f",
            _ => continue,
        };

        if start < i {
            wr.write_str(&TypedStr::String(&v[start..i]).render(color))?;
        }

        wr.write_str(&TypedStr::Escaped(escaped).render(color))?;

        start = i + 1;
    }

    if start != v.len() {
        wr.write_str(&TypedStr::String(&v[start..]).render(color))?;
    }

    wr.write_str("\"")?;
    Ok(())
}

impl<'a> YamlEmitter<'a> {
    pub fn new(writer: &'a mut fmt::Write, color: bool) -> YamlEmitter {
        YamlEmitter {
            writer,
            color,
            best_indent: 2,
            compact: true,
            is_key: false,
            level: -1,
        }
    }

    /// Set 'compact inline notation' on or off, as described for block
    /// [sequences](http://www.yaml.org/spec/1.2/spec.html#id2797382)
    /// and
    /// [mappings](http://www.yaml.org/spec/1.2/spec.html#id2798057).
    ///
    /// In this form, blocks cannot have any properties (such as anchors
    /// or tags), which should be OK, because this emitter doesn't
    /// (currently) emit those anyways.
    #[allow(dead_code)]
    pub fn compact(&mut self, compact: bool) {
        self.compact = compact;
    }

    /// Determine if this emitter is using 'compact inline notation'.
    #[allow(dead_code)]
    pub fn is_compact(&self) -> bool {
        self.compact
    }

    pub fn dump(&mut self, doc: &Yaml) -> EmitResult {
        // write DocumentStart
        // writeln!(self.writer, "---")?;
        self.level = -1;
        self.emit_node(doc)
    }

    fn write_indent(&mut self) -> EmitResult {
        if self.level <= 0 {
            return Ok(());
        }
        for _ in 0..self.level {
            for _ in 0..self.best_indent {
                write!(self.writer, " ")?;
            }
        }
        Ok(())
    }

    fn emit_node(&mut self, node: &Yaml) -> EmitResult {
        match *node {
            Yaml::Array(ref v) => self.emit_array(v),
            Yaml::Hash(ref h) => self.emit_hash(h),
            Yaml::String(ref v) => {
                if need_quotes(v) {
                    escape_str(self.writer, v, self.color)?;
                } else {
                    let typed_output = if self.is_key {
                        TypedStr::Key(v.as_str())
                    } else {
                        TypedStr::String(v.as_str())
                    };
                    let output = typed_output.render(self.color);
                    write!(self.writer, "{}", output)?;
                }
                Ok(())
            }
            Yaml::Boolean(v) => {
                let output = TypedStr::Bool(v.to_string().as_str()).render(self.color);
                self.writer.write_str(&output)?;
                Ok(())
            }
            Yaml::Integer(v) => {
                let output = TypedStr::Number(v.to_string().as_str()).render(self.color);
                write!(self.writer, "{}", output)?;
                Ok(())
            }
            Yaml::Real(ref v) => {
                let output = TypedStr::Number(v.to_string().as_str()).render(self.color);
                write!(self.writer, "{}", output)?;
                Ok(())
            }
            Yaml::Null | Yaml::BadValue => {
                let output = TypedStr::Null(Some("~")).render(self.color);
                write!(self.writer, "{}", output)?;
                Ok(())
            }
            // XXX(chenyh) Alias
            _ => Ok(()),
        }
    }

    fn emit_array(&mut self, v: &[Yaml]) -> EmitResult {
        if v.is_empty() {
            write!(self.writer, "[]")?;
        } else {
            self.level += 1;
            for (cnt, x) in v.iter().enumerate() {
                if cnt > 0 {
                    writeln!(self.writer)?;
                    self.write_indent()?;
                }
                write!(self.writer, "-")?;
                self.emit_val(true, x)?;
            }
            self.level -= 1;
        }
        Ok(())
    }

    fn emit_hash(&mut self, h: &Hash) -> EmitResult {
        if h.is_empty() {
            self.writer.write_str("{}")?;
        } else {
            self.level += 1;
            for (cnt, (k, v)) in h.iter().enumerate() {
                let complex_key = match *k {
                    Yaml::Hash(_) | Yaml::Array(_) => true,
                    _ => false,
                };
                if cnt > 0 {
                    writeln!(self.writer)?;
                    self.write_indent()?;
                }
                if complex_key {
                    write!(self.writer, "?")?;
                    self.emit_val(true, k)?;
                    writeln!(self.writer)?;
                    self.write_indent()?;
                    write!(self.writer, ":")?;
                    self.emit_val(true, v)?;
                } else {
                    self.is_key = true;
                    self.emit_node(k)?;
                    self.is_key = false;
                    write!(self.writer, ":")?;
                    self.emit_val(false, v)?;
                }
            }
            self.level -= 1;
        }
        Ok(())
    }

    /// Emit a yaml as a hash or array value: i.e., which should appear
    /// following a ":" or "-", either after a space, or on a new line.
    /// If `inline` is true, then the preceeding characters are distinct
    /// and short enough to respect the compact flag.
    fn emit_val(&mut self, inline: bool, val: &Yaml) -> EmitResult {
        match *val {
            Yaml::Array(ref v) => {
                if (inline && self.compact) || v.is_empty() {
                    write!(self.writer, " ")?;
                } else {
                    writeln!(self.writer)?;
                    self.level += 1;
                    self.write_indent()?;
                    self.level -= 1;
                }
                self.emit_array(v)
            }
            Yaml::Hash(ref h) => {
                if (inline && self.compact) || h.is_empty() {
                    write!(self.writer, " ")?;
                } else {
                    writeln!(self.writer)?;
                    self.level += 1;
                    self.write_indent()?;
                    self.level -= 1;
                }
                self.emit_hash(h)
            }
            _ => {
                write!(self.writer, " ")?;
                self.emit_node(val)
            }
        }
    }
}

/// Check if the string requires quoting.
/// Strings starting with any of the following characters must be quoted.
/// :, &, *, ?, |, -, <, >, =, !, %, @
/// Strings containing any of the following characters must be quoted.
/// {, }, [, ], ,, #, `
///
/// If the string contains any of the following control characters, it must be escaped with double quotes:
/// \0, \x01, \x02, \x03, \x04, \x05, \x06, \a, \b, \t, \n, \v, \f, \r, \x0e, \x0f, \x10, \x11, \x12, \x13, \x14, \x15, \x16, \x17, \x18, \x19, \x1a, \e, \x1c, \x1d, \x1e, \x1f, \N, \_, \L, \P
///
/// Finally, there are other cases when the strings must be quoted, no matter if you're using single or double quotes:
/// * When the string is true or false (otherwise, it would be treated as a boolean value);
/// * When the string is null or ~ (otherwise, it would be considered as a null value);
/// * When the string looks like a number, such as integers (e.g. 2, 14, etc.), floats (e.g. 2.6, 14.9) and exponential numbers (e.g. 12e7, etc.) (otherwise, it would be treated as a numeric value);
/// * When the string looks like a date (e.g. 2014-12-31) (otherwise it would be automatically converted into a Unix timestamp).
fn need_quotes(string: &str) -> bool {
    fn need_quotes_spaces(string: &str) -> bool {
        string.starts_with(' ') || string.ends_with(' ')
    }

    string == ""
        || need_quotes_spaces(string)
        || string.starts_with(|character: char| match character {
            '&' | '*' | '?' | '|' | '-' | '<' | '>' | '=' | '!' | '%' | '@' => true,
            _ => false,
        })
        || string.contains(|character: char| match character {
            ':'
            | '{'
            | '}'
            | '['
            | ']'
            | ','
            | '#'
            | '`'
            | '\"'
            | '\''
            | '\\'
            | '\0'...'\x06'
            | '\t'
            | '\n'
            | '\r'
            | '\x0e'...'\x1a'
            | '\x1c'...'\x1f' => true,
            _ => false,
        })
        || [
            // http://yaml.org/type/bool.html
            // Note: 'y', 'Y', 'n', 'N', is not quoted deliberately, as in libyaml. PyYAML also parse
            // them as string, not booleans, although it is volating the YAML 1.1 specification.
            // See https://github.com/dtolnay/serde-yaml/pull/83#discussion_r152628088.
            "yes", "Yes", "YES", "no", "No", "NO", "True", "TRUE", "true", "False", "FALSE",
            "false", "on", "On", "ON", "off", "Off", "OFF",
            // http://yaml.org/type/null.html
            "null", "Null", "NULL", "~",
        ]
        .contains(&string)
        || string.starts_with('.')
        || string.parse::<i64>().is_ok()
        || string.parse::<f64>().is_ok()
}

#[derive(Debug)]
pub struct SerError {
    inner: String,
}

impl Error for SerError {}

impl Display for SerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl From<String> for SerError {
    fn from(err: String) -> SerError {
        SerError { inner: err }
    }
}

impl ser::Error for SerError {
    fn custom<T: Display>(msg: T) -> Self {
        msg.to_string().into()
    }
}

impl de::Error for SerError {
    fn custom<T: Display>(msg: T) -> Self {
        msg.to_string().into()
    }
}

// Copy from:
//  https://github.com/dtolnay/serde-yaml/blob/c1931f5abd319dc6a793ae211983c9f0572d7c1c/src/ser.rs#L14
pub struct Serializer;

impl ser::Serializer for Serializer {
    type Ok = Yaml;
    type Error = SerError;

    type SerializeSeq = SerializeArray;
    type SerializeTuple = SerializeArray;
    type SerializeTupleStruct = SerializeArray;
    type SerializeTupleVariant = SerializeTupleVariant;
    type SerializeMap = SerializeMap;
    type SerializeStruct = SerializeStruct;
    type SerializeStructVariant = SerializeStructVariant;

    fn serialize_bool(self, v: bool) -> Result<Yaml, Self::Error> {
        Ok(Yaml::Boolean(v))
    }

    fn serialize_i8(self, v: i8) -> Result<Yaml, Self::Error> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i16(self, v: i16) -> Result<Yaml, Self::Error> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i32(self, v: i32) -> Result<Yaml, Self::Error> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i64(self, v: i64) -> Result<Yaml, Self::Error> {
        Ok(Yaml::Integer(v))
    }

    serde::serde_if_integer128! {
        fn serialize_i128(self, v: i128) -> Result<Yaml, Self::Error> {
            if v <= i64::max_value() as i128 && v >= i64::min_value() as i128 {
                self.serialize_i64(v as i64)
            } else {
                Ok(Yaml::Real(v.to_string()))
            }
        }
    }

    fn serialize_u8(self, v: u8) -> Result<Yaml, Self::Error> {
        self.serialize_i64(v as i64)
    }

    fn serialize_u16(self, v: u16) -> Result<Yaml, Self::Error> {
        self.serialize_i64(v as i64)
    }

    fn serialize_u32(self, v: u32) -> Result<Yaml, Self::Error> {
        self.serialize_i64(v as i64)
    }

    fn serialize_u64(self, v: u64) -> Result<Yaml, Self::Error> {
        if v <= i64::max_value() as u64 {
            self.serialize_i64(v as i64)
        } else {
            Ok(Yaml::Real(v.to_string()))
        }
    }

    serde::serde_if_integer128! {
        fn serialize_u128(self, v: u128) -> Result<Yaml, Self::Error> {
            if v <= i64::max_value() as u128 {
                self.serialize_i64(v as i64)
            } else {
                Ok(Yaml::Real(v.to_string()))
            }
        }
    }

    fn serialize_f32(self, v: f32) -> Result<Yaml, Self::Error> {
        self.serialize_f64(v as f64)
    }

    fn serialize_f64(self, v: f64) -> Result<Yaml, Self::Error> {
        Ok(Yaml::Real(match v.classify() {
            num::FpCategory::Infinite if v.is_sign_positive() => ".inf".into(),
            num::FpCategory::Infinite => "-.inf".into(),
            num::FpCategory::Nan => ".nan".into(),
            _ => {
                let mut buf = vec![];
                ::dtoa::write(&mut buf, v).unwrap();
                ::std::str::from_utf8(&buf).unwrap().into()
            }
        }))
    }

    fn serialize_char(self, value: char) -> Result<Yaml, Self::Error> {
        Ok(Yaml::String(value.to_string()))
    }

    fn serialize_str(self, value: &str) -> Result<Yaml, Self::Error> {
        Ok(Yaml::String(value.to_owned()))
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Yaml, Self::Error> {
        let vec = value.iter().map(|&b| Yaml::Integer(b as i64)).collect();
        Ok(Yaml::Array(vec))
    }

    fn serialize_unit(self) -> Result<Yaml, Self::Error> {
        Ok(Yaml::Null)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Yaml, Self::Error> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        _name: &str,
        _variant_index: u32,
        variant: &str,
    ) -> Result<Yaml, Self::Error> {
        Ok(Yaml::String(variant.to_owned()))
    }

    fn serialize_newtype_struct<T: ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Yaml, Self::Error>
    where
        T: ser::Serialize,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized>(
        self,
        _name: &str,
        _variant_index: u32,
        variant: &str,
        value: &T,
    ) -> Result<Yaml, Self::Error>
    where
        T: ser::Serialize,
    {
        Ok(singleton_hash(to_yaml(variant)?, to_yaml(value)?))
    }

    fn serialize_none(self) -> Result<Yaml, Self::Error> {
        self.serialize_unit()
    }

    fn serialize_some<V: ?Sized>(self, value: &V) -> Result<Yaml, Self::Error>
    where
        V: ser::Serialize,
    {
        value.serialize(self)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<SerializeArray, Self::Error> {
        let array = match len {
            None => yaml::Array::new(),
            Some(len) => yaml::Array::with_capacity(len),
        };
        Ok(SerializeArray { array })
    }

    fn serialize_tuple(self, len: usize) -> Result<SerializeArray, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<SerializeArray, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _enum: &'static str,
        _idx: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<SerializeTupleVariant, Self::Error> {
        Ok(SerializeTupleVariant {
            name: variant,
            array: yaml::Array::with_capacity(len),
        })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<SerializeMap, Self::Error> {
        Ok(SerializeMap {
            hash: yaml::Hash::new(),
            next_key: None,
        })
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<SerializeStruct, Self::Error> {
        Ok(SerializeStruct {
            hash: yaml::Hash::new(),
        })
    }

    fn serialize_struct_variant(
        self,
        _enum: &'static str,
        _idx: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<SerializeStructVariant, Self::Error> {
        Ok(SerializeStructVariant {
            name: variant,
            hash: yaml::Hash::new(),
        })
    }
}

#[doc(hidden)]
pub struct SerializeArray {
    array: yaml::Array,
}

#[doc(hidden)]
pub struct SerializeTupleVariant {
    name: &'static str,
    array: yaml::Array,
}

#[doc(hidden)]
pub struct SerializeMap {
    hash: yaml::Hash,
    next_key: Option<yaml::Yaml>,
}

#[doc(hidden)]
pub struct SerializeStruct {
    hash: yaml::Hash,
}

#[doc(hidden)]
pub struct SerializeStructVariant {
    name: &'static str,
    hash: yaml::Hash,
}

impl ser::SerializeSeq for SerializeArray {
    type Ok = yaml::Yaml;
    type Error = SerError;

    fn serialize_element<T: ?Sized>(&mut self, elem: &T) -> Result<(), Self::Error>
    where
        T: ser::Serialize,
    {
        self.array.push(to_yaml(elem)?);
        Ok(())
    }

    fn end(self) -> Result<Yaml, Self::Error> {
        Ok(Yaml::Array(self.array))
    }
}

impl ser::SerializeTuple for SerializeArray {
    type Ok = yaml::Yaml;
    type Error = SerError;

    fn serialize_element<T: ?Sized>(&mut self, elem: &T) -> Result<(), Self::Error>
    where
        T: ser::Serialize,
    {
        ser::SerializeSeq::serialize_element(self, elem)
    }

    fn end(self) -> Result<Yaml, Self::Error> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for SerializeArray {
    type Ok = yaml::Yaml;
    type Error = SerError;

    fn serialize_field<V: ?Sized>(&mut self, value: &V) -> Result<(), Self::Error>
    where
        V: ser::Serialize,
    {
        ser::SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Yaml, Self::Error> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleVariant for SerializeTupleVariant {
    type Ok = yaml::Yaml;
    type Error = SerError;

    fn serialize_field<V: ?Sized>(&mut self, v: &V) -> Result<(), Self::Error>
    where
        V: ser::Serialize,
    {
        self.array.push(to_yaml(v)?);
        Ok(())
    }

    fn end(self) -> Result<Yaml, Self::Error> {
        Ok(singleton_hash(to_yaml(self.name)?, Yaml::Array(self.array)))
    }
}

impl ser::SerializeMap for SerializeMap {
    type Ok = yaml::Yaml;
    type Error = SerError;

    fn serialize_key<T: ?Sized>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: ser::Serialize,
    {
        self.next_key = Some(to_yaml(key)?);
        Ok(())
    }

    fn serialize_value<T: ?Sized>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ser::Serialize,
    {
        match self.next_key.take() {
            Some(key) => self.hash.insert(key, to_yaml(value)?),
            None => panic!("serialize_value called before serialize_key"),
        };
        Ok(())
    }

    fn serialize_entry<K: ?Sized, V: ?Sized>(
        &mut self,
        key: &K,
        value: &V,
    ) -> Result<(), Self::Error>
    where
        K: ser::Serialize,
        V: ser::Serialize,
    {
        self.hash.insert(to_yaml(key)?, to_yaml(value)?);
        Ok(())
    }

    fn end(self) -> Result<Yaml, Self::Error> {
        Ok(Yaml::Hash(self.hash))
    }
}

impl ser::SerializeStruct for SerializeStruct {
    type Ok = yaml::Yaml;
    type Error = SerError;

    fn serialize_field<V: ?Sized>(
        &mut self,
        key: &'static str,
        value: &V,
    ) -> Result<(), Self::Error>
    where
        V: ser::Serialize,
    {
        self.hash.insert(to_yaml(key)?, to_yaml(value)?);
        Ok(())
    }

    fn end(self) -> Result<Yaml, Self::Error> {
        Ok(Yaml::Hash(self.hash))
    }
}

impl ser::SerializeStructVariant for SerializeStructVariant {
    type Ok = yaml::Yaml;
    type Error = SerError;

    fn serialize_field<V: ?Sized>(&mut self, field: &'static str, v: &V) -> Result<(), Self::Error>
    where
        V: ser::Serialize,
    {
        self.hash.insert(to_yaml(field)?, to_yaml(v)?);
        Ok(())
    }

    fn end(self) -> Result<Yaml, Self::Error> {
        Ok(singleton_hash(to_yaml(self.name)?, Yaml::Hash(self.hash)))
    }
}

pub fn to_writer<W, T: ?Sized>(writer: W, value: &T, color: bool) -> Result<(), String>
where
    W: io::Write,
    T: ser::Serialize,
{
    let doc = to_yaml(value).map_err(|err| err.to_string())?;
    let mut writer_adapter = FmtToIoWriter { writer };
    YamlEmitter::new(&mut writer_adapter, color)
        .dump(&doc)
        .map_err(|err| err.to_string())?;
    Ok(())
}

pub fn to_vec<T: ?Sized>(value: &T, color: bool) -> Result<Vec<u8>, String>
where
    T: ser::Serialize,
{
    let mut vec = Vec::with_capacity(128);
    to_writer(&mut vec, value, color).map_err(|err| err.to_string())?;
    Ok(vec)
}

pub fn to_string<T: ?Sized>(value: &T, color: bool) -> Result<String, String>
where
    T: ser::Serialize,
{
    Ok(String::from_utf8(to_vec(value, color)?).map_err(|err| err.to_string())?)
}

fn to_yaml<T>(elem: T) -> Result<Yaml, SerError>
where
    T: ser::Serialize,
{
    elem.serialize(Serializer)
}

fn singleton_hash(k: Yaml, v: Yaml) -> Yaml {
    let mut hash = yaml::Hash::new();
    hash.insert(k, v);
    Yaml::Hash(hash)
}

struct FmtToIoWriter<W> {
    writer: W,
}

impl<W> fmt::Write for FmtToIoWriter<W>
where
    W: io::Write,
{
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.writer.write(s.as_bytes()).is_err() {
            return Err(fmt::Error);
        }
        Ok(())
    }
}
