-- The T3 window: projects and threads on the left, the conversation and
-- composer on the right, bridge status in the footer.
local _, ns = ...
local T3, U = ns.T3, ns.Util

local BACKDROP = {
  bgFile = "Interface\\Buttons\\WHITE8X8",
  edgeFile = "Interface\\Buttons\\WHITE8X8",
  edgeSize = 1,
}
local BACKDROP_TEMPLATE = BackdropTemplateMixin and "BackdropTemplate" or nil
local SIDEBAR_WIDTH = 250
local ROW_HEIGHT = 36
local HEADER_HEIGHT = 34
local FOOTER_HEIGHT = 24
local COMPOSER_HEIGHT = 112
local MENU_ROW_HEIGHT = 18
local MENU_MAX_ROWS = 20
local MENU_WIDTH = 230
local MONO_FONT = "Interface\\AddOns\\T3Wow\\Fonts\\JetBrainsMono-Regular.ttf"
local TITLE_FONT = "Fonts\\FRIZQT__.TTF"
local CHECK = "|TInterface\\Buttons\\UI-CheckBox-Check:14:14|t"
local SEND_MODES = { "auto", "queue", "steer" }
local SEND_MODE_LABELS = { auto = "Auto", queue = "Queue", steer = "Steer" }

local function hexToRGB(hex)
  return tonumber(hex:sub(1, 2), 16) / 255, tonumber(hex:sub(3, 4), 16) / 255, tonumber(hex:sub(5, 6), 16) / 255
end

local function Panel(parent, alpha)
  local frame = CreateFrame("Frame", nil, parent, BACKDROP_TEMPLATE)
  frame:SetBackdrop(BACKDROP)
  frame:SetBackdropColor(0.08, 0.08, 0.1, alpha or 0.9)
  frame:SetBackdropBorderColor(0.22, 0.22, 0.27, 1)
  return frame
end

local function Button(parent, text, width)
  local button = CreateFrame("Button", nil, parent, "UIPanelButtonTemplate")
  button:SetText(text)
  button:SetSize(width or 70, 22)
  return button
end

local function Tooltip(widget, title, body)
  widget:HookScript("OnEnter", function(self)
    GameTooltip:SetOwner(self, "ANCHOR_TOP")
    GameTooltip:SetText(title)
    if body then
      GameTooltip:AddLine(body, 1, 1, 1, true)
    end
    GameTooltip:Show()
  end)
  widget:HookScript("OnLeave", GameTooltip_Hide)
end

-- A multi-line edit box inside a scroll frame. Scroll frame names are required
-- by the scroll bar template on older clients.
local function ScrollingEditBox(parent, name, font)
  local scroll = CreateFrame("ScrollFrame", name, parent, "UIPanelScrollFrameTemplate")
  local edit = CreateFrame("EditBox", name .. "Edit", scroll)
  edit:SetMultiLine(true)
  edit:SetAutoFocus(false)
  edit:SetFontObject(font)
  -- A scroll child needs a real size: with no height an empty edit box has no
  -- valid rect, cannot take keyboard focus, and keys fall through to movement.
  edit:SetSize(300, 1)
  edit:SetTextInsets(6, 6, 4, 4)
  edit:SetScript("OnEscapePressed", edit.ClearFocus)
  scroll:SetScrollChild(edit)
  scroll:HookScript("OnSizeChanged", function(_, width)
    edit:SetWidth(math.max(50, width))
  end)
  -- Clicking anywhere in the scroll area focuses the text.
  scroll:EnableMouse(true)
  scroll:SetScript("OnMouseDown", function()
    edit:SetFocus()
  end)
  return scroll, edit
end

---------------------------------------------------------------------------
-- Construction
---------------------------------------------------------------------------

function T3:CreateUI()
  local db = T3WowDB
  local function font(name, template)
    local object = CreateFont(name)
    object:CopyFontObject(template)
    return object
  end
  self.fonts = {
    body = font("T3WowFont", ChatFontNormal),
    small = font("T3WowFontSmall", ChatFontNormal),
    mono = font("T3WowFontMono", ChatFontNormal),
    h1 = font("T3WowFontH1", ChatFontNormal),
    h2 = font("T3WowFontH2", ChatFontNormal),
    h3 = font("T3WowFontH3", ChatFontNormal),
  }
  self.font = self.fonts.body
  self:ApplyFont()

  local f = CreateFrame("Frame", "T3WowFrame", UIParent, BACKDROP_TEMPLATE)
  self.frame = f
  f:SetSize(db.ui.width or 920, db.ui.height or 580)
  if db.ui.point then
    f:SetPoint(db.ui.point, UIParent, db.ui.point, db.ui.x or 0, db.ui.y or 0)
  else
    f:SetPoint("CENTER")
  end
  f:SetFrameStrata("HIGH")
  f:SetToplevel(true)
  f:SetClampedToScreen(true)
  f:SetMovable(true)
  f:SetResizable(true)
  f:EnableMouse(true)
  if f.SetResizeBounds then
    f:SetResizeBounds(660, 400)
  else
    f:SetMinResize(660, 400)
  end
  f:SetBackdrop(BACKDROP)
  f:SetBackdropColor(0.04, 0.04, 0.05, 0.96)
  f:SetBackdropBorderColor(0.3, 0.3, 0.36, 1)
  f:Hide()
  table.insert(UISpecialFrames, "T3WowFrame")
  f:SetScript("OnShow", function()
    db.ui.open = true
    self:Refresh()
  end)
  f:SetScript("OnHide", function()
    db.ui.open = false
    if self.menu then
      self.menu:Hide()
    end
  end)

  self:CreateHeader(f)
  self:CreateSidebar(f)
  self:CreateMain(f)
  self:CreateFooter(f)

  self:On("DATA", function()
    self:Refresh()
  end)
  self:On("SELECT", function()
    self:Refresh()
  end)
  self:On("STATUS", function()
    self:RenderStatus()
  end)
  self:On("FONT", function()
    self:ApplyFont()
  end)
