//! IEC 61131-3 standard functions. Unlike function blocks, functions have no memory:
//! the result depends only on the arguments. Type rules live in the checker; this
//! module defines the names, argument counts and the evaluation itself.

use super::ast::{BinaryOp, DataType};
use super::value::{transfer, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdFunction {
    Abs,
    Sqrt,
    Min,
    Max,
    Trunc,
    Round,
    Sin,
    Cos,
    Shl,
    Shr,
    Rol,
    Ror,
    Len,
    Concat,
    Left,
    Right,
    Mid,
    Find,
    Move,
}

impl StdFunction {
    /// Function names are case-insensitive like every IEC identifier.
    pub fn from_name(name: &str) -> Option<Self> {
        let function = match name.to_ascii_uppercase().as_str() {
            "ABS" => StdFunction::Abs,
            "SQRT" => StdFunction::Sqrt,
            "MIN" => StdFunction::Min,
            "MAX" => StdFunction::Max,
            "TRUNC" => StdFunction::Trunc,
            "ROUND" => StdFunction::Round,
            "SIN" => StdFunction::Sin,
            "COS" => StdFunction::Cos,
            "SHL" => StdFunction::Shl,
            "SHR" => StdFunction::Shr,
            "ROL" => StdFunction::Rol,
            "ROR" => StdFunction::Ror,
            "LEN" => StdFunction::Len,
            "CONCAT" => StdFunction::Concat,
            "LEFT" => StdFunction::Left,
            "RIGHT" => StdFunction::Right,
            "MID" => StdFunction::Mid,
            "FIND" => StdFunction::Find,
            "MOVE" => StdFunction::Move,
            _ => return None,
        };
        Some(function)
    }

    pub fn name(self) -> &'static str {
        match self {
            StdFunction::Abs => "ABS",
            StdFunction::Sqrt => "SQRT",
            StdFunction::Min => "MIN",
            StdFunction::Max => "MAX",
            StdFunction::Trunc => "TRUNC",
            StdFunction::Round => "ROUND",
            StdFunction::Sin => "SIN",
            StdFunction::Cos => "COS",
            StdFunction::Shl => "SHL",
            StdFunction::Shr => "SHR",
            StdFunction::Rol => "ROL",
            StdFunction::Ror => "ROR",
            StdFunction::Len => "LEN",
            StdFunction::Concat => "CONCAT",
            StdFunction::Left => "LEFT",
            StdFunction::Right => "RIGHT",
            StdFunction::Mid => "MID",
            StdFunction::Find => "FIND",
            StdFunction::Move => "MOVE",
        }
    }

    /// Allowed argument count, inclusive. MIN and MAX are extensible, as in IEC.
    pub fn arity(self) -> (usize, usize) {
        match self {
            StdFunction::Min | StdFunction::Max | StdFunction::Concat => (2, usize::MAX),
            StdFunction::Left | StdFunction::Right | StdFunction::Find => (2, 2),
            StdFunction::Mid => (3, 3),
            StdFunction::Shl | StdFunction::Shr | StdFunction::Rol | StdFunction::Ror => (2, 2),
            _ => (1, 1),
        }
    }

    /// Evaluate with arguments already type-checked and converted by the checker.
    pub fn call(self, args: &[Value]) -> Result<Value, String> {
        let first = arg(args)?.clone();
        let data_type = first.data_type();
        let real = || first.as_real().ok_or_else(|| format!("{} expects REAL or LREAL, got {}", self.name(), data_type.name()));
        match self {
            StdFunction::Abs => match (first.as_integer(), first.as_real()) {
                (Some(n), _) => Value::integer(n.abs(), data_type).map_err(|_| overflow(self, data_type)),
                (_, Some(x)) => Ok(Value::real(x.abs(), data_type)),
                _ => Err(format!("ABS expects a number, got {}", data_type.name())),
            },
            StdFunction::Sqrt => {
                let x = real()?;
                if x < 0.0 {
                    return Err(format!("SQRT of a negative number ({x})"));
                }
                Ok(Value::real(x.sqrt(), data_type))
            }
            StdFunction::Sin => Ok(Value::real(real()?.sin(), data_type)),
            // MOVE(x) is x: the assignment `y := MOVE(x);` copies it.
            StdFunction::Move => Ok(first),
            StdFunction::Cos => Ok(Value::real(real()?.cos(), data_type)),
            StdFunction::Trunc => to_dint(self, real()?.trunc()),
            // Halves round away from zero: ROUND(2.5) = 3, ROUND(-2.5) = -3.
            StdFunction::Round => to_dint(self, real()?.round()),
            StdFunction::Min | StdFunction::Max => {
                let keep_new = if self == StdFunction::Min { BinaryOp::Less } else { BinaryOp::Greater };
                args[1..].iter().try_fold(first, |best, v| {
                    Ok(if Value::binary(keep_new, v, &best)?.as_bool() == Some(true) { v.clone() } else { best })
                })
            }
            StdFunction::Shl | StdFunction::Shr | StdFunction::Rol | StdFunction::Ror => {
                let (Some(bits), Some(n)) = (first.as_integer(), args.get(1).and_then(|v| v.as_integer())) else {
                    return Err(format!("{} expects a bit string and a count", self.name()));
                };
                if n < 0 {
                    return Err(format!("{}: negative shift count ({n})", self.name()));
                }
                let width = i128::from(data_type.bits().unwrap_or(64));
                let shifted = match self {
                    StdFunction::Shl if n >= width => 0,
                    StdFunction::Shl => bits << n,
                    StdFunction::Shr if n >= width => 0,
                    StdFunction::Shr => bits >> n,
                    _ => {
                        let n = if self == StdFunction::Rol { n % width } else { (width - n % width) % width };
                        (bits << n) | (bits >> (width - n))
                    }
                };
                Value::integer(transfer(shifted, data_type), data_type)
            }
            StdFunction::Len => Value::integer(text(&first)?.chars().count() as i128, DataType::Int).map_err(|_| overflow(self, DataType::Int)),
            StdFunction::Concat => Ok(Value::text(args.iter().map(text).collect::<Result<String, _>>()?, data_type)),
            StdFunction::Left | StdFunction::Right | StdFunction::Mid => {
                let chars: Vec<char> = text(&first)?.chars().collect();
                let count = |i: usize| -> Result<usize, String> {
                    let n = args.get(i).and_then(Value::as_integer).ok_or("missing argument")?;
                    usize::try_from(n).map_err(|_| format!("{}: negative length or position ({n})", self.name()))
                };
                let len = count(1)?.min(chars.len());
                let start = match self {
                    StdFunction::Left => 0,
                    StdFunction::Right => chars.len() - len,
                    // MID(IN, L, P): L characters from position P (the first is 1).
                    _ => match count(2)? {
                        0 => return Err("MID: positions start at 1".to_string()),
                        p => (p - 1).min(chars.len()),
                    },
                };
                Ok(Value::text(chars[start..(start + len).min(chars.len())].iter().collect(), data_type))
            }
            // FIND(IN1, IN2): where IN2 first occurs in IN1 (the first character is 1), or 0.
            StdFunction::Find => {
                let (haystack, needle) = (text(&first)?, text(args.get(1).ok_or("missing argument")?)?);
                let position = haystack.find(needle).map_or(0, |byte| haystack[..byte].chars().count() + 1);
                Value::integer(position as i128, DataType::Int).map_err(|_| overflow(self, DataType::Int))
            }
        }
    }
}

fn arg(args: &[Value]) -> Result<&Value, String> {
    args.first().ok_or_else(|| "missing argument".to_string())
}

fn text(value: &Value) -> Result<&str, String> {
    value.as_text().ok_or_else(|| format!("expected a STRING, got {}", value.data_type().name()))
}

fn to_dint(f: StdFunction, x: f64) -> Result<Value, String> {
    if x >= f64::from(i32::MIN) && x <= f64::from(i32::MAX) {
        Ok(Value::DInt(x as i32))
    } else {
        Err(overflow(f, DataType::DInt))
    }
}

fn overflow(f: StdFunction, data_type: DataType) -> String {
    format!("{} overflow: result out of range for {}", f.name(), data_type.name())
}

