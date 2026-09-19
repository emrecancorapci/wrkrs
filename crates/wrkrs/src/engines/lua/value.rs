//! Value conversion between the contract and Lua.
//!
//! The transfer rules mirror wrk's script_copy_value: nil, booleans,
//! numbers, strings, and tables of the same cross the boundary and
//! everything else is rejected. Lua strings are byte strings while the
//! contract carries Rust strings, so non UTF-8 bytes become replacement
//! characters during a Lua to contract transfer.

use mlua::{Lua, Value as LuaValue};
use wrkrs_engine::{EngineError, Value};

/// Converts a contract value into a Lua value.
pub fn value_to_lua(lua: &Lua, value: &Value) -> Result<LuaValue, EngineError> {
    let converted = match value {
        Value::Null => LuaValue::Nil,
        Value::Bool(boolean) => LuaValue::Boolean(*boolean),
        Value::Int(integer) => LuaValue::Integer(*integer),
        Value::Float(float) => LuaValue::Number(*float),
        Value::Str(string) => {
            let bytes = lua
                .create_string(string.as_bytes())
                .map_err(|error| EngineError::Runtime(super::vm_message(&error)))?;
            LuaValue::String(bytes)
        }
        Value::Table(entries) => {
            let table = lua
                .create_table()
                .map_err(|error| EngineError::Runtime(super::vm_message(&error)))?;
            for (key, entry) in entries {
                let key = value_to_lua(lua, key)?;
                let entry = value_to_lua(lua, entry)?;
                table
                    .raw_set(key, entry)
                    .map_err(|error| EngineError::Runtime(super::vm_message(&error)))?;
            }
            LuaValue::Table(table)
        }
    };
    Ok(converted)
}

/// Converts a Lua value into a contract value.
///
/// Values the transfer rules reject surface as
/// [`EngineError::UnsupportedValue`] carrying the wrk message shape.
pub fn lua_to_value(value: LuaValue) -> Result<Value, EngineError> {
    let converted = match value {
        LuaValue::Nil => Value::Null,
        LuaValue::Boolean(boolean) => Value::Bool(boolean),
        LuaValue::Integer(integer) => Value::Int(integer),
        LuaValue::Number(number) => Value::Float(number),
        LuaValue::String(string) => Value::Str(string.to_string_lossy().to_string()),
        LuaValue::Table(table) => {
            let mut entries = Vec::new();
            for pair in table.pairs::<LuaValue, LuaValue>() {
                let (key, entry) =
                    pair.map_err(|error| EngineError::Runtime(super::vm_message(&error)))?;
                entries.push((lua_to_value(key)?, lua_to_value(entry)?));
            }
            Value::Table(entries)
        }
        other => {
            return Err(EngineError::UnsupportedValue(format!(
                "'{}'",
                lua_type_name(&other)
            )));
        }
    };
    Ok(converted)
}

/// The Lua type name for a value, as luaL_typename reports it.
fn lua_type_name(value: &LuaValue) -> &'static str {
    match value {
        LuaValue::Nil => "nil",
        LuaValue::Boolean(_) => "boolean",
        LuaValue::LightUserData(_) => "userdata",
        LuaValue::Integer(_) | LuaValue::Number(_) => "number",
        LuaValue::String(_) => "string",
        LuaValue::Table(_) => "table",
        LuaValue::Function(_) => "function",
        LuaValue::Thread(_) => "thread",
        LuaValue::UserData(_) => "userdata",
        LuaValue::Error(_) | LuaValue::Other(_) => "error",
    }
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    use super::{lua_to_value, value_to_lua};
    use wrkrs_engine::Value;

    fn round_trip(value: Value) -> Value {
        let lua = Lua::new();
        let converted = value_to_lua(&lua, &value).unwrap();
        lua_to_value(converted).unwrap()
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
    fn tables_round_trip() {
        let table = Value::Table(vec![
            (Value::Str("count".to_owned()), Value::Int(3)),
            (Value::Int(1), Value::Bool(false)),
        ]);
        let round_tripped = round_trip(table.clone());
        // Lua table iteration order follows hashing, so compare by
        // membership rather than sequence.
        let Value::Table(entries) = round_tripped else {
            panic!("expected a table");
        };
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&(Value::Int(1), Value::Bool(false))));
        assert!(entries.contains(&(Value::Str("count".to_owned()), Value::Int(3))));
    }

    #[test]
    fn lua_numbers_stay_floats() {
        let value = lua_to_value(mlua::Value::Number(2.0)).unwrap();
        assert_eq!(value, Value::Float(2.0));
    }

    #[test]
    fn rejects_functions_with_the_wrk_message() {
        let lua = Lua::new();
        let function: mlua::Value = lua.load("return function() end").eval().unwrap();
        let error = lua_to_value(function).unwrap_err();
        assert_eq!(error.to_string(), "cannot transfer 'function' to thread");
    }

    #[test]
    fn rejects_coroutines_with_the_wrk_message() {
        let lua = Lua::new();
        let coroutine: mlua::Value = lua
            .load("return coroutine.create(function() end)")
            .eval()
            .unwrap();
        let error = lua_to_value(coroutine).unwrap_err();
        assert_eq!(error.to_string(), "cannot transfer 'thread' to thread");
    }
}