end

function T3:ApplyFont()
  local size = T3WowDB.settings.fontSize
  local path, _, flags = ChatFontNormal:GetFont()
  local fonts = self.fonts
  fonts.body:SetFont(path, size, flags)
  fonts.small:SetFont(path, math.max(9, size - 2), flags)
  -- The bundled monospace font runs wide, so one size smaller.
  fonts.mono:SetFont(MONO_FONT, math.max(9, size - 1), "")
  if not fonts.mono:GetFont() then
    fonts.mono:SetFont(path, size, flags)
  end
  for level, extra in ipairs({ 5, 3, 1 }) do
    local heading = fonts["h" .. level]
    heading:SetFont(TITLE_FONT, size + extra, "")
    heading:SetTextColor(1, 0.82, 0)
  end
  if self.messageView then
    self.messageView:Layout()
  end
end

function T3:CreateHeader(f)
  local header = CreateFrame("Frame", nil, f)
  header:SetPoint("TOPLEFT", 1, -1)
  header:SetPoint("TOPRIGHT", -1, -1)
  header:SetHeight(HEADER_HEIGHT)
  header:EnableMouse(true)
  header:RegisterForDrag("LeftButton")
  header:SetScript("OnDragStart", function()
    f:StartMoving()
  end)
  header:SetScript("OnDragStop", function()
    f:StopMovingOrSizing()
    local point, _, _, x, y = f:GetPoint()
    T3WowDB.ui.point, T3WowDB.ui.x, T3WowDB.ui.y = point, x, y
  end)
  local bg = header:CreateTexture(nil, "BACKGROUND")
  bg:SetAllPoints()
  bg:SetColorTexture(0.1, 0.1, 0.13, 1)

  local title = header:CreateFontString(nil, "OVERLAY", "GameFontNormalLarge")
  title:SetPoint("LEFT", 12, 0)
  title:SetText("T3 Code")

  local dot = header:CreateTexture(nil, "OVERLAY")
  dot:SetSize(9, 9)
  dot:SetPoint("LEFT", title, "RIGHT", 10, 0)
  dot:SetColorTexture(0.5, 0.5, 0.5, 1)
  self.statusDot = dot

  local host = header:CreateFontString(nil, "OVERLAY", "GameFontDisableSmall")
  host:SetPoint("LEFT", dot, "RIGHT", 6, 0)
  self.hostText = host

  local close = CreateFrame("Button", nil, header, "UIPanelCloseButton")
  close:SetPoint("RIGHT", 2, 0)
  close:SetScript("OnClick", function()
    f:Hide()
  end)

  local sync = Button(header, "Sync", 64)
  sync:SetPoint("RIGHT", close, "LEFT", -4, 0)
  sync:SetScript("OnClick", function()
    self:Sync(IsShiftKeyDown())
  end)
  Tooltip(sync, "Sync", "Loads the latest data. Reloads the UI only when it has to: to send queued messages, "
    .. "to load the whole open thread when only a preview is synced, or when live updates are used up.\n"
    .. "Shift-click to reload anyway. Updates also arrive on their own while this window is open.")
  self.syncButton = sync
end

function T3:CreateSidebar(f)
  local side = Panel(f, 0.6)
  side:SetPoint("TOPLEFT", 6, -HEADER_HEIGHT - 4)
  side:SetPoint("BOTTOMLEFT", 6, FOOTER_HEIGHT + 2)
  side:SetWidth(SIDEBAR_WIDTH)

  local back = Button(side, "<", 26)
  back:SetPoint("TOPLEFT", 6, -6)
  back:SetScript("OnClick", function()
    self.composeNew = false
    self:Select(nil, T3WowDB.focus.threadId)
  end)
  Tooltip(back, "All projects")
  self.backButton = back

  local new = Button(side, "+ New", 58)
  new:SetPoint("TOPRIGHT", -6, -6)
  new:SetScript("OnClick", function()
    self:StartNew(T3WowDB.focus.projectId)
  end)
  Tooltip(new, "New thread", "Start a new thread. Pick its project next to the Start button. "
    .. "The first line of your message becomes its title.")
  self.newButton = new

  local nav = side:CreateFontString(nil, "OVERLAY", "GameFontNormal")
  nav:SetPoint("LEFT", back, "RIGHT", 6, 0)
  nav:SetPoint("RIGHT", new, "LEFT", -6, 0)
  nav:SetJustifyH("LEFT")
  nav:SetWordWrap(false)
  self.navTitle = nav

  local scroll = CreateFrame("ScrollFrame", "T3WowList", side, "UIPanelScrollFrameTemplate")
  scroll:SetPoint("TOPLEFT", 4, -34)
  scroll:SetPoint("BOTTOMRIGHT", -26, 4)
  local content = CreateFrame("Frame", nil, scroll)
  content:SetSize(SIDEBAR_WIDTH - 30, 10)
  scroll:SetScrollChild(content)
  self.listContent = content
  self.rows = {}
