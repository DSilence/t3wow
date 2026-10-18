import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

// @ts-expect-error fengari ships no type declarations
import fengari from "fengari";

const { lua, lauxlib, lualib, to_luastring, to_jsstring } = fengari;

const here = path.dirname(fileURLToPath(import.meta.url));
export const ADDON_DIR = path.resolve(here, "../T3Wow");

/** Files listed in the addon's TOC, in load order. */
export function tocFiles(): string[] {
  return fs
    .readFileSync(path.join(ADDON_DIR, "T3Wow.toc"), "utf8")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== "" && !line.startsWith("#"));
}

/** A Lua state (Lua 5.3 via fengari; the addon sticks to the 5.1 subset). */
export class LuaVM {
  readonly L = lauxlib.luaL_newstate();

  constructor() {
    lualib.luaL_openlibs(this.L);
  }

  private check(status: number) {
    if (status !== lua.LUA_OK) {
      const message = to_jsstring(lua.lua_tostring(this.L, -1) ?? to_luastring("?"));
      lua.lua_pop(this.L, 1);
      throw new Error(message);
    }
  }

  /** Run a chunk with `...` = (addon name, shared namespace table) like WoW does. */
  runAddonFile(source: string, chunkName: string) {
    this.check(lauxlib.luaL_loadbuffer(this.L, to_luastring(source), null, to_luastring(`@${chunkName}`)));
    lua.lua_pushstring(this.L, to_luastring("T3Wow"));
    lua.lua_getglobal(this.L, to_luastring("__ns"));
    this.check(lua.lua_pcall(this.L, 2, 0, 0));
  }

  /** Run Lua code and return its first result as a JS string/number/boolean/null. */
  run(code: string): string | number | boolean | null {
    this.check(lauxlib.luaL_loadstring(this.L, to_luastring(code)));
    this.check(lua.lua_pcall(this.L, 0, 1, 0));
    const L = this.L;
    let value: string | number | boolean | null = null;
    switch (lua.lua_type(L, -1)) {
      case lua.LUA_TSTRING:
        value = to_jsstring(lua.lua_tostring(L, -1));
        break;
      case lua.LUA_TNUMBER:
        value = lua.lua_tonumber(L, -1);
        break;
      case lua.LUA_TBOOLEAN:
        value = lua.lua_toboolean(L, -1);
        break;
    }
    lua.lua_pop(L, 1);
    return value;
  }
}

/** A VM with the WoW stubs and the addon's TOC files loaded. */
export function loadAddon(options: { liveSlots?: number; savedVariables?: string } = {}): LuaVM {
  const vm = new LuaVM();
  vm.run(fs.readFileSync(path.join(here, "wowstub.lua"), "utf8"));
  vm.run(`__ns = {}; __stub.liveSlots = ${options.liveSlots ?? 0}`);
  for (const file of tocFiles()) {
    vm.runAddonFile(fs.readFileSync(path.join(ADDON_DIR, file), "utf8"), file);
  }
  // WoW loads SavedVariables after the addon's files, right before ADDON_LOADED.
  if (options.savedVariables) vm.run(options.savedVariables);
  return vm;
}
