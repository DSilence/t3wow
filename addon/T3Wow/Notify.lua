-- Minimap button, and notifications when a thread finishes or needs input.
--
-- Each snapshot's thread statuses are compared with the previous ones, which
-- are kept in SavedVariables so a thread that finished during a reload still
-- gets noticed. Questions are tracked by request: an agent asking one does
-- not always change the thread's status (a Claude turn stays running). While a thread runs, the addon keeps loading live updates
-- with the window closed (see T3:OnTick), so the news arrives in the background.
local _, ns = ...
local T3, U = ns.T3, ns.Util

local ICON = "Interface\\Icons\\INV_Misc_Gear_01"
local TOAST_SECONDS = 8
-- Ignore changes older than this, e.g. from days without the game open.
local RECENT = 15 * 60
local MAX_TOASTS_AT_ONCE = 3

local FINISHED = { completed = true, failed = true, interrupted = true, cancelled = true, idle = true, rolled_back = true }

---------------------------------------------------------------------------
-- Status changes
---------------------------------------------------------------------------

-- What a status change means for the player, or nil.
local function change(before, now)
  if not before or before == now then
    return nil
  end
  if now == "waiting" then
    return "input"
  end
  if U.ACTIVE[before] and before ~= "waiting" and FINISHED[now] then
    return "finished"
  end
end

function T3:CheckTransitions()
  local db = T3WowDB
  local snapshot = self.snapshot
  if not snapshot or not snapshot.projects then
    return
  end
  local previous = db.statuses
  local current = {}
  local notices = {}
  local now = time()
  for _, project in ipairs(snapshot.projects) do
    for _, thread in ipairs(project.threads or {}) do
      current[thread.id] = thread.status
      local kind = previous and change(previous[thread.id], thread.status)
      if kind and now - (thread.updatedAt or now) < RECENT then
        table.insert(notices, { kind = kind, thread = thread, project = project })
      end
    end
  end
  -- The first snapshot ever only sets the baseline.
  db.statuses = current
  local asked = self:AskedRequests()
  if db.asked then
    local noticed, before = {}, {}
    for _, notice in ipairs(notices) do
      noticed[notice.thread.id] = notice
    end
    for _, threadId in pairs(db.asked) do
      before[threadId] = true
    end
    for requestId, entry in pairs(asked) do
      local notice = noticed[entry.threadId]
      -- A thread going between preview and full sync changes its key, not its question.
      local seen = db.asked[requestId] or db.asked["thread:" .. entry.threadId]
        or (entry.question == nil and before[entry.threadId])
      if not seen and not (notice and notice.kind == "input") then
        notice = self:QuestionNotice(entry)
        if notice then
          noticed[entry.threadId] = notice
          table.insert(notices, notice)
        end
      end
    end
  end
  db.asked = {}
  for requestId, entry in pairs(asked) do
    db.asked[requestId] = entry.threadId
  end
  for id in pairs(db.unread) do
    if not current[id] then
      db.unread[id] = nil
    end
  end
  for _, notice in ipairs(notices) do
    self:Notify(notice)
  end
end

-- Pending questions in the snapshot by request id: { threadId, question }.
-- Preview threads only carry a count, keyed by thread instead.
function T3:AskedRequests()
  local asked = {}
  for threadId, thread in pairs(self.snapshot and self.snapshot.threads or {}) do
    if thread.questions then
      for _, question in ipairs(thread.questions) do
        if not asked[question.requestId] then
          asked[question.requestId] = { threadId = threadId, question = question }
        end
      end
    elseif (thread.pending or 0) > 0 then
      asked["thread:" .. threadId] = { threadId = threadId }
    end
  end
  return asked
end

function T3:QuestionNotice(entry)
  local thread = self:Thread(entry.threadId) or {}
  local summary, project = self:ThreadSummary(entry.threadId)
  summary = summary or { id = entry.threadId, title = thread.title, status = thread.status }
  project = project or self:Project(thread.projectId) or {}
  return { kind = "input", thread = summary, project = project, question = entry.question }
end