end

function T3:Row(index)
  local row = self.rows[index]
  if row then
    return row
  end
  row = CreateFrame("Button", nil, self.listContent)
  row:SetHeight(ROW_HEIGHT)
  row:SetPoint("TOPLEFT", 0, -(index - 1) * ROW_HEIGHT)
  row:SetPoint("RIGHT", self.listContent, "RIGHT")
  row:SetHighlightTexture("Interface\\Buttons\\WHITE8X8")
  row:GetHighlightTexture():SetVertexColor(1, 1, 1, 0.06)

  row.selected = row:CreateTexture(nil, "BACKGROUND")
  row.selected:SetAllPoints()
  row.selected:SetColorTexture(0.35, 0.55, 1, 0.16)

  row.dot = row:CreateTexture(nil, "ARTWORK")
  row.dot:SetSize(7, 7)
  row.dot:SetPoint("TOPLEFT", 6, -10)

  row.title = row:CreateFontString(nil, "OVERLAY", "GameFontHighlight")
  row.title:SetPoint("TOPLEFT", 20, -5)
  row.title:SetPoint("RIGHT", -4, 0)
  row.title:SetJustifyH("LEFT")
  row.title:SetWordWrap(false)

  row.sub = row:CreateFontString(nil, "OVERLAY", "GameFontDisableSmall")
  row.sub:SetPoint("TOPLEFT", row.title, "BOTTOMLEFT", 0, -3)
  row.sub:SetPoint("RIGHT", -4, 0)
  row.sub:SetJustifyH("LEFT")
  row.sub:SetWordWrap(false)

  row:SetScript("OnClick", function(button)
    if button.onClick then
      button.onClick()
    end
  end)
  self.rows[index] = row
  return row
end

function T3:CreateMain(f)
  local main = CreateFrame("Frame", nil, f)
  main:SetPoint("TOPLEFT", SIDEBAR_WIDTH + 14, -HEADER_HEIGHT - 4)
  main:SetPoint("BOTTOMRIGHT", -8, FOOTER_HEIGHT + 2)

  local title = main:CreateFontString(nil, "OVERLAY", "GameFontHighlightLarge")
  title:SetPoint("TOPLEFT", 2, -4)
  title:SetPoint("RIGHT", -150, 0)
  title:SetJustifyH("LEFT")
  title:SetWordWrap(false)
  self.threadTitle = title

  local meta = main:CreateFontString(nil, "OVERLAY", "GameFontDisableSmall")
  meta:SetPoint("TOPLEFT", title, "BOTTOMLEFT", 0, -4)
  meta:SetPoint("RIGHT", -4, 0)
  meta:SetJustifyH("LEFT")
  meta:SetWordWrap(false)
  self.threadMeta = meta

  local stop = Button(main, "Stop", 60)
  stop:SetPoint("TOPRIGHT", 0, -2)
  stop:SetScript("OnClick", function()
    if T3WowDB.focus.threadId then
      self:Interrupt(T3WowDB.focus.threadId)
    end
  end)
  Tooltip(stop, "Stop", "Interrupt the running turn.")
  self.stopButton = stop

  local compact = Button(main, "Compact", 76)
  compact:SetScript("OnClick", function()
    if T3WowDB.focus.threadId then
      self:Compact(T3WowDB.focus.threadId)
    end
  end)
  Tooltip(compact, "Compact", "Summarize the conversation to free up the model's context, like /compact in T3 Code. "
    .. "Runs after the current turn if the agent is working.")
  self.compactButton = compact

  local box = Panel(main, 0.5)
  box:SetPoint("TOPLEFT", 0, -42)
  box:SetPoint("BOTTOMRIGHT", 0, COMPOSER_HEIGHT + 6)
  local view = ns.MessageView.Create(box, "T3WowMessages", self.fonts, function(title, text)
    self:ShowCopy(title, text)
  end, function(action)
    self:QuestionAction(action)
  end)
  view.scroll:SetPoint("TOPLEFT", 4, -4)
  view.scroll:SetPoint("BOTTOMRIGHT", -26, 4)
  self.messageView = view

  self:CreateComposer(main)
end

