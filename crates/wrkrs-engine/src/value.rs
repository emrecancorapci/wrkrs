/// A value transferred between host, engine, and thread environments.
///
/// The variant set mirrors what wrk can copy across scripting
/// environments: nil, boolean, number, string, and tables of the same.
/// Functions, userdata, and coroutines cannot be transferred and surface
/// as [`EngineError::UnsupportedValue`](crate::EngineError::UnsupportedValue).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// No value, the scripting equivalent of nil.
    Null,
    /// A true or false value.
    Bool(bool),
    /// An exact integer number.
    Int(i64),
    /// A floating point number.
    Float(f64),
    /// A string of bytes.
    Str(String),
    /// A table as key value pairs in environment iteration order.
    Table(Vec<(Value, Value)>),
}

impl Value {
    /// The scripting VM type name for this value.
    ///
    /// Used to build wrk compatible transfer error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "nil",
            Value::Bool(_) => "boolean",
            Value::Int(_) | Value::Float(_) => "number",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
        }
    }
}
