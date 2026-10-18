-- Encoding for the screenshot channel (game -> bridge without a reload).
--
-- WoW only writes SavedVariables on reload, but an addon may take a
-- screenshot at any time. FastSend.lua draws the bytes below as a grid of
-- colored cells in the top-left corner, takes a screenshot and hides the grid;
-- the bridge decodes the image and deletes it (bridge/src/shot.rs).
--
-- Grid: GRID_WIDTH cells per row, CELL physical pixels each. Row 0 alternates
-- black and white so the reader can measure the cell size. Every later cell
-- carries 3 bits as red, green and blue fully on or off, which survives JPEG.
-- Chunk bytes: "T3W1", message id (u32), index (u8), count (u8), payload
-- length (u16), Adler-32 of the payload (u32), all big-endian, then the payload.
--
-- Pure Lua 5.1 without the bit library, so it runs in tests too.
local _, ns = ...
ns = ns or {}
local S = {}
ns.Shot = S

local floor, char, byte, format, concat = math.floor, string.char, string.byte, string.format, table.concat

S.MAGIC = "T3W1"
S.CELL = 8
S.GRID_WIDTH = 96
S.MAX_ROWS = 64
S.HEADER = 16
S.CAPACITY = floor(S.GRID_WIDTH * S.MAX_ROWS * 3 / 8) - S.HEADER
S.MAX_CHUNKS = 8

local function u32(n)
  return char(floor(n / 16777216) % 256, floor(n / 65536) % 256, floor(n / 256) % 256, n % 256)
end

local function u16(n)
  return char(floor(n / 256) % 256, n % 256)
end

function S.Adler32(text)
  local a, b = 1, 0
  for i = 1, #text do
    a = (a + byte(text, i)) % 65521
    b = (b + a) % 65521
  end
  return b * 65536 + a
end

local ESCAPES = { ['"'] = '\\"', ["\\"] = "\\\\", ["\n"] = "\\n", ["\r"] = "\\r", ["\t"] = "\\t" }

local function jsonString(text)
  return '"' .. text:gsub('[%c"\\]', function(c)
    return ESCAPES[c] or format("\\u%04x", byte(c))
  end) .. '"'
end

--- JSON for plain data: tables with 1..n keys become arrays, others objects.
function S.JSON(value)
  local kind = type(value)
  if kind == "string" then
    return jsonString(value)
  elseif kind == "number" then
    if value ~= value or value == math.huge or value == -math.huge then
      return "null"
    elseif value == floor(value) and math.abs(value) < 2 ^ 53 then
      return format("%d", value)
    end
    return format("%.17g", value)
  elseif kind == "boolean" then
    return tostring(value)
  elseif kind == "table" then
    local parts = {}
    if #value > 0 then
      for i = 1, #value do
        parts[i] = S.JSON(value[i])
      end
      return "[" .. concat(parts, ",") .. "]"
    end
    for key, item in pairs(value) do
      if (type(key) == "string" or type(key) == "number") and type(item) ~= "function" then
        parts[#parts + 1] = jsonString(tostring(key)) .. ":" .. S.JSON(item)
      end
    end
    return "{" .. concat(parts, ",") .. "}"
  end
  return "null"
end

--- Split a payload into chunk byte strings (header + part of the payload).
function S.Chunks(payload, messageId)
  local count = math.max(1, math.ceil(#payload / S.CAPACITY))
  local chunks = {}
  for index = 0, count - 1 do
    local part = payload:sub(index * S.CAPACITY + 1, (index + 1) * S.CAPACITY)
    chunks[index + 1] = S.MAGIC .. u32(messageId) .. char(index, count) .. u16(#part) .. u32(S.Adler32(part)) .. part
  end
  return chunks
end

--- Cell values 0..7 (red = 4, green = 2, blue = 1) for a chunk, row by row.
function S.Cells(bytes)
  local cells, value, used = {}, 0, 0
  for i = 1, #bytes do
    local b = byte(bytes, i)
    for shift = 7, 0, -1 do
      value = value * 2 + floor(b / 2 ^ shift) % 2
      used = used + 1
      if used == 3 then
        cells[#cells + 1] = value
        value, used = 0, 0
      end
    end
  end
  if used > 0 then
    cells[#cells + 1] = value * 2 ^ (3 - used)
  end
  return cells
end

--- The grid for a chunk: rows of GRID_WIDTH cell values, the clock row first.
function S.Layout(bytes)
  local cells = S.Cells(bytes)
  local width = S.GRID_WIDTH
  local grid = { {} }
  for x = 1, width do
    grid[1][x] = x % 2 == 1 and 0 or 7
  end
  for i = 1, math.ceil(#cells / width) * width do
    local row = 2 + floor((i - 1) / width)
    grid[row] = grid[row] or {}
    grid[row][(i - 1) % width + 1] = cells[i] or 0
  end
  return grid
end

return S
