-- The conversation pane: renders thread entries (see U.ThreadEntries) as
-- stacked font strings in a scroll frame. Markdown blocks get their own fonts:
-- headings in the game's title font, code and tables in a bundled monospace
-- font. Each message has a Copy button, and links open a copy box too, since
-- rendered text cannot be selected. Question options are clickable rows.
local _, ns = ...
local M = ns.Markdown

local View = {}
View.__index = View
ns.MessageView = View

local PAD = 8
local HEADER_GAP = 12
local BLOCK_GAP = 6
local LIST_INDENT = 16
local CODE_PAD = 6

local TEXT_COLOR = { 0.86, 0.86, 0.86 }
local QUOTE_COLOR = { 0.7, 0.7, 0.72 }
local CODE_COLOR = { 0.8, 0.88, 1 }
local TABLE_HEADER_COLOR = { 1, 1, 1 }
local DIM_COLOR = { 0.45, 0.45, 0.5 }

--- `fonts`: { body, small, mono, h1, h2, h3 } font objects.
--- `onCopy(title, text)` opens the copy box.
--- `onAction(action)` runs when an option row or action button is clicked.
function View.Create(parent, name, fonts, onCopy, onAction)
  local scroll = CreateFrame("ScrollFrame", name, parent, "UIPanelScrollFrameTemplate")
  local content = CreateFrame("Frame", name .. "Content", scroll)
  content:SetSize(300, 10)
  scroll:SetScrollChild(content)
  local view = setmetatable({
    scroll = scroll,
    content = content,
    fonts = fonts,
    onCopy = onCopy,
    onAction = onAction,
    pools = {},
    entries = {},
    width = 300,
    plain = {},
  }, View)
  scroll:HookScript("OnSizeChanged", function(_, width)
    view:Resize(width)
  end)
  if content.SetHyperlinksEnabled then
    content:SetHyperlinksEnabled(true)
    content:SetScript("OnHyperlinkClick", function(_, link)
      local url = link and link:match("^t3url:(.*)$")
      if url then
        view.onCopy("Link", url)
      end
    end)
  end
  return view
end

---------------------------------------------------------------------------
-- Widget pools: everything is hidden before a layout and reused in order.
---------------------------------------------------------------------------

function View:Acquire(kind, create)
  local pool = self.pools[kind]
  if not pool then
    pool = { items = {}, used = 0 }
    self.pools[kind] = pool
  end
  pool.used = pool.used + 1
  local item = pool.items[pool.used]
  if not item then
    item = create()
    pool.items[pool.used] = item
  end
  item:ClearAllPoints()
  item:Show()
  return item
end

function View:ReleaseAll()
  for _, pool in pairs(self.pools) do
    for _, item in ipairs(pool.items) do
      item:Hide()
    end
    pool.used = 0
  end
  self.plain = {}
end