function T3:Notify(notice)
  if not self.ready then
    -- The inbox loads before the UI exists; tell the player once it does.
    self.queuedNotices = self.queuedNotices or {}
    table.insert(self.queuedNotices, notice)
    return
  end
  local db = T3WowDB
  local thread, project = notice.thread, notice.project
  local viewing = self.frame and self.frame:IsShown() and db.focus.threadId == thread.id
  if not viewing then
    db.unread[thread.id] = time()
  end
  if not db.settings.notify then
    self:Fire("DATA")
    return
  end
  local label, color = U.Status(thread.status)
  local what = "finished: " .. label:lower()
  if notice.kind == "input" then
    color = "ff9933"
    what = notice.question and ("asks: " .. U.Escape(U.Truncate(((notice.question.question or ""):gsub("%s+", " ")), 60)))
      or "needs your input"
  end
  local title = U.Escape(U.Truncate(thread.title or "Thread", 60))
  self:Print(U.Color(color, title) .. " " .. what .. U.Color("808080", "  (" .. U.Escape(project.title or "") .. ")"))
  if db.settings.sound and PlaySound then
    local sound = notice.kind == "input" and (SOUNDKIT and SOUNDKIT.READY_CHECK or 8960)
      or (SOUNDKIT and SOUNDKIT.TELL_MESSAGE or 3081)
    PlaySound(sound, "Master")
  end
  if FlashClientIcon then
    FlashClientIcon()
  end
  if not viewing then
    self:Toast(notice, title, U.Color(color, what))
  end
  self:Fire("DATA")
end

