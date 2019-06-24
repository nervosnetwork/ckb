use colored::Colorize;
use serde::ser::Serialize;
use serde_json::ser::{CharEscape, Formatter, Serializer};
use serde_json::value::Value;

use std::io::{Result, Write};
use std::str;

macro_rules! colorize {
    ($s:expr, $color:expr) => {{
        let colored_string = match $color {
            Color::Black => $s.black(),
            Color::Blue => $s.blue(),
            Color::Cyan => $s.cyan(),
            Color::Green => $s.green(),
            Color::Magenta => $s.magenta(),
            Color::Purple => $s.purple(),
            Color::Red => $s.red(),
            Color::White => $s.white(),
            Color::Yellow => $s.yellow(),

            Color::Plain => $s.normal(),
        };

        colored_string.to_string()
    }};
}

/// The set of available colors for the various JSON components.
#[derive(Clone)]
pub enum Color {
    #[allow(dead_code)]
    Black,
    Blue,
    Cyan,
    Green,
    Magenta,
    #[allow(dead_code)]
    Purple,
    Red,
    #[allow(dead_code)]
    White,
    Yellow,

    /// Default color
    Plain,
}

impl Default for Color {
    fn default() -> Self {
        Color::Plain
    }
}

#[derive(Default)]
pub struct ColorizerBuilder {
    null: Color,
    boolean: Color,
    number: Color,
    string: Color,
    key: Color,
    escape_sequence: Color,
}

impl ColorizerBuilder {
    fn new() -> Self {
        Default::default()
    }

    /// Sets the color of the null value.
    pub fn null(&mut self, color: Color) -> &mut Self {
        self.null = color;
        self
    }

    /// Sets the color of boolean values.
    pub fn boolean(&mut self, color: Color) -> &mut Self {
        self.boolean = color;
        self
    }

    /// Sets the color of number values.
    pub fn number(&mut self, color: Color) -> &mut Self {
        self.number = color;
        self
    }

    /// Sets the color of string values.
    pub fn string(&mut self, color: Color) -> &mut Self {
        self.string = color;
        self
    }

    /// Sets the color of JSON object keys.
    pub fn key(&mut self, color: Color) -> &mut Self {
        self.key = color;
        self
    }

    /// Sets the color of escape sequences within string values.
    pub fn escape_sequence(&mut self, color: Color) -> &mut Self {
        self.escape_sequence = color;
        self
    }

    /// Constructs a new Colorizer.
    pub fn build(&self) -> Colorizer {
        Colorizer {
            null: self.null.clone(),
            boolean: self.boolean.clone(),
            number: self.number.clone(),
            string: self.string.clone(),
            key: self.key.clone(),
            escape_sequence: self.escape_sequence.clone(),
            indent_level: 0,
            array_empty: true,
            current_is_key: false,
        }
    }
}

/// A struct representing a specific configuration of colors for the various JSON components.
#[derive(Clone, Default)]
pub struct Colorizer {
    pub null: Color,
    pub boolean: Color,
    pub number: Color,
    pub string: Color,
    pub key: Color,
    escape_sequence: Color,
    indent_level: usize,
    array_empty: bool,
    current_is_key: bool,
}

impl Colorizer {
    /// Start builder a new Colorizer.
    pub fn builder() -> ColorizerBuilder {
        ColorizerBuilder::new()
    }

    /// Creates a new Colorizer with a predefined set of colors for the various JSON components.
    ///
    /// Use this if you want your JSON to be colored, but don't care about the specific colors.
    pub fn arbitrary() -> Self {
        Colorizer::builder()
            .null(Color::Cyan)
            .boolean(Color::Yellow)
            .number(Color::Magenta)
            .string(Color::Green)
            .key(Color::Blue)
            .escape_sequence(Color::Red)
            .build()
    }

    /// Colorize a JSON string. Currently, all strings will be pretty-printed (with indentation and
    /// spacing).
    ///
    /// # Errors
    ///
    /// An error is returned if the string is invalid JSON or an I/O error occurs.
    #[allow(dead_code)]
    pub fn colorize_json_str(&self, s: &str) -> Result<String> {
        let value: Value = ::serde_json::from_str(s)?;
        self.colorize_json_value(&value)
    }

    /// An error is returned if the string is invalid JSON or an I/O error occurs.
    pub fn colorize_json_value(&self, value: &Value) -> Result<String> {
        let vec = self.to_vec(value)?;
        let string = unsafe { String::from_utf8_unchecked(vec) };
        Ok(string)
    }

    fn to_vec<T: ?Sized>(&self, value: &T) -> Result<Vec<u8>>
    where
        T: Serialize,
    {
        let mut writer = Vec::with_capacity(128);

        self.to_writer(&mut writer, value)?;
        Ok(writer)
    }

    fn to_writer<W: ?Sized, T: ?Sized>(&self, writer: &mut W, value: &T) -> Result<()>
    where
        W: Write,
        T: Serialize,
    {
        let mut ser = Serializer::with_formatter(writer, self.clone());
        value.serialize(&mut ser)?;
        Ok(())
    }

    #[inline]
    fn get_indentation(&self) -> String {
        (0..self.indent_level * 2).map(|_| ' ').collect()
    }

