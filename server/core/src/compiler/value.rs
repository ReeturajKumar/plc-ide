use serde::{Deserialize, Serialize};

use super::ast::{BinaryOp, DataType};

/// A runtime value. Each variant is exactly one IEC type. Operations never mix variants;
/// the only conversions are the ones the checker makes explicit (`convert`).
/// Serialized untagged so the frontend sees a plain `true` / `10` / `24.5` / `"text"`.
// ponytail: 64-bit values travel as JSON numbers, exact in the monitor only up to 2^53.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    SInt(i8),
    Int(i16),
    DInt(i32),
    LInt(i64),
    USInt(u8),
    UInt(u16),
    UDInt(u32),
    ULInt(u64),
    Real(f64),
    LReal(f64),
    Byte(u8),
    Word(u16),
    DWord(u32),
    LWord(u64),
    /// TIME, in milliseconds.
    Time(i64),
    /// DATE, TOD and DT, in milliseconds (see `DataType`).
    Date(i64),
    Tod(i64),
    Dt(i64),
    /// CHAR (a Latin-1 code) and WCHAR (a UTF-16 code unit).
    Char(u8),
    WChar(u16),
    String(String),
    WString(String),
    /// An array's elements (its bounds are in its type) and a structure's members, in order.
    Array(Vec<Value>),
    Struct(Vec<Value>),
    /// POINTER TO / REFERENCE TO: where the target is, or None (null / not bound).
    Pointer(Option<Box<Target>>),
}

/// A place in runtime memory: a variable's slot and the positions of an element in it.
/// Slots are fixed for the life of a loaded project, so a pointer never dangles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub slot: String,
    pub path: Vec<usize>,
}

/// Milliseconds in a day.
pub const DAY_MS: i128 = 86_400_000;

impl Value {
    pub fn default_for(data_type: DataType) -> Self {
        match data_type {
            DataType::Bool => Value::Bool(false),
            DataType::Real | DataType::LReal => Value::real(0.0, data_type),
            DataType::String | DataType::WString => Value::text(String::new(), data_type),
            other => Value::integer(0, other).expect("0 fits every integer type"),
        }
    }

