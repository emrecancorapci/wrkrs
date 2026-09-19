//! The thread and address userdata scripts see.
//!
//! Ports the wrk.thread metatable from script.c: addr get and set, get,
//! set, and stop, plus the address userdata wrk.lookup returns.

use std::net::SocketAddr;
use std::sync::Arc;

use mlua::{IntoLua, Lua, UserData, UserDataMethods, Value as LuaValue};
use wrkrs_engine::{EngineError, ThreadApi};

use super::value::{lua_to_value, lua_type_name, value_to_lua};
use super::vm_message;

/// An address from wrk.lookup, printable as host and service.
pub struct Address(pub SocketAddr);

impl UserData for Address {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method("__tostring", |_, address, ()| Ok(address.0.to_string()));
    }
}

/// The thread userdata backing thread.addr, thread:get, thread:set,
/// and thread:stop.
pub struct ThreadHandle(pub Arc<dyn ThreadApi>);

impl ThreadHandle {
    /// Wraps an engine error so it crosses the Lua boundary.
    fn lua_error(error: EngineError) -> mlua::Error {
        mlua::Error::RuntimeError(error.to_string())
    }
}

impl UserData for ThreadHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method("__index", |lua, handle, key: String| {
            index_thread(lua, &handle.0, &key)
        });
        methods.add_meta_method(
            "__newindex",
            |_, handle, (key, value): (String, LuaValue)| assign_thread(&handle.0, &key, value),
        );
    }
}

/// Serves a thread field read: get, set, stop, or addr.
fn index_thread(lua: &Lua, api: &Arc<dyn ThreadApi>, key: &str) -> Result<LuaValue, mlua::Error> {
    match key {
        "get" => {
            let api = api.clone();
            let function = lua.create_function(move |lua, (_, name): (LuaValue, String)| {
                let value = api.get_global(&name).map_err(ThreadHandle::lua_error)?;
                value_to_lua(lua, &value).map_err(ThreadHandle::lua_error)
            })?;
            Ok(function.into_lua(lua)?)
        }
        "set" => {
            let api = api.clone();
            let function =
                lua.create_function(move |_, (_, name, value): (LuaValue, String, LuaValue)| {
                    let value = lua_to_value(value).map_err(ThreadHandle::lua_error)?;
                    api.set_global(&name, &value)
                        .map_err(ThreadHandle::lua_error)
                })?;
            Ok(function.into_lua(lua)?)
        }
        "stop" => {
            let api = api.clone();
            let function = lua.create_function(move |_, _: LuaValue| {
                api.stop();
                Ok(())
            })?;
            Ok(function.into_lua(lua)?)
        }
        "addr" => match api.addr() {
            Some(address) => {
                let userdata = lua.create_userdata(Address(address))?;
                Ok(userdata.into_lua(lua)?)
            }
            None => Ok(LuaValue::Nil),
        },
        _ => Ok(LuaValue::Nil),
    }
}