    #[inline]
    fn get_string_color(&self) -> &Color {
        if self.current_is_key {
            &self.key
        } else {
            &self.string
        }
    }
}

impl Formatter for Colorizer {
    fn write_null<W: ?Sized>(&mut self, writer: &mut W) -> Result<()>
    where
        W: Write,
    {
        write!(writer, "{}", colorize!("null", &self.null))
    }

    fn write_bool<W: ?Sized>(&mut self, writer: &mut W, value: bool) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.boolean))
    }

    fn write_i8<W: ?Sized>(&mut self, writer: &mut W, value: i8) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.number))
    }

    fn write_i16<W: ?Sized>(&mut self, writer: &mut W, value: i16) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.number))
    }

    fn write_i32<W: ?Sized>(&mut self, writer: &mut W, value: i32) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.number))
    }

    fn write_i64<W: ?Sized>(&mut self, writer: &mut W, value: i64) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.number))
    }

    fn write_u8<W: ?Sized>(&mut self, writer: &mut W, value: u8) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.number))
    }

    fn write_u16<W: ?Sized>(&mut self, writer: &mut W, value: u16) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.number))
    }

    fn write_u32<W: ?Sized>(&mut self, writer: &mut W, value: u32) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.number))
    }

    fn write_u64<W: ?Sized>(&mut self, writer: &mut W, value: u64) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.number))
    }

    fn write_f32<W: ?Sized>(&mut self, writer: &mut W, value: f32) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.number))
    }

    fn write_f64<W: ?Sized>(&mut self, writer: &mut W, value: f64) -> Result<()>
    where
        W: Write,
    {
        let value_as_string = format!("{}", value);
        write!(writer, "{}", colorize!(&value_as_string, &self.number))
    }

    fn begin_string<W: ?Sized>(&mut self, writer: &mut W) -> Result<()>
    where
        W: Write,
    {
        write!(writer, "{}", colorize!("\"", self.get_string_color()))
    }

    fn end_string<W: ?Sized>(&mut self, writer: &mut W) -> Result<()>
    where
        W: Write,
    {
        write!(writer, "{}", colorize!("\"", self.get_string_color()))
    }

    fn write_string_fragment<W: ?Sized>(&mut self, writer: &mut W, fragment: &str) -> Result<()>
    where
        W: Write,
    {
        write!(writer, "{}", colorize!(fragment, self.get_string_color()))
    }

    fn write_char_escape<W: ?Sized>(
        &mut self,
        writer: &mut W,
        char_escape: CharEscape,
    ) -> Result<()>
    where
        W: Write,
    {
        let s = match char_escape {
            CharEscape::Quote => "\\\"",
            CharEscape::ReverseSolidus => "\\\\",
            CharEscape::Solidus => "\\/",
            CharEscape::Backspace => "\\b",
            CharEscape::FormFeed => "\\f",
            CharEscape::LineFeed => "\\n",
            CharEscape::CarriageReturn => "\\r",
            CharEscape::Tab => "\\t",
            CharEscape::AsciiControl(byte) => {
                let hex_digits = [
                    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
                ];

                let mut bytes = "\\u00".to_string();
                bytes.push(hex_digits[(byte >> 4) as usize]);
                bytes.push(hex_digits[(byte & 0xF) as usize]);

                return write!(writer, "{}", colorize!(bytes, &self.escape_sequence));
            }
        };

        write!(writer, "{}", colorize!(s, &self.escape_sequence))
    }

    fn begin_array<W: ?Sized>(&mut self, writer: &mut W) -> Result<()>
    where
        W: Write,
    {
        self.array_empty = true;
        self.indent_level += 1;
        write!(writer, "[")
    }

    fn end_array<W: ?Sized>(&mut self, writer: &mut W) -> Result<()>
    where
        W: Write,
    {
        self.indent_level -= 1;
        if self.array_empty {
            write!(writer, "]")
        } else {
            write!(writer, "\n{}]", self.get_indentation())
        }
    }

    fn begin_array_value<W: ?Sized>(&mut self, writer: &mut W, first: bool) -> Result<()>
    where
        W: Write,
    {
        self.array_empty = false;
        if !first {
            write!(writer, ",")?;
        }

        write!(writer, "\n{}", self.get_indentation())
    }

    fn begin_object_key<W: ?Sized>(&mut self, writer: &mut W, first: bool) -> Result<()>
    where
        W: Write,
    {
        if !first {
            write!(writer, ",")?;
        }

        self.current_is_key = true;

        write!(writer, "\n{}", self.get_indentation())
    }

    fn end_object_key<W: ?Sized>(&mut self, _writer: &mut W) -> Result<()>
    where
        W: Write,
    {
        self.current_is_key = false;
        Ok(())
    }

    fn begin_object_value<W: ?Sized>(&mut self, writer: &mut W) -> Result<()>
    where
        W: Write,
    {
        write!(writer, ": ")
    }

    fn begin_object<W: ?Sized>(&mut self, writer: &mut W) -> Result<()>
    where
        W: Write,
    {
        self.indent_level += 1;
        write!(writer, "{{")
    }

    fn end_object<W: ?Sized>(&mut self, writer: &mut W) -> Result<()>
    where
        W: Write,
    {
        self.indent_level -= 1;
        write!(writer, "\n{}}}", self.get_indentation())
    }
}
