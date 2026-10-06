//! IEC 61131-3 standard functions. Unlike function blocks, functions have no memory:
//! the result depends only on the arguments. Type rules live in the checker; this
//! module defines the names, argument counts and the evaluation itself.

use super::ast::{BinaryOp, DataType};
use super::value::Value;

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
        }
    }

    /// Allowed argument count, inclusive. MIN and MAX are extensible, as in IEC.
    pub fn arity(self) -> (usize, usize) {
        match self {
            StdFunction::Min | StdFunction::Max => (2, usize::MAX),
            _ => (1, 1),
        }
    }

    /// Evaluate with arguments already type-checked and converted by the checker.
    pub fn call(self, args: &[Value]) -> Result<Value, String> {
        let real = |v: Value| match v {
            Value::Real(x) => Ok(x),
            other => Err(format!("{} expects REAL, got {}", self.name(), other.data_type().name())),
        };
        match self {
            StdFunction::Abs => match args.first().copied() {
                Some(Value::Int(n)) => n.checked_abs().map(Value::Int).ok_or_else(|| overflow(self, DataType::Int)),
                Some(Value::DInt(n)) => n.checked_abs().map(Value::DInt).ok_or_else(|| overflow(self, DataType::DInt)),
                Some(Value::Real(x)) => Ok(Value::Real(x.abs())),
                other => Err(format!("ABS expects a number, got {other:?}")),
            },
            StdFunction::Sqrt => {
                let x = real(arg(args)?)?;
                if x < 0.0 {
                    return Err(format!("SQRT of a negative number ({x})"));
                }
                Ok(Value::Real(x.sqrt()))
            }
            StdFunction::Sin => Ok(Value::Real(real(arg(args)?)?.sin())),
            StdFunction::Cos => Ok(Value::Real(real(arg(args)?)?.cos())),
            StdFunction::Trunc => to_dint(self, real(arg(args)?)?.trunc()),
            // Halves round away from zero: ROUND(2.5) = 3, ROUND(-2.5) = -3.
            StdFunction::Round => to_dint(self, real(arg(args)?)?.round()),
            StdFunction::Min | StdFunction::Max => {
                let keep_new = if self == StdFunction::Min { BinaryOp::Less } else { BinaryOp::Greater };
                let (first, rest) = args.split_first().ok_or("MIN/MAX needs arguments")?;
                rest.iter().try_fold(*first, |best, &v| {
                    Ok(if Value::binary(keep_new, v, best)?.as_bool() == Some(true) { v } else { best })
                })
            }
        }
    }
}

fn arg(args: &[Value]) -> Result<Value, String> {
    args.first().copied().ok_or_else(|| "missing argument".to_string())
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

