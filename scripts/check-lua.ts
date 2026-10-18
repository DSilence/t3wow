// Syntax-check the addon as Lua 5.1 and flag accidental global assignments.
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

import luaparse from "luaparse";

const addonDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../addon/T3Wow");
const ALLOWED_GLOBALS = new Set([
  "T3WowDB",
  "T3Wow",
  "T3Wow_Receive",
  "T3Wow_Toggle",
  "SLASH_T3WOW1",
  "SLASH_T3WOW2",
  "BINDING_HEADER_T3WOW",
  "BINDING_NAME_T3WOW_TOGGLE",
]);

let problems = 0;
for (const file of fs.readdirSync(addonDir).filter((f) => f.endsWith(".lua"))) {
  const source = fs.readFileSync(path.join(addonDir, file), "utf8");
  const globals: { name: string; line: number }[] = [];
  try {
    luaparse.parse(source, {
      luaVersion: "5.1",
      scope: true,
      locations: true,
      onCreateNode(node) {
        if (node.type === "AssignmentStatement") {
          for (const target of node.variables) {
            if (target.type === "Identifier" && !(target as { isLocal?: boolean }).isLocal) {
              globals.push({ name: target.name, line: target.loc?.start.line ?? 0 });
            }
          }
        }
        if (node.type === "FunctionDeclaration" && node.identifier?.type === "Identifier" && !node.isLocal) {
          globals.push({ name: node.identifier.name, line: node.loc?.start.line ?? 0 });
        }
      },
    });
  } catch (error) {
    console.error(`${file}: ${(error as Error).message}`);
    problems++;
    continue;
  }
  for (const global of globals) {
    if (!ALLOWED_GLOBALS.has(global.name)) {
      console.error(`${file}:${global.line}: assigns global '${global.name}'`);
      problems++;
    }
  }
}
if (problems > 0) process.exit(1);
console.log("Lua OK");