    pub fn data_type(&self) -> DataType {
        match self {
            Value::Bool(_) => DataType::Bool,
            Value::SInt(_) => DataType::SInt,
            Value::Int(_) => DataType::Int,
            Value::DInt(_) => DataType::DInt,
            Value::LInt(_) => DataType::LInt,
            Value::USInt(_) => DataType::USInt,
            Value::UInt(_) => DataType::UInt,
            Value::UDInt(_) => DataType::UDInt,
            Value::ULInt(_) => DataType::ULInt,
            Value::Real(_) => DataType::Real,
            Value::LReal(_) => DataType::LReal,
            Value::Byte(_) => DataType::Byte,
            Value::Word(_) => DataType::Word,
            Value::DWord(_) => DataType::DWord,
            Value::LWord(_) => DataType::LWord,
            Value::Time(_) => DataType::Time,
            Value::Date(_) => DataType::Date,
            Value::Tod(_) => DataType::Tod,
            Value::Dt(_) => DataType::Dt,
            Value::Char(_) => DataType::Char,
            Value::WChar(_) => DataType::WChar,
            Value::String(_) => DataType::String,
            Value::WString(_) => DataType::WString,
            Value::Array(_) => DataType::Array,
            Value::Struct(_) => DataType::Struct,
            Value::Pointer(_) => DataType::Pointer,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    /// Integers and bit strings (unsigned) as i128 (FOR counters, CASE selectors).
    pub fn as_integer(&self) -> Option<i128> {
        match *self {
            Value::SInt(n) => Some(n.into()),
            Value::Int(n) => Some(n.into()),
            Value::DInt(n) => Some(n.into()),
            Value::LInt(n) => Some(n.into()),
            Value::USInt(n) | Value::Byte(n) => Some(n.into()),
            Value::UInt(n) | Value::Word(n) => Some(n.into()),
            Value::UDInt(n) | Value::DWord(n) => Some(n.into()),
            Value::ULInt(n) | Value::LWord(n) => Some(n.into()),
            _ => None,
        }
    }

    /// TIME, DATE, TOD and DT as their milliseconds.
    pub fn as_ms(&self) -> Option<i64> {
        match *self {
            Value::Time(ms) | Value::Date(ms) | Value::Tod(ms) | Value::Dt(ms) => Some(ms),
            _ => None,
        }
    }

    pub fn as_real(&self) -> Option<f64> {
        match *self {
            Value::Real(x) | Value::LReal(x) => Some(x),
            _ => None,
        }
    }

    /// CHAR and WCHAR as their character code.
    pub fn as_char(&self) -> Option<i128> {
        match *self {
            Value::Char(c) => Some(c.into()),
            Value::WChar(c) => Some(c.into()),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::String(s) | Value::WString(s) => Some(s),
            _ => None,
        }
    }

    /// The element or member at `path` (positions, outermost first).
    pub fn at(&self, path: &[usize]) -> Option<&Value> {
        path.iter().try_fold(self, |v, &i| match v {
            Value::Array(items) | Value::Struct(items) => items.get(i),
            _ => None,
        })
    }

    pub fn at_mut(&mut self, path: &[usize]) -> Option<&mut Value> {
        path.iter().try_fold(self, |v, &i| match v {
            Value::Array(items) | Value::Struct(items) => items.get_mut(i),
            _ => None,
        })
    }

    /// A REAL or LREAL.
    pub fn real(x: f64, data_type: DataType) -> Value {
        if data_type == DataType::LReal { Value::LReal(x) } else { Value::Real(x) }
    }

    /// A STRING or WSTRING.
    pub fn text(s: String, data_type: DataType) -> Value {
        if data_type == DataType::WString { Value::WString(s) } else { Value::String(s) }
    }

    /// Store an i128 in an integer, bit string, TIME, date or character type, failing if
    /// it doesn't fit.
    pub fn integer(n: i128, data_type: DataType) -> Result<Value, String> {
        let value = match data_type {
            DataType::SInt => i8::try_from(n).ok().map(Value::SInt),
            DataType::Int => i16::try_from(n).ok().map(Value::Int),
            DataType::DInt => i32::try_from(n).ok().map(Value::DInt),
            DataType::LInt => i64::try_from(n).ok().map(Value::LInt),
            DataType::USInt => u8::try_from(n).ok().map(Value::USInt),
            DataType::UInt => u16::try_from(n).ok().map(Value::UInt),
            DataType::UDInt => u32::try_from(n).ok().map(Value::UDInt),
            DataType::ULInt => u64::try_from(n).ok().map(Value::ULInt),
            DataType::Byte => u8::try_from(n).ok().map(Value::Byte),
            DataType::Word => u16::try_from(n).ok().map(Value::Word),
            DataType::DWord => u32::try_from(n).ok().map(Value::DWord),
            DataType::LWord => u64::try_from(n).ok().map(Value::LWord),
            DataType::Time => i64::try_from(n).ok().map(Value::Time),
            DataType::Date => i64::try_from(n).ok().map(Value::Date),
            DataType::Tod => i64::try_from(n).ok().map(Value::Tod),
            DataType::Dt => i64::try_from(n).ok().map(Value::Dt),
            DataType::Char => u8::try_from(n).ok().map(Value::Char),
            DataType::WChar => u16::try_from(n).ok().map(Value::WChar),
            other => return Err(format!("{} is not an integer type", other.name())),
        };
        value.ok_or_else(|| overflow(data_type))
    }

    /// The value as text: what `*_TO_STRING` gives, e.g. `TRUE`, `-5`, `2.5`, `T#1s`.
    pub fn to_text(&self) -> String {
        match self {
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
            Value::Time(ms) => format_time(*ms),
            Value::Date(ms) | Value::Tod(ms) | Value::Dt(ms) => format_date(self.data_type(), *ms),
            // CHAR is Latin-1, whose codes are the first 256 of Unicode.
            Value::Char(c) => char::from(*c).to_string(),
            Value::WChar(c) => char::from_u32((*c).into()).unwrap_or(char::REPLACEMENT_CHARACTER).to_string(),
            Value::String(s) | Value::WString(s) => s.clone(),
            v => v.as_real().map_or_else(|| v.as_integer().unwrap_or_default().to_string(), |x| x.to_string()),
        }
    }

    /// Text → `to`, for `STRING_TO_*`: numbers, TRUE/FALSE, or a character's code.
    fn parse_text(text: &str, from: DataType, to: DataType) -> Result<Value, String> {
        let invalid = || format!("{}_TO_{}: '{text}' is not a valid {}", from.name(), to.name(), to.name());
        let s = text.trim();
        match to {
            DataType::Bool if s.eq_ignore_ascii_case("TRUE") || s == "1" => Ok(Value::Bool(true)),
            DataType::Bool if s.eq_ignore_ascii_case("FALSE") || s == "0" => Ok(Value::Bool(false)),
            DataType::Char | DataType::WChar => Value::integer(text.chars().next().map_or(0, |c| u32::from(c).into()), to),
            t if t.is_real() => s.parse::<f64>().ok().filter(|x| x.is_finite()).map(|x| Value::real(x, t)).ok_or_else(invalid),
            t if t.is_integer() || t.is_bit_string() => s.parse::<i128>().map_err(|_| invalid()).and_then(|n| Value::integer(n, t)),
            _ => Err(invalid()),
        }
    }

    /// Convert to `to`: the implicit widenings the checker inserts, and the explicit
    /// `*_TO_*` functions. REAL → integer rounds (halves away from zero); integer →
    /// integer is range-checked; to or from a bit string is a binary transfer (the bits,
    /// truncated or zero-extended); BOOL is 0/1, and anything non-zero converts to TRUE.
    /// TIME and the date types convert as their milliseconds; DT_TO_DATE keeps the day
    /// and DT_TO_TOD the time of day. CHAR/WCHAR convert as their code; to STRING is
    /// `to_text`, and from STRING parses.
    pub fn convert(&self, to: DataType) -> Result<Value, String> {
        let from = self.data_type();
        if from == to {
            return Ok(self.clone());
        }
        if matches!(to, DataType::String | DataType::WString) {
            return Ok(Value::text(self.to_text(), to));
        }
        if let Some(text) = self.as_text() {
            return Value::parse_text(text, from, to);
        }
        let out_of_range = || format!("{from}_TO_{to}: value out of range", from = from.name(), to = to.name());
        let n = match (self, self.as_real()) {
            (_, Some(x)) => match to {
                DataType::Bool => return Ok(Value::Bool(x != 0.0)),
                DataType::Real | DataType::LReal => return Ok(Value::real(x, to)),
                // `as` saturates, so out-of-range values fail the range check.
                _ => x.round() as i128,
            },
            (Value::Bool(b), _) => i128::from(*b),
            (v, _) => v.as_ms().map(i128::from).or_else(|| v.as_integer()).or_else(|| v.as_char()).ok_or_else(out_of_range)?,
        };
        match to {
            DataType::Bool => Ok(Value::Bool(n != 0)),
            DataType::Real | DataType::LReal => Ok(Value::real(n as f64, to)),
            DataType::Date => Value::integer(n - n.rem_euclid(DAY_MS), to).map_err(|_| out_of_range()),
            DataType::Tod if from == DataType::Dt => Value::integer(n.rem_euclid(DAY_MS), to),
            DataType::Tod if !(0..DAY_MS).contains(&n) => Err(out_of_range()),
            _ if from.is_bit_string() || to.is_bit_string() => Value::integer(transfer(n, to), to),
            _ => Value::integer(n, to).map_err(|_| out_of_range()),
        }
    }

    #[allow(clippy::should_implement_trait)] // fallible, unlike `std::ops::Not`
    pub fn not(&self) -> Result<Value, String> {
        let data_type = self.data_type();
        match self {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            v if data_type.is_bit_string() => {
                let n = v.as_integer().unwrap_or_default();
                Value::integer(transfer(!n, data_type), data_type)
            }
            _ => Err(format!("NOT requires BOOL or a bit string, found {}", data_type.name())),
        }
    }

    pub fn negate(&self) -> Result<Value, String> {
        let data_type = self.data_type();
        match (self.as_integer(), self.as_real()) {
            (Some(n), _) if data_type.is_integer() => Value::integer(-n, data_type),
            (_, Some(x)) => Ok(Value::real(-x, data_type)),
            _ => Err(format!("Type error: '-' cannot be applied to {}", data_type.name())),
        }
    }

    /// Apply a binary operator. Both operands must have the same type.
    pub fn binary(op: BinaryOp, a: &Value, b: &Value) -> Result<Value, String> {
        use BinaryOp::*;
        let mismatch = || {
            format!(
                "Type error: '{}' cannot be applied to {} and {}",
                op.symbol(),
                a.data_type().name(),
                b.data_type().name()
            )
        };
        if let Some(result) = date_arith(op, a.data_type(), b.data_type()) {
            let (x, y) = (i128::from(a.as_ms().unwrap_or_default()), i128::from(b.as_ms().unwrap_or_default()));
            let r = if op == Add { x + y } else { x - y };
            // A time of day goes round the clock: TOD#23:00 + T#2h = TOD#01:00.
            let r = if result == DataType::Tod { r.rem_euclid(DAY_MS) } else { r };
            return Value::integer(r, result);
        }
        let data_type = a.data_type();
        if data_type != b.data_type() {
            return Err(mismatch());
        }
        match op {
            And | Or | Xor => match (a, b) {
                (Value::Bool(x), Value::Bool(y)) => Ok(Value::Bool(match op {
                    And => *x && *y,
                    Or => *x || *y,
                    _ => x != y,
                })),
                _ if data_type.is_bit_string() => {
                    let (x, y) = (a.as_integer().unwrap_or_default(), b.as_integer().unwrap_or_default());
                    Value::integer(
                        match op {
                            And => x & y,
                            Or => x | y,
                            _ => x ^ y,
                        },
                        data_type,
                    )
                }
                _ => Err(mismatch()),
            },
            Equal | NotEqual => Ok(Value::Bool((a == b) == (op == Equal))),
            Less | Greater | LessEqual | GreaterEqual => {
                let ordering = if let (Some(x), Some(y)) = (a.as_text(), b.as_text()) {
                    Some(x.cmp(y))
                } else {
                    let int = |v: &Value| v.as_integer().or_else(|| v.as_char()).or_else(|| v.as_ms().map(i128::from));
                    match (int(a), int(b), a.as_real(), b.as_real()) {
                        (Some(x), Some(y), _, _) => x.partial_cmp(&y),
                        (_, _, Some(x), Some(y)) => x.partial_cmp(&y),
                        _ => return Err(mismatch()),
                    }
                }
                .ok_or_else(mismatch)?;
                Ok(Value::Bool(match op {
                    Less => ordering.is_lt(),
                    Greater => ordering.is_gt(),
                    LessEqual => ordering.is_le(),
                    _ => ordering.is_ge(),
                }))
            }
            Add | Sub | Mul | Div | Mod => match (a, b) {
                // Durations add and subtract; scaling a TIME isn't supported yet.
                (Value::Time(x), Value::Time(y)) if matches!(op, Add | Sub) => {
                    let r = if op == Add { x.checked_add(*y) } else { x.checked_sub(*y) };
                    r.map(Value::Time).ok_or_else(|| overflow(DataType::Time))
                }
                _ if data_type.is_integer() => {
                    let (x, y) = (a.as_integer().unwrap_or_default(), b.as_integer().unwrap_or_default());
                    Value::integer(int_arith(op, x, y)?.ok_or_else(|| overflow(data_type))?, data_type)
                }
                // IEC defines MOD for integers only.
                (Value::Real(x), Value::Real(y)) | (Value::LReal(x), Value::LReal(y)) if op != Mod => {
                    let r = real_arith(op, *x, *y)?;
                    if r.is_finite() { Ok(Value::real(r, data_type)) } else { Err(overflow(data_type)) }
                }
                _ => Err(mismatch()),
            },
        }
    }
}

/// The result type of `+`/`-` between date types and TIME, if that combination exists:
/// TOD ± TIME → TOD, DT ± TIME → DT, DATE − DATE / TOD − TOD / DT − DT → TIME.
pub fn date_arith(op: BinaryOp, a: DataType, b: DataType) -> Option<DataType> {
    use DataType::*;
    match (op, a, b) {
        (BinaryOp::Add | BinaryOp::Sub, Tod | Dt, Time) => Some(a),
        (BinaryOp::Sub, Date, Date) | (BinaryOp::Sub, Tod, Tod) | (BinaryOp::Sub, Dt, Dt) => Some(Time),
        _ => None,
    }
}

/// `n` as the bit pattern of `to`: two's complement, cut to `to`'s width, read back
/// signed or unsigned as `to` is.
pub fn transfer(n: i128, to: DataType) -> i128 {
    let bits = to.bits().unwrap_or(64);
    let pattern = (n as u128) & ((1u128 << bits) - 1);
    let signed = !(to.is_unsigned() || to.is_bit_string());
    if signed && pattern >> (bits - 1) == 1 { pattern as i128 - (1i128 << bits) } else { pattern as i128 }
}

/// Integer arithmetic in i128 (wide enough for any operands but ULINT × ULINT, which is
/// None like any other overflow); the caller narrows back to the operand type.
fn int_arith(op: BinaryOp, x: i128, y: i128) -> Result<Option<i128>, String> {
    match op {
        BinaryOp::Add => Ok(x.checked_add(y)),
        BinaryOp::Sub => Ok(x.checked_sub(y)),
        BinaryOp::Mul => Ok(x.checked_mul(y)),
        BinaryOp::Div | BinaryOp::Mod if y == 0 => Err("Division by zero".to_string()),
        // IEC integer division truncates toward zero, and MOD takes the dividend's sign;
        // Rust's `/` and `%` do exactly that.
        BinaryOp::Div => Ok(Some(x / y)),
        BinaryOp::Mod => Ok(Some(x % y)),
        _ => Err(format!("'{}' is not an arithmetic operator", op.symbol())),
    }
}

/// Infinity/NaN can't be represented in the monitor (or JSON): the caller treats a
/// non-finite result as overflow.
fn real_arith(op: BinaryOp, x: f64, y: f64) -> Result<f64, String> {
    match op {
        BinaryOp::Add => Ok(x + y),
        BinaryOp::Sub => Ok(x - y),
        BinaryOp::Mul => Ok(x * y),
        BinaryOp::Div if y == 0.0 => Err("Division by zero".to_string()),
        BinaryOp::Div => Ok(x / y),
        _ => Err(format!("'{}' is not an arithmetic operator", op.symbol())),
    }
}

/// IEC-style duration text for messages, e.g. `T#1m30s`, `T#2s500ms`, `T#0ms`.
pub fn format_time(ms: i64) -> String {
    if ms == 0 {
        return "T#0ms".to_string();
    }
    let sign = if ms < 0 { "-" } else { "" };
    let mut rest = ms.unsigned_abs();
    let mut text = format!("T#{sign}");
    for (unit, size) in [("d", 86_400_000), ("h", 3_600_000), ("m", 60_000), ("s", 1_000), ("ms", 1)] {
        if rest >= size {
            text.push_str(&format!("{}{unit}", rest / size));
            rest %= size;
        }
    }
    text
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian (H. Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// (year, month, day) → days since 1970-01-01, or None if there is no such day.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    // 31 February rolls over into March: only a day that maps back to itself exists.
    (civil_from_days(days) == (y + i64::from(m <= 2), m, d)).then_some(days)
}

/// IEC literal text of a date value, e.g. `D#2024-01-31`, `TOD#12:30:05.250`,
/// `DT#2024-01-31-12:30:05`.
pub fn format_date(data_type: DataType, ms: i64) -> String {
    let (days, in_day) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    let (y, m, d) = civil_from_days(days);
    let date = format!("{y:04}-{m:02}-{d:02}");
    let (h, min, s, milli) = (in_day / 3_600_000, in_day / 60_000 % 60, in_day / 1000 % 60, in_day % 1000);
    let time = if milli == 0 { format!("{h:02}:{min:02}:{s:02}") } else { format!("{h:02}:{min:02}:{s:02}.{milli:03}") };
    match data_type {
        DataType::Date => format!("D#{date}"),
        DataType::Tod => format!("TOD#{time}"),
        _ => format!("DT#{date}-{time}"),
    }
}

fn overflow(data_type: DataType) -> String {
    let range = match data_type.range() {
        Some((lo, hi)) if !data_type.is_time_like() => format!(" (range {lo}..{hi})"),
        _ => String::new(),
    };
    format!("{} overflow: result out of range{range}", data_type.name())
}