function T3:FlushNotices()
  local queued = self.queuedNotices or {}
  self.queuedNotices = nil
  for index, notice in ipairs(queued) do
    if index > MAX_TOASTS_AT_ONCE then
      self:Print(string.format("%d more threads changed while you were away.", #queued - MAX_TOASTS_AT_ONCE))
      break
    end
    self:Notify(notice)
  end
end

function T3:UnreadCount()
  local count = 0
  for _ in pairs(T3WowDB.unread) do
    count = count + 1
  end
  return count
end

-- The most recently finished thread the player has not looked at.
function T3:LatestUnread()
  local best, at = nil, 0
  for id, when in pairs(T3WowDB.unread) do
    if when > at then
      best, at = id, when
    end
  end
  return best
end

function T3:Open(threadId)
  local summary, project = self:ThreadSummary(threadId)
  self.composeNew = false
  self.frame:Show()
  self:Select(project and project.id or T3WowDB.focus.projectId, threadId)
end

---------------------------------------------------------------------------
-- Toast: a clickable card under the top of the screen
---------------------------------------------------------------------------

function T3:Toast(notice, title, body)
  local toast = self.toast
  if not toast then
    toast = CreateFrame("Button", "T3WowToast", UIParent, BackdropTemplateMixin and "BackdropTemplate" or nil)
    toast:SetSize(340, 52)
    toast:SetPoint("TOP", UIParent, "TOP", 0, -110)
    toast:SetFrameStrata("DIALOG")
    toast:SetBackdrop({ bgFile = "Interface\\Buttons\\WHITE8X8", edgeFile = "Interface\\Buttons\\WHITE8X8", edgeSize = 1 })
    toast:SetBackdropColor(0.05, 0.05, 0.07, 0.94)
    toast:SetBackdropBorderColor(0.4, 0.55, 0.9, 1)
    toast:RegisterForClicks("AnyUp")
    local icon = toast:CreateTexture(nil, "ARTWORK")
    icon:SetSize(34, 34)
    icon:SetPoint("LEFT", 9, 0)
    icon:SetTexture(ICON)
    toast.title = toast:CreateFontString(nil, "OVERLAY", "GameFontHighlight")
    toast.title:SetPoint("TOPLEFT", icon, "TOPRIGHT", 10, -1)
    toast.title:SetPoint("RIGHT", -10, 0)
    toast.title:SetJustifyH("LEFT")
    toast.title:SetWordWrap(false)
    toast.body = toast:CreateFontString(nil, "OVERLAY", "GameFontNormalSmall")
    toast.body:SetPoint("TOPLEFT", toast.title, "BOTTOMLEFT", 0, -4)
    toast.body:SetPoint("RIGHT", -10, 0)
    toast.body:SetJustifyH("LEFT")
    toast.body:SetWordWrap(false)
    toast:SetScript("OnClick", function(button, mouse)
      button:Hide()
      if mouse ~= "RightButton" and button.threadId then
        self:Open(button.threadId)
      end
    end)
    toast:SetScript("OnUpdate", function(button)
      if GetTime() >= (button.hideAt or 0) and not button:IsMouseOver() then
        button:Hide()
      end
    end)
    toast:Hide()
    self.toast = toast
  end
  toast.threadId = notice.thread.id
  toast.title:SetText(title)
  toast.body:SetText(body .. U.Color("808080", "  - click to open"))
  toast.hideAt = GetTime() + TOAST_SECONDS
  toast:Show()
end

---------------------------------------------------------------------------
-- Minimap button
---------------------------------------------------------------------------

local function placeOnMinimap(button, angle)
  local radians = math.rad(angle)
  local radius = (Minimap:GetWidth() or 140) / 2 + 5
  button:ClearAllPoints()
  button:SetPoint("CENTER", Minimap, "CENTER", math.cos(radians) * radius, math.sin(radians) * radius)
end

function T3:CreateMinimapButton()
  if not Minimap or self.minimapButton then
    return
  end
  local settings = T3WowDB.minimap
  local button = CreateFrame("Button", "T3WowMinimapButton", Minimap)
  button:SetSize(31, 31)
  button:SetFrameStrata("MEDIUM")
  button:SetFrameLevel(8)
  button:RegisterForClicks("AnyUp")
  button:RegisterForDrag("LeftButton")
  button:SetHighlightTexture("Interface\\Minimap\\UI-Minimap-ZoomButton-Highlight")

  local background = button:CreateTexture(nil, "BACKGROUND")
  background:SetSize(20, 20)
  background:SetTexture("Interface\\Minimap\\UI-Minimap-Background")
  background:SetPoint("TOPLEFT", 7, -5)
  local icon = button:CreateTexture(nil, "ARTWORK")
  icon:SetSize(17, 17)
  icon:SetTexture(ICON)
  icon:SetPoint("TOPLEFT", 7, -6)
  local border = button:CreateTexture(nil, "OVERLAY")
  border:SetSize(53, 53)
  border:SetTexture("Interface\\Minimap\\MiniMap-TrackingBorder")
  border:SetPoint("TOPLEFT")

  -- Green while something runs, a count of finished threads you have not opened.
  local running = button:CreateTexture(nil, "OVERLAY")
  running:SetSize(10, 10)
  running:SetPoint("BOTTOMLEFT", 4, 4)
  running:SetTexture("Interface\\COMMON\\Indicator-Green")
  button.running = running
  local badge = button:CreateFontString(nil, "OVERLAY", "NumberFontNormalSmall")
  badge:SetPoint("TOPRIGHT", -2, -2)
  button.badge = badge

  button:SetScript("OnClick", function(_, mouse)
    local unread = self:LatestUnread()
    if mouse == "RightButton" and unread then
      self:Open(unread)
    else
      self:Toggle()
    end
  end)
  button:SetScript("OnDragStart", function(b)
    b:SetScript("OnUpdate", function()
      local mx, my = Minimap:GetCenter()
      local cx, cy = GetCursorPosition()
      local scale = Minimap:GetEffectiveScale()
      settings.angle = math.deg(math.atan2(cy / scale - my, cx / scale - mx))
      placeOnMinimap(b, settings.angle)
    end)
  end)
  button:SetScript("OnDragStop", function(b)
    b:SetScript("OnUpdate", nil)
  end)
  button:SetScript("OnEnter", function(b)
    GameTooltip:SetOwner(b, "ANCHOR_LEFT")
    self:MinimapTooltip(GameTooltip)
    GameTooltip:Show()
  end)
  button:SetScript("OnLeave", GameTooltip_Hide)

  self.minimapButton = button
  placeOnMinimap(button, settings.angle)
  button:SetShown(not settings.hide)
  self:On("DATA", function()
    self:RenderMinimapButton()
  end)
  self:On("SELECT", function()
    self:RenderMinimapButton()
  end)
  self:RenderMinimapButton()
end

function T3:RunningThreads()
  local running = {}
  for _, project in ipairs(self.snapshot and self.snapshot.projects or {}) do
    for _, thread in ipairs(project.threads or {}) do
      if U.ACTIVE[thread.status] then
        table.insert(running, thread)
      end
    end
  end
  return running
end

function T3:RenderMinimapButton()
  local button = self.minimapButton
  if not button then
    return
  end
  button.running:SetShown(#self:RunningThreads() > 0)
  local unread = self:UnreadCount()
  button.badge:SetText(unread > 0 and U.Color("ffd100", tostring(unread)) or "")
end

function T3:MinimapTooltip(tooltip)
  tooltip:SetText("T3 Code")
  local state, message = self:BridgeState()
  if state ~= "ok" and message then
    tooltip:AddLine(message, 1, 0.6, 0.2, true)
  end
  for _, thread in ipairs(self:RunningThreads()) do
    local label, color = U.Status(thread.status)
    tooltip:AddLine(U.Color(color, label) .. "  " .. U.Escape(U.Truncate(thread.title or "", 50)), 1, 1, 1)
  end
  local unread = self:UnreadCount()
  if unread > 0 then
    tooltip:AddLine(unread .. (unread == 1 and " thread" or " threads") .. " finished since you last looked", 1, 0.82, 0)
  end
  tooltip:AddLine("Click: open or close. Right-click: open the latest finished thread. Drag to move.", 0.6, 0.6, 0.6, true)
end

function T3:SetMinimapShown(shown)
  T3WowDB.minimap.hide = not shown
  if self.minimapButton then
    self.minimapButton:SetShown(shown)
  end
end