function T3:CreateComposer(main)
  local box = Panel(main, 0.8)
  box:SetPoint("BOTTOMLEFT", 0, 0)
  box:SetPoint("BOTTOMRIGHT", 0, 0)
  box:SetHeight(COMPOSER_HEIGHT)

  local scroll, edit = ScrollingEditBox(box, "T3WowComposer", self.font)
  scroll:SetPoint("TOPLEFT", 4, -4)
  scroll:SetPoint("BOTTOMRIGHT", -26, 32)
  self.composer = edit

  local placeholder = box:CreateFontString(nil, "OVERLAY", "GameFontDisable")
  placeholder:SetPoint("TOPLEFT", 12, -9)
  self.placeholder = placeholder

  edit:SetScript("OnTextChanged", function(widget)
    local text = widget:GetText()
    placeholder:SetShown(text == "")
    local key = self:DraftKey()
    if key then
      T3WowDB.drafts = T3WowDB.drafts or {}
      T3WowDB.drafts[key] = text ~= "" and text or nil
    end
  end)
  edit:SetScript("OnEnterPressed", function(widget)
    if IsControlKeyDown() then
      self:Submit()
      return
    end
    -- Some clients insert the newline themselves; only add one if they did not.
    local before, cursor = widget:GetText(), widget:GetCursorPosition()
    C_Timer.After(0, function()
      if widget:GetText() == before then
        widget:SetCursorPosition(cursor)
        widget:Insert("\n")
      end
    end)
  end)

  local send = Button(box, "Send", 80)
  send:SetPoint("BOTTOMRIGHT", -6, 6)
  send:SetScript("OnClick", function()
    self:Submit()
  end)
  self.sendButton = send

  local answer = Button(box, "Answer", 80)
  answer:SetPoint("RIGHT", send, "LEFT", -6, 0)
  answer:SetScript("OnClick", function()
    self:Submit(true)
  end)
  Tooltip(answer, "Answer", "Send what you typed as the answer to the agent's question. An option number picks that option.")
  self.answerButton = answer

  self.sendMode = "auto"
  local mode = Button(box, "Auto", 64)
  mode:SetPoint("BOTTOMLEFT", 6, 6)
  mode:SetScript("OnClick", function(button)
    local nextIndex = 1
    for i, value in ipairs(SEND_MODES) do
      if value == self.sendMode then
        nextIndex = i % #SEND_MODES + 1
      end
    end
    self.sendMode = SEND_MODES[nextIndex]
    button:SetText(SEND_MODE_LABELS[self.sendMode])
  end)
  Tooltip(mode, "While the agent is working",
    "Auto: steer the running turn when possible, otherwise queue.\nQueue: send after the current turn.\nSteer: add to the running turn.")
  self.modeButton = mode

  local project = Button(box, "Project", 140)
  project:SetPoint("BOTTOMLEFT", 6, 6)
  project:SetScript("OnClick", function(button)
    self:ShowProjectMenu(button)
  end)
  Tooltip(project, "Project", "Where the new thread runs.")
  self.projectButton = project

  local model = Button(box, "Model", 150)
  model:SetScript("OnClick", function(button)
    self:ShowModelMenu(button)
  end)
  Tooltip(model, "Model", "The model for this thread from your next message on, or for the new thread.")
  self.modelButton = model

  local effort = Button(box, "Effort", 86)
  effort:SetPoint("LEFT", model, "RIGHT", 4, 0)
  effort:SetScript("OnClick", function(button)
    self:ShowEffortMenu(button)
  end)
  Tooltip(effort, "Reasoning", "How hard the model thinks.")
  self.effortButton = effort

  local hint = box:CreateFontString(nil, "OVERLAY", "GameFontDisableSmall")
  hint:SetPoint("RIGHT", answer, "LEFT", -8, 0)
  hint:SetJustifyH("LEFT")
  hint:SetWordWrap(false)
  hint:SetText("Ctrl+Enter to send. Sending reloads the UI.")
  self.composerHint = hint
end

function T3:CreateFooter(f)
  local status = f:CreateFontString(nil, "OVERLAY", "GameFontDisableSmall")
  status:SetPoint("BOTTOMLEFT", 10, 7)
  status:SetJustifyH("LEFT")
  self.statusText = status

  local outbox = f:CreateFontString(nil, "OVERLAY", "GameFontNormalSmall")
  outbox:SetPoint("BOTTOMRIGHT", -24, 7)
  self.outboxText = outbox

  local grip = CreateFrame("Button", nil, f)
  grip:SetSize(16, 16)
  grip:SetPoint("BOTTOMRIGHT", -2, 2)
  grip:SetNormalTexture("Interface\\ChatFrame\\UI-ChatIM-SizeGrabber-Up")
  grip:SetHighlightTexture("Interface\\ChatFrame\\UI-ChatIM-SizeGrabber-Highlight")
  grip:SetPushedTexture("Interface\\ChatFrame\\UI-ChatIM-SizeGrabber-Down")
  grip:SetScript("OnMouseDown", function()
    f:StartSizing("BOTTOMRIGHT")
  end)
  grip:SetScript("OnMouseUp", function()
    f:StopMovingOrSizing()
    T3WowDB.ui.width, T3WowDB.ui.height = f:GetSize()
    local point, _, _, x, y = f:GetPoint()
    T3WowDB.ui.point, T3WowDB.ui.x, T3WowDB.ui.y = point, x, y
  end)
end

---------------------------------------------------------------------------
-- Rendering
---------------------------------------------------------------------------

function T3:Toggle()
  if self.frame:IsShown() then
    self.frame:Hide()
  else
    self.frame:Show()
  end
end

function T3:DraftKey()
  if self.composeNew then
    return self.newProjectId and ("new:" .. self.newProjectId)
  end
  return T3WowDB.focus.threadId
end

-- Open the new-thread form for a project: the given one, else the most recent.
function T3:StartNew(projectId)
  local projects = self.snapshot and self.snapshot.projects or {}
  if not (projectId and self:Project(projectId)) then
    projectId = projects[1] and projects[1].id
  end
  if not projectId then
    return
  end
  self.composeNew = true
  self.newProjectId = projectId
  self:Refresh()
  self.composer:SetFocus()
end

-- Move the new thread to another project, keeping the draft and a picked model.
function T3:SetNewProject(projectId)
  local db = T3WowDB
  local from, to = self:DraftKey(), "new:" .. projectId
  if from and from ~= to then
    db.drafts = db.drafts or {}
    local text = self.composer:GetText()
    db.drafts[to] = text ~= "" and text or db.drafts[to]
    db.drafts[from] = nil
    db.models[to] = db.models[from] or db.models[to]
    db.models[from] = nil
  end
  self.newProjectId = projectId
  -- The composer already holds the text; skip reloading it from the draft.
  self.draftKey = to
  self:Refresh()