function View:Text(font, x, y, width, text, rgb, wrap)
  local fs = self:Acquire("text", function()
    local s = self.content:CreateFontString(nil, "OVERLAY")
    s:SetJustifyH("LEFT")
    s:SetJustifyV("TOP")
    return s
  end)
  fs:SetFontObject(font)
  fs:SetWordWrap(wrap ~= false)
  if fs.SetNonSpaceWrap then
    fs:SetNonSpaceWrap(wrap ~= false)
  end
  fs:SetPoint("TOPLEFT", self.content, "TOPLEFT", x, -y)
  fs:SetWidth(math.max(20, width))
  fs:SetTextColor((rgb or TEXT_COLOR)[1], (rgb or TEXT_COLOR)[2], (rgb or TEXT_COLOR)[3])
  fs:SetText(text)
  self.plain[#self.plain + 1] = text
  -- An empty string still takes a line.
  return math.max(fs:GetStringHeight() or 0, 1)
end

function View:Box(x, y, width, height, r, g, b, a)
  local tex = self:Acquire("texture", function()
    return self.content:CreateTexture(nil, "BACKGROUND")
  end)
  tex:SetPoint("TOPLEFT", self.content, "TOPLEFT", x, -y)
  tex:SetSize(math.max(1, width), math.max(1, height))
  tex:SetColorTexture(r, g, b, a)
  return tex
end

function View:CopyButton(y, title, text)
  local button = self:Acquire("copy", function()
    local b = CreateFrame("Button", nil, self.content)
    b:SetSize(36, 14)
    local label = b:CreateFontString(nil, "OVERLAY", "GameFontDisableSmall")
    label:SetAllPoints()
    label:SetText("Copy")
    b:SetFontString(label)
    b:SetHighlightFontObject("GameFontHighlightSmall")
    b:SetScript("OnClick", function(self)
      self.view.onCopy(self.title, self.copyText)
    end)
    return b
  end)
  button.view, button.title, button.copyText = self, title, text
  button:SetPoint("TOPRIGHT", self.content, "TOPRIGHT", -PAD, -y)
end

--- A clickable answer option; multi-select options get a check box.
function View:Option(y, width, entry)
  local row = self:Acquire("option", function()
    local b = CreateFrame("Button", nil, self.content)
    b.bg = b:CreateTexture(nil, "BACKGROUND")
    b.bg:SetAllPoints()
    b.bg:SetColorTexture(0.16, 0.17, 0.22, 0.9)
    local highlight = b:CreateTexture(nil, "HIGHLIGHT")
    highlight:SetAllPoints()
    highlight:SetColorTexture(0.35, 0.45, 0.75, 0.35)
    b.box = b:CreateTexture(nil, "ARTWORK")
    b.box:SetSize(20, 20)
    b.box:SetPoint("TOPLEFT", 4, -3)
    b.box:SetTexture("Interface\\Buttons\\UI-CheckBox-Up")
    b.mark = b:CreateTexture(nil, "OVERLAY")
    b.mark:SetAllPoints(b.box)
    b.mark:SetTexture("Interface\\Buttons\\UI-CheckBox-Check")
    b.label = b:CreateFontString(nil, "OVERLAY")
    b.label:SetJustifyH("LEFT")
    b.label:SetJustifyV("TOP")
    b.label:SetWordWrap(true)
    b:SetScript("OnClick", function(button)
      button.view.onAction(button.action)
    end)
    return b
  end)
  row.view, row.action = self, entry.action
  local left = entry.picked == nil and 8 or 26
  row.box:SetShown(entry.picked ~= nil)
  row.mark:SetShown(entry.picked == true)
  row.label:SetFontObject(self.fonts.body)
  row.label:SetTextColor(TEXT_COLOR[1], TEXT_COLOR[2], TEXT_COLOR[3])
  row.label:ClearAllPoints()
  row.label:SetPoint("TOPLEFT", left, -5)
  row.label:SetWidth(math.max(20, width - left - 8))
  row.label:SetText(entry.text)
  self.plain[#self.plain + 1] = (entry.picked and "[x] " or entry.picked == false and "[ ] " or "") .. entry.text
  local height = math.max(row.label:GetStringHeight() or 0, 14) + 10
  row:SetPoint("TOPLEFT", self.content, "TOPLEFT", PAD, -y)
  row:SetSize(width, height)
  return height
end

--- A button that runs an action, e.g. sending multi-select choices.
function View:ActionButton(y, entry)
  local button = self:Acquire("action", function()
    local b = CreateFrame("Button", nil, self.content, "UIPanelButtonTemplate")
    b:SetScript("OnClick", function(widget)
      widget.view.onAction(widget.action)
    end)
    return b
  end)
  button.view, button.action = self, entry.action
  button:SetText(entry.text)
  button:SetSize(130, 22)
  button:SetEnabled(entry.enabled ~= false)
  button:SetPoint("TOPLEFT", self.content, "TOPLEFT", PAD, -y)
  self.plain[#self.plain + 1] = "[" .. entry.text .. "]"
  return 22
end

---------------------------------------------------------------------------
-- Layout
---------------------------------------------------------------------------

function View:Block(block, y, width)
  local fonts = self.fonts
  if block.kind == "heading" then
    local font = fonts["h" .. math.min(block.level, 3)]
    return y + self:Text(font, PAD, y + 2, width, block.text, nil) + 4
  elseif block.kind == "paragraph" then
    return y + self:Text(fonts.body, PAD, y, width, block.text)
  elseif block.kind == "item" then
    local indent = block.level * LIST_INDENT
    local markerWidth = block.marker:match("^%d") and 22 or 14
    self:Text(fonts.body, PAD + indent, y, markerWidth, block.marker, DIM_COLOR, false)
    return y + self:Text(fonts.body, PAD + indent + markerWidth + 2, y, width - indent - markerWidth - 2, block.text)
  elseif block.kind == "quote" then
    local height = self:Text(fonts.body, PAD + 12, y, width - 12, block.text, QUOTE_COLOR)
    self:Box(PAD + 2, y, 3, height, 0.4, 0.4, 0.48, 1)
    return y + height
  elseif block.kind == "code" then
    local top = y
    y = y + CODE_PAD
    if block.lang then
      self:Text(fonts.small, PAD + CODE_PAD, y, width - CODE_PAD * 2, block.lang, DIM_COLOR)
      y = y + 14
    end
    local height = self:Text(fonts.mono, PAD + CODE_PAD, y, width - CODE_PAD * 2, block.text, CODE_COLOR)
    y = y + height + CODE_PAD
    self:Box(PAD, top, width, y - top, 0.12, 0.13, 0.17, 0.95)
    return y
  elseif block.kind == "table" then
    local top = y
    y = y + 4
    for _, line in ipairs(M.TableLines(block, self:MonoColumns(width - CODE_PAD * 2))) do
      local rgb = line.header and TABLE_HEADER_COLOR or line.rule and DIM_COLOR or TEXT_COLOR
      y = y + self:Text(fonts.mono, PAD + CODE_PAD, y, width - CODE_PAD * 2, line.text, rgb, false) + 1
    end
    y = y + 3
    self:Box(PAD, top, width, y - top, 0.1, 0.1, 0.13, 0.7)
    return y
  elseif block.kind == "rule" then
    self:Box(PAD, y + 4, width, 1, 0.3, 0.3, 0.36, 1)
    return y + 9
  end
  return y
end

--- How many monospace characters fit in `width` pixels.
function View:MonoColumns(width)
  local probe = self.probe
  if not probe then
    probe = self.content:CreateFontString(nil, "OVERLAY")
    probe:Hide()
    self.probe = probe
  end
  probe:SetFontObject(self.fonts.mono)
  probe:SetText(string.rep("0", 20))
  local charWidth = (probe:GetStringWidth() or 0) / 20
  if charWidth <= 0 then
    return 100
  end
  return math.max(20, math.floor(width / charWidth))
end

function View:Layout()
  self:ReleaseAll()
  local width = self.width - PAD * 2
  local y = PAD
  for index, entry in ipairs(self.entries) do
    if entry.kind == "header" then
      if index > 1 then
        y = y + HEADER_GAP
      end
      if entry.copy and entry.copy ~= "" then
        self:CopyButton(y, "Message", entry.copy)
      end
      y = y + self:Text(self.fonts.small, PAD, y, width - 44, entry.text) + 4
    elseif entry.kind == "markdown" then
      for _, block in ipairs(M.Parse(entry.text)) do
        y = self:Block(block, y, width) + BLOCK_GAP
      end
    elseif entry.kind == "note" then
      if index > 1 and self.entries[index - 1].kind ~= "header" then
        y = y + 4
      end
      y = y + self:Text(self.fonts.body, PAD, y, width, entry.text) + BLOCK_GAP
    elseif entry.kind == "option" then
      y = y + self:Option(y, width, entry) + 3
    elseif entry.kind == "button" then
      y = y + 3 + self:ActionButton(y + 3, entry) + BLOCK_GAP
    end
  end
  self.content:SetSize(self.width, y + PAD)
end

function View:IsAtBottom()
  local scroll = self.scroll
  return scroll:GetVerticalScroll() >= scroll:GetVerticalScrollRange() - 24
end

function View:ScrollToBottom()
  local scroll = self.scroll
  C_Timer.After(0.05, function()
    scroll:UpdateScrollChildRect()
    scroll:SetVerticalScroll(scroll:GetVerticalScrollRange())
  end)
end

--- Show entries. With `keepPosition`, stay where the reader is unless they
--- were following the end of the conversation.
function View:SetEntries(entries, keepPosition)
  local follow = not keepPosition or self:IsAtBottom()
  self.entries = entries
  self:Layout()
  if follow then
    self:ScrollToBottom()
  end
end

function View:Resize(width)
  width = math.max(100, width or 0)
  if math.abs(width - self.width) < 1 then
    return
  end
  self.width = width
  -- Coalesce the burst of size changes while the window is dragged.
  if not self.resizePending then
    self.resizePending = true
    C_Timer.After(0, function()
      self.resizePending = false
      local follow = self:IsAtBottom()
      self:Layout()
      if follow then
        self:ScrollToBottom()
      end
    end)
  end
end

--- Everything currently shown, one string per text widget (for tests and debugging).
function View:PlainText()
  return table.concat(self.plain, "\n")
end
