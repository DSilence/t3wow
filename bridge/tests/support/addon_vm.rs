//! Runs the real addon in Lua 5.1 with the WoW API stubs from addon/test,
//! to check that it understands what the bridge writes.

use std::fs;
use std::path::{Path, PathBuf};

use mlua::{Lua, Table, Value};

pub fn addon_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../addon/T3Wow")
}

pub struct AddonVm {
    lua: Lua,
}

impl AddonVm {
    /// The WoW stubs and the addon's TOC files, then SavedVariables like the client loads them.
    pub fn load(saved_variables: &str) -> Self {
        let vm = Self { lua: Lua::new() };
        let stub = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../addon/test/wowstub.lua")).unwrap();
        vm.lua.load(stub).set_name("@wowstub.lua").exec().unwrap();
        vm.run("__ns = {}; __stub.liveSlots = 0");
        let toc = fs::read_to_string(addon_dir().join("T3Wow.toc")).unwrap();
        for file in toc.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            vm.run_addon_file(&fs::read_to_string(addon_dir().join(file)).unwrap(), file);
        }
        vm.run(saved_variables);
        vm
    }

    /// Run a chunk with `...` = (addon name, shared namespace table) like WoW does.
    pub fn run_addon_file(&self, source: &str, name: &str) {
        let ns: Table = self.lua.globals().get("__ns").unwrap();
        self.lua.load(source).set_name(format!("@{name}")).call::<()>(("T3Wow", ns)).unwrap();
    }

    pub fn run(&self, code: &str) {
        self.lua.load(code).exec().unwrap();
    }

    pub fn string(&self, expr: &str) -> String {
        match self.lua.load(format!("return {expr}")).eval::<Value>().unwrap() {
            Value::String(s) => s.to_str().unwrap().to_string(),
            other => panic!("{expr} is {other:?}, not a string"),
        }
    }

    pub fn number(&self, expr: &str) -> f64 {
        self.lua.load(format!("return {expr}")).eval::<f64>().unwrap()
    }
}
