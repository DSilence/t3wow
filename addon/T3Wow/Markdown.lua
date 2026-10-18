-- Markdown to blocks for the message view, with inline formatting as WoW
-- escape codes. Pure Lua (no frame API) so it can be tested outside the game.
--
-- Blocks:
--   { kind = "heading", level = 1..6, text }
--   { kind = "paragraph", text }
--   { kind = "item", level = 0.., marker, text }      list items, nested by indent
--   { kind = "code", lang, text }                     fenced code, escaped but not formatted
--   { kind = "quote", text }
--   { kind = "table", rows = { { cell, ... }, ... }, align = { "left"|"center"|"right", ... } }
--   { kind = "rule" }
-- `text` and table cells are display-ready: `|` escaped, inline Markdown turned
-- into colors and links.
local _, ns = ...
ns = ns or {}
local M = {}
ns.Markdown = M

local concat, insert = table.concat, table.insert

M.COLORS = {
  strong = "ffffff",
  em = "e6d7b0",
  code = "9cdcfe",
  strike = "808080",
  link = "7fb2ff",
}

local function color(hex, text)
  return "|cff" .. hex .. text .. "|r"
end

--- Hyperlink that the view turns into a "copy this URL" dialog when clicked.
local function link(url, label)
  return color(M.COLORS.link, "|Ht3url:" .. url .. "|h" .. label .. "|h")
end

