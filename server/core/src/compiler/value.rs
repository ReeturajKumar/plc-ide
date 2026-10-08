use serde::{Deserialize, Serialize};

use super::ast::{BinaryOp, DataType};

/// A runtime value. Each variant is exactly one IEC type. Operations never mix variants;
/// the only conversions are the widenings the checker inserts explicitly (`convert`).
/// Serialized untagged so the frontend sees a plain `true` / `10` / `24.5`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i16),
    DInt(i32),
    Real(f64),
    /// TIME, in milliseconds.
    Time(i64),
}

impl Value {
    pub fn default_for(data_type: DataType) -> Self {
        match data_type {
            DataType::Bool => Value::Bool(false),
            DataType::Int => Value::Int(0),
            DataType::DInt => Value::DInt(0),
            DataType::Real => Value::Real(0.0),
            DataType::Time => Value::Time(0),
        }
    }

    pub fn data_type(self) -> DataType {
        match self {
            Value::Bool(_) => DataType::Bool,
            Value::Int(_) => DataType::Int,
            Value::DInt(_) => DataType::DInt,
            Value::Real(_) => DataType::Real,
            Value::Time(_) => DataType::Time,
        }
    }

    pub fn as_bool(self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    /// INT and DINT as i64 (FOR counters, CASE selectors).
    pub fn as_integer(self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(n.into()),
            Value::DInt(n) => Some(n.into()),
            _ => None,
        }
    }

    /// Store an i64 in an INT or DINT, failing if it doesn't fit.
    pub fn integer(n: i64, data_type: DataType) -> Result<Value, String> {
        match data_type {
            DataType::Int => i16::try_from(n).map(Value::Int).map_err(|_| overflow(DataType::Int)),
            DataType::DInt => i32::try_from(n).map(Value::DInt).map_err(|_| overflow(DataType::DInt)),
            other => Err(format!("{} is not an integer type", other.name())),
        }
    }

    /// The implicit widenings IEC 61131-3 allows: INT → DINT and INT → REAL. Both are exact.
    pub fn convert(self, to: DataType) -> Result<Value, String> {
        match (self, to) {
            (v, t) if v.data_type() == t => Ok(v),
            (Value::Int(n), DataType::DInt) => Ok(Value::DInt(n.into())),
            (Value::Int(n), DataType::Real) => Ok(Value::Real(n.into())),
            (v, t) => Err(format!("Cannot convert {} to {}", v.data_type().name(), t.name())),
        }
    }

    #[allow(clippy::should_implement_trait)] // fallible, unlike `std::ops::Not`
    pub fn not(self) -> Result<Value, String> {
        match self {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            other => Err(format!("NOT requires BOOL, found {}", other.data_type().name())),
        }
    }

    pub fn negate(self) -> Result<Value, String> {
        match self {
            Value::Int(n) => n.checked_neg().map(Value::Int).ok_or_else(|| overflow(DataType::Int)),
            Value::DInt(n) => n.checked_neg().map(Value::DInt).ok_or_else(|| overflow(DataType::DInt)),
            Value::Real(x) => Ok(Value::Real(-x)),
            other => Err(format!("Type error: '-' cannot be applied to {}", other.data_type().name())),
        }
    }

    /// Apply a binary operator. Both operands must have the same type.
    pub fn binary(op: BinaryOp, a: Value, b: Value) -> Result<Value, String> {
        use BinaryOp::*;
        let mismatch = || {
            format!(
                "Type error: '{}' cannot be applied to {} and {}",
                op.symbol(),
                a.data_type().name(),
                b.data_type().name()
            )
        };
        match op {
            And | Or | Xor => match (a, b) {
                (Value::Bool(x), Value::Bool(y)) => Ok(Value::Bool(match op {
                    And => x && y,
                    Or => x || y,
                    _ => x != y,
                })),
                _ => Err(mismatch()),
            },
            Equal | NotEqual => {
                if a.data_type() != b.data_type() {
                    return Err(mismatch());
                }
                Ok(Value::Bool((a == b) == (op == Equal)))
            }
            Less | Greater | LessEqual | GreaterEqual => {
                let ordering = match (a, b) {
                    (Value::Int(x), Value::Int(y)) => x.partial_cmp(&y),
                    (Value::DInt(x), Value::DInt(y)) => x.partial_cmp(&y),
                    (Value::Real(x), Value::Real(y)) => x.partial_cmp(&y),
                    (Value::Time(x), Value::Time(y)) => x.partial_cmp(&y),
                    _ => return Err(mismatch()),
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
                (Value::Int(x), Value::Int(y)) => Value::integer(int_arith(op, x.into(), y.into())?, DataType::Int),
                (Value::DInt(x), Value::DInt(y)) => Value::integer(int_arith(op, x.into(), y.into())?, DataType::DInt),
                // IEC defines MOD for integers only.
                (Value::Real(x), Value::Real(y)) if op != Mod => real_arith(op, x, y).map(Value::Real),
                // Durations add and subtract; scaling a TIME isn't supported yet.
                (Value::Time(x), Value::Time(y)) if matches!(op, Add | Sub) => {
                    let r = if op == Add { x.checked_add(y) } else { x.checked_sub(y) };
                    r.map(Value::Time).ok_or_else(|| overflow(DataType::Time))
                }
                _ => Err(mismatch()),
            },
        }
    }
}

/// Integer arithmetic in i64 (wide enough for any INT/DINT product); the caller
/// narrows back to the operand type, which is where overflow is detected.
fn int_arith(op: BinaryOp, x: i64, y: i64) -> Result<i64, String> {
    match op {
        BinaryOp::Add => Ok(x + y),
        BinaryOp::Sub => Ok(x - y),
        BinaryOp::Mul => Ok(x * y),
        BinaryOp::Div | BinaryOp::Mod if y == 0 => Err("Division by zero".to_string()),
        // IEC integer division truncates toward zero, and MOD takes the dividend's sign;
        // Rust's `/` and `%` do exactly that.
        BinaryOp::Div => Ok(x / y),
        BinaryOp::Mod => Ok(x % y),
        _ => Err(format!("'{}' is not an arithmetic operator", op.symbol())),
    }
}

fn real_arith(op: BinaryOp, x: f64, y: f64) -> Result<f64, String> {
    let result = match op {
        BinaryOp::Add => x + y,
        BinaryOp::Sub => x - y,
        BinaryOp::Mul => x * y,
        BinaryOp::Div if y == 0.0 => return Err("Division by zero".to_string()),
        BinaryOp::Div => x / y,
        _ => return Err(format!("'{}' is not an arithmetic operator", op.symbol())),
    };
    // Infinity/NaN can't be represented in the monitor (or JSON), so treat them as overflow.
    if result.is_finite() {
        Ok(result)
    } else {
        Err(overflow(DataType::Real))
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

fn overflow(data_type: DataType) -> String {
    let range = match data_type {
        DataType::Int => " (range -32768..32767)",
        DataType::DInt => " (range -2147483648..2147483647)",
        _ => "",
    };
    format!("{} overflow: result out of range{range}", data_type.name())
}