end

function T3:ShowProjectMenu(anchor)
  local items = { { title = "Project" } }
  for _, project in ipairs(self.snapshot and self.snapshot.projects or {}) do
    table.insert(items, {
      text = project.title,
      checked = project.id == self.newProjectId,
      onClick = function()
        self:SetNewProject(project.id)
      end,
    })
  end
  self:ShowMenu(anchor, items)
end

function T3:Refresh()
  if not self.frame or not self.frame:IsShown() then
    return
  end
  -- Leave the "new thread" form when its project disappears.
  if self.composeNew and not self:Project(self.newProjectId) then
    self.composeNew = false
  end
  self:RenderList()
  self:RenderThread()
  self:RenderStatus()
end

function T3:RenderList()
  local focus = T3WowDB.focus
  local project = focus.projectId and self:Project(focus.projectId)
  local items = {}
  local now = time()
  if project then
    self.navTitle:SetText(U.Escape(project.title))
    self.backButton:Show()
    self.newButton:SetShown(self:CanSend())
    for _, thread in ipairs(project.threads or {}) do
      local label, color = U.Status(thread.status)
      local sub = U.Color(color, label) .. "  " .. U.Ago(now, thread.updatedAt)
      if thread.model and thread.model ~= "" then
        sub = sub .. "  " .. U.Escape(thread.model)
      end
      if T3WowDB.unread[thread.id] then
        sub = U.Color("ffd100", "new") .. "  " .. sub
      end
      table.insert(items, {
        title = U.Escape(thread.title),
        sub = sub,
        color = color,
        selected = not self.composeNew and thread.id == focus.threadId,
        onClick = function()
          self.composeNew = false
          self:Select(project.id, thread.id)
        end,
      })
    end
  else
    self.navTitle:SetText("Projects")
    self.backButton:Hide()
    self.newButton:SetShown(self:CanSend() and #(self.snapshot and self.snapshot.projects or {}) > 0)
    for _, p in ipairs(self.snapshot and self.snapshot.projects or {}) do
      local running = 0
      for _, thread in ipairs(p.threads or {}) do
        if U.ACTIVE[thread.status] then
          running = running + 1
        end
      end
      local sub = #(p.threads or {}) .. " threads"
      local unread = 0
      for _, thread in ipairs(p.threads or {}) do
        if T3WowDB.unread[thread.id] then
          unread = unread + 1
        end
      end
      if unread > 0 then
        sub = sub .. U.Color("ffd100", "  " .. unread .. " new")
      end
      if running > 0 then
        sub = sub .. U.Color("33dd66", "  " .. running .. " running")
      end
      table.insert(items, {
        title = U.Escape(p.title),
        sub = sub,
        color = running > 0 and "33dd66" or "606070",
        onClick = function()
          if self.composeNew then
            self:SetNewProject(p.id)
          end
          self:Select(p.id, nil)
        end,
      })
    end
  end

  for index, item in ipairs(items) do
    local row = self:Row(index)
    row.title:SetText(item.title)
    row.sub:SetText(item.sub)
    row.dot:SetColorTexture(hexToRGB(item.color))
    row.selected:SetShown(item.selected and true or false)
    row.onClick = item.onClick
    row:Show()
  end
  for index = #items + 1, #self.rows do
    self.rows[index]:Hide()
  end
  self.listContent:SetHeight(math.max(10, #items * ROW_HEIGHT))
end

-- Show message view entries, skipping the layout when nothing changed.
function T3:SetEntries(entries, keepPosition)
  local parts = {}
  for i, entry in ipairs(entries) do
    parts[i] = entry.kind .. "\001" .. (entry.text or "") .. "\001" .. (entry.copy or "")
      .. "\001" .. tostring(entry.picked) .. tostring(entry.enabled)
  end
  local signature = table.concat(parts, "\002")
  if self.entriesSignature == signature then
    return
  end
  self.entriesSignature = signature
  self.messageView:SetEntries(entries, keepPosition)
end

function T3:SetMessages(text)
  self:SetEntries({ { kind = "note", text = text } })
end

-- What the message view shows, as text (for tests).
function T3:MessagesText()
  return self.messageView:PlainText()
end

function T3:RenderThread()
  local db = T3WowDB
  local focus = db.focus
  local canSend = self:CanSend()
  self.stopButton:Hide()
  self.compactButton:Hide()
  self.answerButton:Hide()
  self.sendButton:SetText("Send")
  self.question = nil

  local placeholderText = "Message the agent..."
  if self.composeNew then
    local project = self:Project(self.newProjectId)
    self.threadTitle:SetText("New thread")
    self.threadMeta:SetText(project and ("in " .. U.Escape(project.title)) or "")
    self.sendButton:SetText("Start")
    placeholderText = "Describe the task. The first line becomes the title."
    local failures = self:FailuresFor(nil)
    local text = U.Color("a0a0a0", "The thread starts in the project folder with the project and model below.")
    for _, failure in ipairs(failures) do
      text = text .. "\n\n" .. U.Color("ff5555", "Not started: " .. U.Escape(failure.message))
    end
    self:SetMessages(text)
  elseif focus.threadId then
    local summary = self:ThreadSummary(focus.threadId)
    local thread = self:Thread(focus.threadId)
    local status = (thread and thread.status) or (summary and summary.status) or "idle"
    local label, color = U.Status(status)
    self.threadTitle:SetText(U.Escape((thread and thread.title) or (summary and summary.title) or "Thread"))
    local meta = U.Color(color, label)
    local model = (thread and thread.model) or (summary and summary.model)
    if model then
      local known = summary and self:FindModel(summary.provider, model)
      meta = meta .. "   " .. U.Escape(known and known.label or model)
    end
    if thread and thread.branch then
      meta = meta .. "   " .. U.Color("a0a0a0", U.Escape(thread.branch))
    end
    self.threadMeta:SetText(meta)
    self.stopButton:SetShown(canSend and U.ACTIVE[status] or false)
    self.compactButton:ClearAllPoints()
    self.compactButton:SetPoint("TOPRIGHT", self.stopButton:IsShown() and -64 or 0, -2)
    self.compactButton:SetShown(canSend)
    if thread then
      self:PruneAnswers(thread)
      local pending = self:PendingFor(focus.threadId)
      local answering = { drafts = db.answers, picks = self.picks, interactive = canSend }
      self:SetEntries(U.ThreadEntries(thread, pending, self:FailuresFor(focus.threadId), answering), true)
      self.question = U.CurrentQuestion(U.OpenQuestions(thread.questions, pending), db.answers)
      self.answerButton:SetShown(canSend and self.question ~= nil)
    elseif #self:PendingFor(focus.threadId) > 0 then
      self:SetEntries(U.ThreadEntries({ messages = {} }, self:PendingFor(focus.threadId), {}))
    else
      self:SetMessages(U.Color("a0a0a0", "This thread's messages are not synced yet.\n\n"
        .. "Press Sync: the UI reloads and the bridge starts following this thread."))
    end
  else
    self.threadTitle:SetText(self.snapshot and "Pick a thread" or "Welcome")
    self.threadMeta:SetText("")
    local _, message = self:BridgeState()
    local intro = self.snapshot and "Choose a project on the left, then a thread."
      or ("No data from the bridge yet.\n\n"
        .. "1. On this computer run:  t3wow setup <your environment's MCP URL>\n"
        .. "   It signs in and starts the bridge whenever WoW runs.\n"
        .. "2. Restart the game once, then press Sync here (or /reload).")
    if message and self.snapshot then
      intro = intro .. "\n\n" .. U.Color("ff9933", U.Escape(message))
    end
    self:SetMessages(intro)
  end

  local composerEnabled = canSend and (self.composeNew or focus.threadId ~= nil)
  -- Only touch input when it changes: this runs on every data update, and
  -- re-enabling keyboard input must not disturb a composer being typed in.
  if self.composerEnabled ~= composerEnabled then
    self.composerEnabled = composerEnabled
    self.composer:EnableMouse(composerEnabled)
    self.composer:EnableKeyboard(composerEnabled)
    if not composerEnabled then
      self.composer:ClearFocus()
    end
  end
  self.sendButton:SetEnabled(composerEnabled)
  self.modeButton:SetShown(not self.composeNew)
  self:RenderModelButtons(composerEnabled)
  if not canSend then
    placeholderText = "Read-only: approve the bridge with Supervised or higher to send."
  elseif not composerEnabled then
    placeholderText = "Open a thread to send a message."
  elseif self.question then
    placeholderText = "Click an option above, or type your own answer and press Answer."
  end
  self.placeholder:SetText(placeholderText)
  self.composerHint:SetText(self:CanFastSend() and "Ctrl+Enter to send." or "Ctrl+Enter to send. Sending reloads the UI.")

  -- Restore the draft for this thread or project.
  local key = self:DraftKey()
  if key ~= self.draftKey then
    self.draftKey = key
    self.composer:SetText(key and db.drafts and db.drafts[key] or "")
  end
  self.placeholder:SetShown(self.composer:GetText() == "")
end

function T3:Submit(asAnswer)
  local db = T3WowDB
  local text = U.Trim(self.composer:GetText())
  if text == "" or not self:CanSend() then
    return
  end
  local key = self:DraftKey()
  if self.composeNew then
    local projectId = self.newProjectId
    if not projectId then
      return
    end
    self.composeNew = false
    if key and db.drafts then
      db.drafts[key] = nil
    end
    self.composer:SetText("")
    self:Launch(projectId, text)
    return
  end
  local threadId = db.focus.threadId
  if not threadId then
    return
  end
  if key and db.drafts then
    db.drafts[key] = nil
  end
  self.composer:SetText("")
  self.composer:ClearFocus()
  if asAnswer and self.question then
    self:Answer(threadId, self.question, text)
  else
    self:SendMessage(threadId, text, self.sendMode)
  end
end

---------------------------------------------------------------------------
-- Model picker
---------------------------------------------------------------------------

local function effortLabel(model, id)
  for _, effort in ipairs(model and model.efforts or {}) do
    if effort.id == id then
      return effort.label
    end
  end
end

function T3:RenderModelButtons(enabled)
  local key = self:DraftKey()
  local choice = key and self:Choice(key)
  local model = choice and self:FindModel(choice.provider, choice.model)
  local show = enabled and choice ~= nil and #self:Catalog() > 0
  local projectButton = self.projectButton
  projectButton:SetShown(self.composeNew and enabled or false)
  if self.composeNew then
    local project = self:Project(self.newProjectId)
    projectButton:SetText(U.Escape(U.Truncate(project and project.title or "Project", 22)))
  end
  local button = self.modelButton
  button:ClearAllPoints()
  button:SetPoint("LEFT", self.composeNew and projectButton or self.modeButton, "RIGHT", 4, 0)
  button:SetShown(show)
  local hasEffort = show and model and model.efforts and #model.efforts > 0
  self.effortButton:SetShown(hasEffort or false)
  -- The hint fills the space between the left-hand buttons and Answer/Send.
  local leftOf = self.composeNew and (enabled and projectButton or nil) or self.modeButton
  if show then
    leftOf = hasEffort and self.effortButton or button
  end
  local hint = self.composerHint
  hint:ClearAllPoints()
  if leftOf then
    hint:SetPoint("LEFT", leftOf, "RIGHT", 8, 0)
  else
    hint:SetPoint("BOTTOMLEFT", 10, 11)
  end
  hint:SetPoint("RIGHT", self.answerButton:IsShown() and self.answerButton or self.sendButton, "LEFT", -8, 0)
  if not show then
    return
  end
  button:SetText(U.Escape(U.Truncate(model and model.label or choice.model or "Model", 26)))
  if hasEffort then
    local id = choice.effort or model.defaultEffort
    self.effortButton:SetText(U.Escape(effortLabel(model, id) or "Default"))
  end
end

function T3:ShowModelMenu(anchor)
  local key = self:DraftKey()
  local choice = key and self:Choice(key)
  local items = {}
  for _, provider in ipairs(self:Catalog()) do
    table.insert(items, { title = provider.name })
    for _, model in ipairs(provider.models or {}) do
      table.insert(items, {
        text = model.label,
        checked = choice and choice.provider == provider.id and choice.model == model.id,
        onClick = function()
          -- Keep the reasoning level when the new model offers it.
          local effort = choice and choice.effort
          if not effortLabel(model, effort) then
            effort = nil
          end
          self:SetChoice(key, { provider = provider.id, model = model.id, effortKey = model.effortKey, effort = effort })
        end,
      })
    end
  end
  self:ShowMenu(anchor, items)
end

function T3:ShowEffortMenu(anchor)
  local key = self:DraftKey()
  local choice = key and self:Choice(key)
  local model = choice and self:FindModel(choice.provider, choice.model)
  if not model then
    return
  end
  local current = choice.effort or model.defaultEffort
  local items = { { title = model.label } }
  for _, effort in ipairs(model.efforts or {}) do
    table.insert(items, {
      text = effort.label .. (effort.id == model.defaultEffort and U.Color("808080", "  default") or ""),
      checked = effort.id == current,
      onClick = function()
        self:SetChoice(key, { provider = choice.provider, model = choice.model, effortKey = model.effortKey, effort = effort.id })
      end,
    })
  end
  self:ShowMenu(anchor, items)
end

-- A popup list above `anchor`. Items: { title } or { text, checked, onClick }.
function T3:ShowMenu(anchor, items)
  local menu = self.menu
  if not menu then
    menu = CreateFrame("Frame", "T3WowMenu", UIParent, BACKDROP_TEMPLATE)
    menu:SetBackdrop(BACKDROP)
    menu:SetBackdropColor(0.06, 0.06, 0.08, 0.98)
    menu:SetBackdropBorderColor(0.35, 0.35, 0.42, 1)
    menu:SetFrameStrata("FULLSCREEN_DIALOG")
    menu:SetClampedToScreen(true)
    menu:EnableMouse(true)
    menu:Hide()
    table.insert(UISpecialFrames, "T3WowMenu")
    local scroll = CreateFrame("ScrollFrame", "T3WowMenuScroll", menu, "UIPanelScrollFrameTemplate")
    scroll:SetPoint("TOPLEFT", 4, -4)
    local content = CreateFrame("Frame", nil, scroll)
    content:SetSize(MENU_WIDTH - 8, 10)
    scroll:SetScrollChild(content)
    menu.scroll, menu.content, menu.rows = scroll, content, {}
    -- Close on a click anywhere else (the event is missing on some clients).
    if pcall(menu.RegisterEvent, menu, "GLOBAL_MOUSE_DOWN") then
      menu:SetScript("OnEvent", function()
        if menu:IsShown() and not menu:IsMouseOver() and not (menu.anchor and menu.anchor:IsMouseOver()) then
          menu:Hide()
        end
      end)
    end
    self.menu = menu
  end
  if menu:IsShown() and menu.anchor == anchor then
    menu:Hide()
    return
  end
  menu.anchor = anchor

  for index, item in ipairs(items) do
    local row = menu.rows[index]
    if not row then
      row = CreateFrame("Button", nil, menu.content)
      row:SetHeight(MENU_ROW_HEIGHT)
      row:SetPoint("TOPLEFT", 0, -(index - 1) * MENU_ROW_HEIGHT)
      row:SetPoint("RIGHT", menu.content, "RIGHT")
      row:SetHighlightTexture("Interface\\Buttons\\WHITE8X8")
      row:GetHighlightTexture():SetVertexColor(1, 1, 1, 0.08)
      row.label = row:CreateFontString(nil, "OVERLAY", "GameFontHighlightSmall")
      row.label:SetPoint("LEFT", 6, 0)
      row.label:SetPoint("RIGHT", -4, 0)
      row.label:SetJustifyH("LEFT")
      row.label:SetWordWrap(false)
      row:SetScript("OnClick", function(button)
        if button.onClick then
          menu:Hide()
          button.onClick()
        end
      end)
      menu.rows[index] = row
    end
    if item.title then
      row.label:SetText(U.Color("ffd100", U.Escape(item.title)))
      row.onClick = nil
      row:EnableMouse(false)
    else
      row.label:SetText((item.checked and CHECK or "      ") .. " " .. U.Escape(item.text))
      row.onClick = item.onClick
      row:EnableMouse(true)
    end
    row:Show()
  end
  for index = #items + 1, #menu.rows do
    menu.rows[index]:Hide()
  end

  local visible = math.min(#items, MENU_MAX_ROWS)
  local scrolls = #items > MENU_MAX_ROWS
  menu:SetSize(MENU_WIDTH + (scrolls and 22 or 0), visible * MENU_ROW_HEIGHT + 8)
  menu.scroll:SetPoint("BOTTOMRIGHT", scrolls and -26 or -4, 4)
  local bar = _G["T3WowMenuScrollScrollBar"]
  if bar then
    bar:SetShown(scrolls)
  end
  menu.content:SetHeight(#items * MENU_ROW_HEIGHT)
  menu:ClearAllPoints()
  menu:SetPoint("BOTTOMLEFT", anchor, "TOPLEFT", 0, 2)
  menu:Show()
end

---------------------------------------------------------------------------
-- Copy box: rendered text cannot be selected, so messages and links open here.
---------------------------------------------------------------------------

function T3:ShowCopy(title, text)
  local dialog = self.copyDialog
  if not dialog then
    dialog = CreateFrame("Frame", "T3WowCopy", UIParent, BACKDROP_TEMPLATE)
    dialog:SetSize(560, 340)
    dialog:SetPoint("CENTER")
    dialog:SetFrameStrata("DIALOG")
    dialog:SetToplevel(true)
    dialog:EnableMouse(true)
    dialog:SetBackdrop(BACKDROP)
    dialog:SetBackdropColor(0.05, 0.05, 0.07, 0.98)
    dialog:SetBackdropBorderColor(0.35, 0.35, 0.42, 1)
    table.insert(UISpecialFrames, "T3WowCopy")
    dialog.title = dialog:CreateFontString(nil, "OVERLAY", "GameFontNormal")
    dialog.title:SetPoint("TOPLEFT", 12, -10)
    local hint = dialog:CreateFontString(nil, "OVERLAY", "GameFontDisableSmall")
    hint:SetPoint("LEFT", dialog.title, "RIGHT", 10, 0)
    hint:SetText("Ctrl+C to copy, Esc to close")
    local close = CreateFrame("Button", nil, dialog, "UIPanelCloseButton")
    close:SetPoint("TOPRIGHT", 2, 2)
    local scroll, edit = ScrollingEditBox(dialog, "T3WowCopyText", self.fonts.mono)
    scroll:SetPoint("TOPLEFT", 8, -30)
    scroll:SetPoint("BOTTOMRIGHT", -28, 8)
    -- Read only: typing puts the text back.
    edit:SetScript("OnTextChanged", function(widget, userInput)
      if userInput then
        widget:SetText(widget.content or "")
        widget:HighlightText()
      end
    end)
    edit:SetScript("OnEscapePressed", function()
      dialog:Hide()
    end)
    dialog.edit = edit
    self.copyDialog = dialog
  end
  dialog.title:SetText(title)
  dialog.edit.content = U.Escape(text)
  dialog.edit:SetText(dialog.edit.content)
  dialog:Show()
  dialog.edit:SetFocus()
  dialog.edit:HighlightText()
end

function T3:RenderStatus()
  if not self.frame or not self.frame:IsShown() then
    return
  end
  local state, message = self:BridgeState()
  local colors = { ok = "33dd66", stale = "ff9933", missing = "808080", connecting = "ffd100" }
  local color = colors[state] or "ff5555"
  self.statusDot:SetColorTexture(hexToRGB(color))
  local bridge = self.snapshot and self.snapshot.bridge
  self.hostText:SetText(bridge and bridge.server and U.Escape(bridge.server) or "")

  local parts = {}
  if self.snapshot then
    local age = time() - (self.snapshot.generatedAt or 0)
    table.insert(parts, U.Color(color, state == "ok" and "Connected" or state:gsub("_", " ")))
    table.insert(parts, "updated " .. (age < 5 and "just now" or (U.Ago(time(), self.snapshot.generatedAt) .. " ago")))
  else
    table.insert(parts, U.Color(color, "No bridge data"))
  end
  if self.liveSlots > 0 then
    table.insert(parts, "live updates left " .. self:LiveRemaining())
  else
    table.insert(parts, U.Color("ff9933", "no live slots (run t3wow install, then restart WoW)"))
  end
  if message and state ~= "ok" then
    table.insert(parts, U.Escape(message))
  end
  self.statusText:SetText(table.concat(parts, U.Color("505060", "  |  ")))

  local waiting, unsaved = self:OutboxCounts()
  if unsaved > 0 then
    local text = self.syncAfterCombat and "Sync after combat to send" or "press Sync to send"
    self.outboxText:SetText(U.Color("ffd100", unsaved .. " unsent: " .. text))
  elseif waiting > 0 then
    self.outboxText:SetText(U.Color("a0a0a0", "delivering " .. waiting .. "..."))
  else
    self.outboxText:SetText("")
  end
end

-- Key binding and addon compartment entry point.
function T3Wow_Toggle()
  T3:Toggle()
end

BINDING_HEADER_T3WOW = "T3 Code"
BINDING_NAME_T3WOW_TOGGLE = "Toggle the T3 Code window"
