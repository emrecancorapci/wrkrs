//! Value conversion between the contract and JavaScript.
//!
//! The transfer rules mirror the Lua engine: null, booleans, numbers,
//! strings, and plain objects of the same cross the boundary, functions
//! and symbols are rejected.

use rquickjs::IntoJs;
use wrkrs_engine::{EngineError, Value};

/// Converts a contract value into a JavaScript value.
pub fn value_to_js<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: &Value,
) -> Result<rquickjs::Value<'js>, rquickjs::Error> {
    let converted = match value {
        Value::Null => rquickjs::Value::new_null(ctx.clone()),
        Value::Bool(boolean) => (*boolean).into_js(ctx)?,
        Value::Int(integer) => (*integer).into_js(ctx)?,
        Value::Float(float) => (*float).into_js(ctx)?,
        Value::Str(string) => string.as_str().into_js(ctx)?,
        Value::Table(entries) => {
            let object = rquickjs::Object::new(ctx.clone())?;
            for (key, entry) in entries {
                let key = object_key(key)?;
                let entry = value_to_js(ctx, entry)?;
                object.set(key.as_str(), entry)?;
            }
            object.into_value()
        }
    };
    Ok(converted)
}

/// Converts a JavaScript value into a contract value.
pub fn js_to_value(value: &rquickjs::Value<'_>) -> Result<Value, EngineError> {
    if value.is_null() || value.is_undefined() {
        return Ok(Value::Null);
    }
    if value.is_function() || value.is_symbol() {
        return Err(EngineError::UnsupportedValue(format!(
            "'{}'",
            js_type_name(value)
        )));
    }
    if let Some(boolean) = value.as_bool() {
        return Ok(Value::Bool(boolean));
    }
    if let Some(int) = value.as_int() {
        return Ok(Value::Int(i64::from(int)));
    }
    if let Some(float) = value.as_float() {
        return Ok(Value::Float(float));
    }
    if let Some(string) = value.as_string() {
        let string = string
            .to_string()
            .map_err(|error| EngineError::Runtime(error.to_string()))?;
        return Ok(Value::Str(string));
    }
    if let Some(object) = value.as_object() {
        let mut entries = Vec::new();
        for key in object.keys::<String>() {
            let key = key.map_err(|error| EngineError::Runtime(error.to_string()))?;
            let entry: rquickjs::Value = object
                .get(key.as_str())
                .map_err(|error| EngineError::Runtime(error.to_string()))?;
            entries.push((Value::Str(key), js_to_value(&entry)?));
        }
        return Ok(Value::Table(entries));
    }
    Err(EngineError::UnsupportedValue(format!(
        "'{}'",
        js_type_name(value)
    )))
}

/// Renders a table key for a JavaScript object property.
fn object_key(key: &Value) -> Result<String, rquickjs::Error> {
    match key {
        Value::Str(string) => Ok(string.clone()),
        Value::Int(integer) => Ok(integer.to_string()),
        Value::Float(float) => Ok(float.to_string()),
        other => Err(rquickjs::Error::FromJs {
            from: "table key",
            to: "string",
            message: Some(format!("{} keys cannot transfer", other.type_name())),
        }),
    }
}

/// The JavaScript type name for a value.
fn js_type_name(value: &rquickjs::Value<'_>) -> &'static str {
    if value.is_function() {
        "function"
    } else if value.is_symbol() {
        "symbol"
    } else {
        "object"
    }
}

#[cfg(test)]
mod tests {
    use rquickjs::Context;
    use wrkrs_engine::Value;

    fn round_trip(value: Value) -> Value {
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        context.with(|ctx| {
            let converted = super::value_to_js(&ctx, &value).unwrap();
            super::js_to_value(&converted).unwrap()
        })
    }

    #[test]
    fn scalars_round_trip() {
        assert_eq!(round_trip(Value::Null), Value::Null);
        assert_eq!(round_trip(Value::Bool(true)), Value::Bool(true));
        assert_eq!(round_trip(Value::Int(7)), Value::Int(7));
        assert_eq!(round_trip(Value::Float(1.5)), Value::Float(1.5));
        assert_eq!(
            round_trip(Value::Str("name".to_owned())),
            Value::Str("name".to_owned())
        );
    }

    #[test]
    fn objects_round_trip_by_membership() {
        let table = Value::Table(vec![
            (Value::Str("count".to_owned()), Value::Int(3)),
            (Value::Int(1), Value::Bool(false)),
        ]);
        let Value::Table(entries) = round_trip(table) else {
            panic!("expected a table");
        };
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&(Value::Str("count".to_owned()), Value::Int(3))));
        assert!(entries.contains(&(Value::Str("1".to_owned()), Value::Bool(false))));
    }

    #[test]
    fn rejects_functions_with_the_wrk_message() {
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        context.with(|ctx| {
            let function: rquickjs::Value = ctx.eval("(function () {})").unwrap();
            let error = super::js_to_value(&function).unwrap_err();
            assert_eq!(error.to_string(), "cannot transfer 'function' to thread");
        });
    }
}