--- Inline Markdown to WoW text: code spans, bold, italics, strikethrough, links.
function M.Inline(raw)
  local text = tostring(raw or ""):gsub("|", "||")
  -- Code spans first, so nothing inside them is formatted.
  local spans = {}
  text = text:gsub("(`+)(.-)%1", function(_, code)
    insert(spans, code)
    return "\001" .. #spans .. "\002"
  end)
  -- Links: [label](url), then bare URLs. Their targets are kept aside too.
  local links = {}
  text = text:gsub("%[([^%]]+)%]%(%s*<?([^%s%)>]+)>?[^%)]*%)", function(label, url)
    insert(links, { url, label })
    return "\003" .. #links .. "\004"
  end)
  text = text:gsub("<?(https?://[%w%-%._~:/%?#%[%]@!%$&'%*%+,;=%%]+)>?", function(url)
    local trailing = url:match("[%.,;:!%?%)]+$") or ""
    url = url:sub(1, #url - #trailing)
    insert(links, { url, url })
    return "\003" .. #links .. "\004" .. trailing
  end)
  text = text:gsub("%*%*(.-)%*%*", function(s) return color(M.COLORS.strong, s) end)
  text = text:gsub("__([^_].-)__", function(s) return color(M.COLORS.strong, s) end)
  text = text:gsub("~~(.-)~~", function(s) return color(M.COLORS.strike, s) end)
  text = text:gsub("%f[%w%*]%*([^%*%s][^%*]-)%*", function(s) return color(M.COLORS.em, s) end)
  text = text:gsub("%f[%w_]_([^_%s][^_]-)_%f[^%w_]", function(s) return color(M.COLORS.em, s) end)
  text = text:gsub("\003(%d+)\004", function(n)
    local entry = links[tonumber(n)]
    return link(entry[1], entry[2])
  end)
  text = text:gsub("\001(%d+)\002", function(n)
    return color(M.COLORS.code, spans[tonumber(n)])
  end)
  return text
end

--- Inline Markdown reduced to plain text (for measuring table columns).
function M.Plain(raw)
  local text = tostring(raw or "")
  text = text:gsub("`+([^`]*)`+", "%1")
  text = text:gsub("%[([^%]]+)%]%([^%)]*%)", "%1")
  text = text:gsub("%*%*(.-)%*%*", "%1"):gsub("__(.-)__", "%1"):gsub("~~(.-)~~", "%1")
  return text
end

--- Number of characters (not bytes) in a UTF-8 string.
function M.Width(text)
  local _, count = tostring(text or ""):gsub("[^\128-\191]", "")
  return count
end

local function trim(s)
  return (s:gsub("^%s+", ""):gsub("%s+$", ""))
end

local function splitRow(line)
  line = trim(line):gsub("^|", ""):gsub("|$", "")
  local cells = {}
  -- `\|` stays inside a cell.
  line = line:gsub("\\|", "\005")
  for cell in (line .. "|"):gmatch("(.-)|") do
    insert(cells, trim((cell:gsub("\005", "|"))))
  end
  return cells
end

local function isTableSeparator(line)
  if not line or not line:find("-", 1, true) then
    return false
  end
  local stripped = trim(line):gsub("^|", ""):gsub("|$", "")
  for cell in (stripped .. "|"):gmatch("(.-)|") do
    if not trim(cell):match("^:?%-+:?$") then
      return false
    end
  end
  return true
end

local function alignment(cell)
  cell = trim(cell)
  local left, right = cell:sub(1, 1) == ":", cell:sub(-1) == ":"
  if left and right then
    return "center"
  elseif right then
    return "right"
  end
  return "left"
end

local function listItem(line)
  local indent, marker, rest = line:match("^(%s*)([%-%*%+])%s+(.*)$")
  if not indent then
    indent, marker, rest = line:match("^(%s*)(%d+[%.%)])%s+(.*)$")
  end
  if not indent then
    return nil
  end
  indent = indent:gsub("\t", "    ")
  local level = math.min(4, math.floor(#indent / 2))
  if marker:match("^[%-%*%+]$") then
    marker = "•"
    local box, after = rest:match("^%[([ xX])%]%s+(.*)$")
    if box then
      marker = box == " " and "[ ]" or "[x]"
      rest = after
    end
  end
  return level, marker, rest
end

local function isRule(line)
  local compact = line:gsub("%s", "")
  return #compact >= 3 and (compact:match("^%-+$") or compact:match("^%*+$") or compact:match("^_+$")) ~= nil
end

local function startsBlock(line, nextLine)
  return line:match("^%s*```") or line:match("^%s*~~~") or line:match("^%s*#+%s")
    or line:match("^%s*>") or listItem(line) or isRule(line)
    or (line:find("|", 1, true) and isTableSeparator(nextLine))
end

--- Parse Markdown into display blocks.
function M.Parse(source)
  local lines = {}
  for line in (tostring(source or ""):gsub("\r\n", "\n") .. "\n"):gmatch("(.-)\n") do
    insert(lines, line)
  end
  local blocks = {}
  local i = 1
  while i <= #lines do
    local line = lines[i]
    local fence, lang = line:match("^%s*(```+)%s*([^`%s]*)")
    if not fence then
      fence, lang = line:match("^%s*(~~~+)%s*(%S*)")
    end
    if fence then
      -- Fenced code runs to the closing fence, or to the end while a message streams in.
      local code = {}
      i = i + 1
      while i <= #lines and not lines[i]:match("^%s*" .. fence:gsub("%p", "%%%0")) do
        insert(code, (lines[i]:gsub("|", "||")))
        i = i + 1
      end
      insert(blocks, { kind = "code", lang = lang ~= "" and lang or nil, text = concat(code, "\n") })
      i = i + 1
    elseif line:match("^%s*$") then
      i = i + 1
    elseif line:match("^%s*#+%s") or line:match("^%s*#+$") then
      local hashes, text = line:match("^%s*(#+)%s*(.-)%s*#*%s*$")
      insert(blocks, { kind = "heading", level = math.min(6, #hashes), text = M.Inline(text) })
      i = i + 1
    elseif isRule(line) then
      insert(blocks, { kind = "rule" })
      i = i + 1
    elseif line:find("|", 1, true) and isTableSeparator(lines[i + 1]) then
      local header = splitRow(line)
      local align = {}
      for _, cell in ipairs(splitRow(lines[i + 1])) do
        insert(align, alignment(cell))
      end
      local rows = { header }
      i = i + 2
      while i <= #lines and lines[i]:find("|", 1, true) and not lines[i]:match("^%s*$") do
        insert(rows, splitRow(lines[i]))
        i = i + 1
      end
      insert(blocks, { kind = "table", rows = rows, align = align })
    elseif line:match("^%s*>") then
      local quoted = {}
      while i <= #lines and lines[i]:match("^%s*>") do
        insert(quoted, M.Inline((lines[i]:gsub("^%s*>%s?", ""))))
        i = i + 1
      end
      insert(blocks, { kind = "quote", text = concat(quoted, "\n") })
    elseif listItem(line) then
      local level, marker, text = listItem(line)
      local parts = { text }
      i = i + 1
      -- Indented lines that start nothing new continue the item.
      while i <= #lines and lines[i]:match("^%s+%S") and not startsBlock(lines[i], lines[i + 1]) do
        insert(parts, trim(lines[i]))
        i = i + 1
      end
      insert(blocks, { kind = "item", level = level, marker = marker, text = M.Inline(concat(parts, " ")) })
    else
      -- A paragraph keeps its line breaks, which agents use like a chat would.
      local parts = {}
      while i <= #lines and not lines[i]:match("^%s*$") and (#parts == 0 or not startsBlock(lines[i], lines[i + 1])) do
        insert(parts, M.Inline(trim(lines[i])))
        i = i + 1
      end
      insert(blocks, { kind = "paragraph", text = concat(parts, "\n") })
    end
  end
  return blocks
end

local function cut(text, width)
  if M.Width(text) <= width then
    return text
  end
  -- Cut on a character boundary, leaving room for the ellipsis.
  local count = 0
  for pos in text:gmatch("()[^\128-\191]") do
    count = count + 1
    if count == width then
      return text:sub(1, pos - 1) .. "…"
    end
  end
  return text
end

--- Lay out a table as monospace lines: column widths from the plain text,
--- cells padded to them, a rule under the header. When the table is wider than
--- `maxChars`, the widest columns shrink first and their cells are cut.
function M.TableLines(block, maxChars)
  maxChars = maxChars or 100
  local widths, plain = {}, {}
  for r, row in ipairs(block.rows) do
    plain[r] = {}
    for c, cell in ipairs(row) do
      plain[r][c] = M.Plain(cell)
      widths[c] = math.max(widths[c] or 3, M.Width(plain[r][c]))
    end
  end
  local gap = 2
  local function total()
    local sum = gap * math.max(0, #widths - 1)
    for _, w in ipairs(widths) do
      sum = sum + w
    end
    return sum
  end
  while total() > maxChars do
    local widest = 1
    for c, w in ipairs(widths) do
      if w > widths[widest] then
        widest = c
      end
    end
    if widths[widest] <= 4 then
      break
    end
    widths[widest] = widths[widest] - 1
  end
  local lines = {}
  for r, row in ipairs(plain) do
    local cells = {}
    for c = 1, #widths do
      local text = cut(row[c] or "", widths[c])
      local pad = widths[c] - M.Width(text)
      local align = block.align and block.align[c] or "left"
      if align == "right" then
        text = string.rep(" ", pad) .. text
      elseif align == "center" then
        local left = math.floor(pad / 2)
        text = string.rep(" ", left) .. text .. string.rep(" ", pad - left)
      else
        text = text .. string.rep(" ", pad)
      end
      cells[c] = text:gsub("|", "||")
    end
    insert(lines, { text = (concat(cells, string.rep(" ", gap)):gsub("%s+$", "")), header = r == 1 })
    if r == 1 then
      local rule = {}
      for c = 1, #widths do
        rule[c] = string.rep("─", widths[c])
      end
      insert(lines, { text = concat(rule, string.rep(" ", gap)), rule = true })
    end
  end
  return lines
end

return M