/// Serves a thread field write: only addr is assignable.
fn assign_thread(api: &Arc<dyn ThreadApi>, key: &str, value: LuaValue) -> Result<(), mlua::Error> {
    if key == "addr" {
        let LuaValue::UserData(userdata) = &value else {
            return Err(mlua::Error::RuntimeError("'addr' expected".to_owned()));
        };
        let address = userdata
            .borrow::<Address>()
            .map_err(|error| mlua::Error::RuntimeError(vm_message(&error)))?;
        api.set_addr(address.0);
        return Ok(());
    }
    // wrk reports the type of the assigned value here, not the key,
    // which script.c passes to luaL_typename.
    Err(mlua::Error::RuntimeError(format!(
        "cannot set '{}' on thread",
        lua_type_name(&value)
    )))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::{Address, ThreadHandle};
    use wrkrs_engine::{EngineError, ThreadApi, Value};

    #[derive(Default)]
    struct FakeThread {
        addr: Mutex<Option<std::net::SocketAddr>>,
        stopped: Mutex<bool>,
        globals: Mutex<HashMap<String, Value>>,
    }

    impl ThreadApi for FakeThread {
        fn addr(&self) -> Option<std::net::SocketAddr> {
            *self.addr.lock().unwrap_or_else(|p| p.into_inner())
        }

        fn set_addr(&self, addr: std::net::SocketAddr) {
            *self.addr.lock().unwrap_or_else(|p| p.into_inner()) = Some(addr);
        }

        fn stop(&self) {
            *self.stopped.lock().unwrap_or_else(|p| p.into_inner()) = true;
        }

        fn get_global(&self, name: &str) -> Result<Value, EngineError> {
            Ok(self
                .globals
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(name)
                .cloned()
                .unwrap_or(Value::Null))
        }

        fn set_global(&self, name: &str, value: &Value) -> Result<(), EngineError> {
            self.globals
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(name.to_owned(), value.clone());
            Ok(())
        }
    }

    fn thread_global(lua: &mlua::Lua, api: std::sync::Arc<FakeThread>) {
        let userdata = lua.create_userdata(ThreadHandle(api)).unwrap();
        lua.globals().set("thread", userdata).unwrap();
    }

    fn address(lua: &mlua::Lua, address: std::net::SocketAddr) -> mlua::AnyUserData {
        lua.create_userdata(Address(address)).unwrap()
    }

    #[test]
    fn addresses_print_as_host_and_service() {
        let lua = mlua::Lua::new();
        let userdata = address(&lua, "127.0.0.1:8080".parse().unwrap());
        lua.globals().set("addr", userdata).unwrap();
        let printed: String = lua
            .load("return tostring(addr)")
            .set_name("print_addr")
            .eval()
            .unwrap();
        assert_eq!(printed, "127.0.0.1:8080");

        let userdata = address(&lua, "[::1]:8080".parse().unwrap());
        lua.globals().set("addr", userdata).unwrap();
        let printed: String = lua
            .load("return tostring(addr)")
            .set_name("print_addr")
            .eval()
            .unwrap();
        assert_eq!(printed, "[::1]:8080");
    }

    #[test]
    fn set_and_get_transfer_values() {
        let lua = mlua::Lua::new();
        let api = std::sync::Arc::new(FakeThread::default());
        thread_global(&lua, api.clone());
        lua.load("thread:set(\"id\", 3)").exec().unwrap();
        let id: i64 = lua.load("return thread:get(\"id\")").eval().unwrap();
        assert_eq!(id, 3);
        assert_eq!(api.get_global("id").unwrap(), Value::Int(3));
    }

    #[test]
    fn set_rejects_functions() {
        let lua = mlua::Lua::new();
        thread_global(&lua, std::sync::Arc::new(FakeThread::default()));
        let error = lua.load("thread:set(\"f\", print)").exec().unwrap_err();
        assert!(super::super::vm_message(&error).contains("cannot transfer 'function' to thread"));
    }

    #[test]
    fn stop_stops_the_thread() {
        let lua = mlua::Lua::new();
        let api = std::sync::Arc::new(FakeThread::default());
        thread_global(&lua, api.clone());
        lua.load("thread:stop()").exec().unwrap();
        assert!(*api.stopped.lock().unwrap_or_else(|p| p.into_inner()));
    }

    #[test]
    fn addr_round_trips_through_assignment() {
        let lua = mlua::Lua::new();
        let api = std::sync::Arc::new(FakeThread::default());
        thread_global(&lua, api.clone());
        lua.globals()
            .set("target", address(&lua, "127.0.0.1:9090".parse().unwrap()))
            .unwrap();
        lua.load("thread.addr = target").exec().unwrap();
        assert_eq!(api.addr(), Some("127.0.0.1:9090".parse().unwrap()));
        let printed: String = lua.load("return tostring(thread.addr)").eval().unwrap();
        assert_eq!(printed, "127.0.0.1:9090");
    }

    #[test]
    fn other_keys_cannot_be_assigned() {
        let lua = mlua::Lua::new();
        thread_global(&lua, std::sync::Arc::new(FakeThread::default()));
        let error = lua.load("thread.name = 1").exec().unwrap_err();
        // wrk reports the type of the assigned value, not the key.
        assert_eq!(
            super::super::vm_message(&error),
            "cannot set 'number' on thread"
        );
    }
}
