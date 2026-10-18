// Builds the Lua the bridge writes, for feeding test snapshots to the addon.
// The bridge's own serializer lives in bridge/src/lua.rs; bridge/tests/e2e.rs
// checks its real output against the addon.

export type LuaValue = null | boolean | number | string | LuaValue[] | { [key: string]: LuaValue };

const IDENTIFIER = /^[A-Za-z_][A-Za-z0-9_]*$/;
const LUA_KEYWORDS = new Set([
  "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in",
  "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
]);

function luaString(value: string): string {
  return (
    '"' +
    value.replace(/[\\"\x00-\x1f\x7f]/g, (ch) => {
      if (ch === "\\") return "\\\\";
      if (ch === '"') return '\\"';
      if (ch === "\n") return "\\n";
      if (ch === "\r") return "\\r";
      if (ch === "\t") return "\\t";
      return "\\" + ch.charCodeAt(0).toString().padStart(3, "0");
    }) +
    '"'
  );
}

const luaKey = (key: string) => (IDENTIFIER.test(key) && !LUA_KEYWORDS.has(key) ? key : `[${luaString(key)}]`);

export function toLua(value: LuaValue | undefined): string {
  if (value === null || value === undefined) return "nil";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") return String(value);
  if (typeof value === "string") return luaString(value);
  if (Array.isArray(value)) return `{${value.map((item) => toLua(item)).join(", ")}}`;
  const entries = Object.entries(value).filter(([, v]) => v !== undefined && v !== null);
  return `{${entries.map(([k, v]) => `${luaKey(k)} = ${toLua(v)}`).join(", ")}}`;
}

/** The Data.lua a bridge-owned addon runs (see `data_chunk` in bridge/src/wow.rs). */
export function dataChunk(luaTable: string, source: "inbox" | "live"): string {
  return `-- Written by the T3 WoW bridge. Do not edit.\nlocal snapshot = ${luaTable}\nif T3Wow_Receive then T3Wow_Receive(snapshot, "${source}") end\n`;
}
