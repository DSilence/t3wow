-- State, the link to the bridge, and slash commands.
--
-- WoW addons cannot open network connections or read files at runtime, so the
-- t3wow bridge (a small program next to the game) talks to T3 Code and exchanges data with the game:
--   bridge -> game: it rewrites the Data.lua of T3Wow_Inbox (read on every UI
--     load) and of the load-on-demand T3Wow_LiveNNN addons. Each of those can
--     be loaded once per UI session, so loading the next one pulls the latest
--     snapshot without a /reload.
--   game -> bridge: commands go into T3WowDB.outbox. WoW writes SavedVariables
--     only on /reload or logout. To send without a reload, FastSend.lua shows
--     the outbox to the bridge as a screenshot; the reload is the fallback.
local ADDON, ns = ...
local U = ns.Util

local T3 = {}
ns.T3 = T3
_G.T3Wow = T3

local LoadAddOn = C_AddOns and C_AddOns.LoadAddOn or LoadAddOn
local GetAddOnInfo = C_AddOns and C_AddOns.GetAddOnInfo or GetAddOnInfo
local IsAddOnLoaded = C_AddOns and C_AddOns.IsAddOnLoaded or IsAddOnLoaded

local MAX_WATCHED = 8
local STALE_AFTER = 75
local FIRST_POLL_DELAY = 2
local FAILURE_VISIBLE_FOR = 300
-- Live update interval with the window closed, while something runs.
local BACKGROUND_INTERVAL = 10

T3.snapshot = nil
T3.liveSlots = 0
T3.liveCursor = 0
T3.lastPoll = 0
T3.listeners = {}

local DEFAULTS = {
  nextSeq = 1,
  outbox = {},
  focus = {},
  watch = {},
  seenFailures = {},
  -- Model picked per thread id (or "new:<projectId>") until it is sent.
  models = {},
  -- Threads that finished since the player last opened them: id -> time.
  unread = {},
  settings = { autoReload = true, liveInterval = 3, idleInterval = 12, fontSize = 13, notify = true, sound = true, fastSend = true },
  minimap = { angle = 200, hide = false },
  ui = {},
}

local function applyDefaults(target, defaults)
  for key, value in pairs(defaults) do
    if target[key] == nil then
      target[key] = type(value) == "table" and applyDefaults({}, value) or value
    elseif type(value) == "table" and type(target[key]) == "table" then
      applyDefaults(target[key], value)
    end
  end
  return target
end

local function newInstanceId()
  local chars = {}
  for i = 1, 16 do
    chars[i] = string.format("%x", math.random(0, 15))
  end
  return string.format("%x-%s", time(), table.concat(chars))
end

function T3:Print(message)
  DEFAULT_CHAT_FRAME:AddMessage("|cff8fb8ffT3:|r " .. message)
end

-- Minimal observer so the UI can redraw when data changes.
function T3:On(event, fn)
  self.listeners[event] = self.listeners[event] or {}
  table.insert(self.listeners[event], fn)
end

function T3:Fire(event, ...)
  for _, fn in ipairs(self.listeners[event] or {}) do
    fn(...)
  end
end

---------------------------------------------------------------------------
-- Snapshots from the bridge
---------------------------------------------------------------------------

-- Called by T3Wow_Inbox and T3Wow_LiveNNN when they load.
function T3Wow_Receive(snapshot, source)
  if type(snapshot) ~= "table" then
    return
  end
  if T3.snapshot and (snapshot.seq or 0) < (T3.snapshot.seq or 0) then
    return
  end
  T3.snapshot = snapshot
  T3.snapshotSource = source
  -- The inbox can load before ADDON_LOADED has prepared T3WowDB; acks are applied then.
  if T3.dbReady then
    T3:ApplyAcks()
    T3:CheckTransitions()
  end
  T3:Fire("DATA")
end

function T3:Acks()
  local acks = self.snapshot and self.snapshot.acks
  return acks and self.dbReady and acks[T3WowDB.instanceId]
end

-- Drop delivered commands from the outbox and remember their results.
function T3:ApplyAcks()
  local ack = self:Acks()
  if not ack then
    return
  end
  local db = T3WowDB
  local kept = {}
  for _, command in ipairs(db.outbox) do
    if command.seq > (ack.seq or 0) then
      table.insert(kept, command)
    end
  end
  db.outbox = kept
  for seq in pairs(db.seenFailures) do
    if seq < (ack.seq or 0) - 100 then
      db.seenFailures[seq] = nil
    end
  end
  for _, result in ipairs(ack.results or {}) do
    if not result.ok and not db.seenFailures[result.seq] then
      db.seenFailures[result.seq] = time()
      self:Print("|cffff5555Not delivered:|r " .. U.Escape(result.message or "unknown error"))
    end
  end
  -- Open a thread the player launched from the game once it exists.
  for _, result in ipairs(ack.results or {}) do
    if db.awaitLaunch and result.seq == db.awaitLaunch then
      db.awaitLaunch = nil
      if result.ok and result.threadId then
        self:Select(db.awaitLaunchProject or db.focus.projectId, result.threadId)
      end
    end
  end
end

-- Recent delivery failures for a thread (or for commands without one), newest last.
-- Each stays visible for a few minutes after it is first seen.
function T3:FailuresFor(threadId)
  local ack = self:Acks()
  local seen = T3WowDB.seenFailures
  local now = time()
  local failures = {}
  for _, result in ipairs(ack and ack.results or {}) do
    if not result.ok and (result.threadId == threadId or result.threadId == nil) then
      seen[result.seq] = seen[result.seq] or now
      if now - seen[result.seq] < FAILURE_VISIBLE_FOR then
        table.insert(failures, result)
      end
    end
  end
  return failures
end

-- Commands waiting for the bridge, and how many of them have not reached it
-- yet (neither written by a reload nor sent by screenshot).
function T3:OutboxCounts()
  local unsaved = 0
  for _, command in ipairs(T3WowDB.outbox) do
    if not command.saved and not self.shotAt[command.seq] then
      unsaved = unsaved + 1
    end
  end
  return #T3WowDB.outbox, unsaved
end

function T3:PendingFor(threadId)
  local pending = {}
  for _, command in ipairs(T3WowDB.outbox) do
    if command.threadId == threadId then
      table.insert(pending, command)
    end
  end
  return pending
end

function T3:Project(projectId)
  for _, project in ipairs(self.snapshot and self.snapshot.projects or {}) do
    if project.id == projectId then
      return project
    end
  end
end

function T3:Thread(threadId)
  return threadId and self.snapshot and self.snapshot.threads and self.snapshot.threads[threadId]
end

-- The list entry for a thread, which exists even when its messages were not synced.
function T3:ThreadSummary(threadId)
  for _, project in ipairs(self.snapshot and self.snapshot.projects or {}) do
    for _, thread in ipairs(project.threads or {}) do
      if thread.id == threadId then
        return thread, project
      end
    end
  end
end

function T3:BridgeState()
  local snapshot = self.snapshot
  if not snapshot then
    return "missing", "No data from the bridge yet. Run `t3wow setup` on this computer."
  end
  local bridge = snapshot.bridge or {}
  local age = time() - (snapshot.generatedAt or 0)
  if bridge.status ~= "ok" then
    return bridge.status or "error", bridge.message or "Bridge error"
  end
  if age > STALE_AFTER and self.liveCursor >= self.liveSlots then
    return "stale", "Live updates used up. Sync to refresh."
  end
  if age > STALE_AFTER and self.liveCursor > 0 then
    return "stale", "No recent data. Is the bridge running? Check `t3wow status`."
  end
  return "ok", bridge.message
end

---------------------------------------------------------------------------
-- Live updates
---------------------------------------------------------------------------

local function slotName(index)
  return string.format("T3Wow_Live%03d", index)
end

function T3:CountLiveSlots()
  local count = 0
  while true do
    local name, _, _, loadable, reason = GetAddOnInfo(slotName(count + 1))
    if not name or reason == "MISSING" then
      break
    end
    count = count + 1
  end
  self.liveSlots = count
end

function T3:LiveRemaining()
  return math.max(0, self.liveSlots - self.liveCursor)
end

-- Load the next live slot; its Data.lua calls T3Wow_Receive with the latest snapshot.
function T3:PollLive()
  self.lastPoll = GetTime()
  if InCombatLockdown() then
    return
  end
  while self.liveCursor < self.liveSlots do
    self.liveCursor = self.liveCursor + 1
    local name = slotName(self.liveCursor)
    if not IsAddOnLoaded(name) then
      local loaded, reason = LoadAddOn(name)
      if not loaded and reason == "DISABLED" and not self.warnedDisabled then
        self.warnedDisabled = true
        self:Print("Some T3Wow_Live addons are disabled; enable them in the AddOns list for live updates.")
      end
      if loaded then
        break
      end
    end
  end
  self:Fire("STATUS")
end

function T3:PollInterval()
  local settings = T3WowDB.settings
  local bridge = self.snapshot and self.snapshot.bridge
  if #T3WowDB.outbox > 0 or (bridge and bridge.active) then
    return settings.liveInterval
  end
  return settings.idleInterval
end

function T3:OnTick()
  if not self.ready then
    return
  end
  self:CheckFastSend()
  local visible = self.frame and self.frame:IsShown()
  local interval = self:PollInterval()
  -- Poll while the window is open and right after sending. With the window
  -- closed, keep an eye on running threads so their end can be announced.
  local watching = T3WowDB.settings.notify and #self:RunningThreads() > 0
  if not visible and #T3WowDB.outbox == 0 then
    interval = BACKGROUND_INTERVAL
  end
  local due = GetTime() - self.lastPoll >= interval
  if (visible or #T3WowDB.outbox > 0 or watching) and due and self:LiveRemaining() > 0 then
    self:PollLive()
  end
  self:Fire("STATUS")
end

---------------------------------------------------------------------------
-- Commands to the bridge
---------------------------------------------------------------------------

function T3:CanSend()
  local bridge = self.snapshot and self.snapshot.bridge
  return not bridge or bridge.canOperate ~= false
end

function T3:Enqueue(command)
  local db = T3WowDB
  command.seq = db.nextSeq
  command.at = time()
  db.nextSeq = db.nextSeq + 1
  table.insert(db.outbox, command)
  self:Fire("DATA")
  if self:FastSend() then
    -- On its way by screenshot; the acknowledgement comes with a live update.
    self:Fire("STATUS")
  elseif db.settings.autoReload then
    self:Sync()
  else
    self:Print("Queued. Type /t3 sync (or press Sync) to deliver it.")
  end
  return command
end

---------------------------------------------------------------------------
-- Models
---------------------------------------------------------------------------

function T3:Catalog()
  return self.snapshot and self.snapshot.models or {}
end

function T3:FindModel(providerId, modelId)
  for _, provider in ipairs(self:Catalog()) do
    if provider.id == providerId then
      for _, model in ipairs(provider.models or {}) do
        if model.id == modelId then
          return model, provider
        end
      end
    end
  end
end

local function sameChoice(a, b)
  return a and b and a.provider == b.provider and a.model == b.model and (a.effort or "") == (b.effort or "")
end

-- What a thread (or a new thread in a project) runs now, before any change in game.
function T3:BaseChoice(threadId, projectId)
  if threadId then
    local thread = self:Thread(threadId)
    if thread and thread.selection then
      return thread.selection
    end
    local summary = self:ThreadSummary(threadId)
    return summary and summary.provider and { provider = summary.provider, model = summary.model }
  end
  local project = self:Project(projectId)
  if project and project.defaultModel then
    return { provider = project.defaultModel.provider, model = project.defaultModel.model }
  end
  -- The bridge falls back to the project's latest thread, like this.
  local latest = project and project.threads and project.threads[1]
  if latest and latest.provider then
    return { provider = latest.provider, model = latest.model }
  end
  local provider = self:Catalog()[1]
  local model = provider and provider.models and provider.models[1]
  return model and { provider = provider.id, model = model.id }
end

-- The model to use for a thread or "new:<projectId>" key: the player's pick, else the base.
function T3:Choice(key)
  local threadId, projectId = key, nil
  if key and key:match("^new:") then
    threadId, projectId = nil, key:sub(5)
  end
  local base = key and self:BaseChoice(threadId, projectId)
  local picked = key and T3WowDB.models[key]
  if picked and threadId and sameChoice(picked, base) then
    -- The thread runs the picked model now.
    T3WowDB.models[key] = nil
    picked = nil
  end
  return picked or base, picked ~= nil
end

function T3:SetChoice(key, choice)
  if key then
    T3WowDB.models[key] = choice
    self:Fire("DATA")
  end
end

local function commandModel(choice)
  return choice and {
    provider = choice.provider,
    model = choice.model,
    effortKey = choice.effort and choice.effortKey or nil,
    effort = choice.effort,
  }
end

function T3:SendMessage(threadId, text, mode)
  local choice, picked = self:Choice(threadId)
  return self:Enqueue({
    kind = "send",
    threadId = threadId,
    text = text,
    mode = mode or "auto",
    -- Only a model picked in game switches the thread; otherwise it keeps its own.
    model = picked and commandModel(choice) or nil,
  })
end

function T3:Launch(projectId, text)
  local key = "new:" .. projectId
  local choice, picked = self:Choice(key)
  local command = {
    kind = "launch",
    projectId = projectId,
    title = U.TitleFor(text),
    text = text,
    model = picked and commandModel(choice) or nil,
  }
  T3WowDB.models[key] = nil
  T3WowDB.awaitLaunch = T3WowDB.nextSeq
  T3WowDB.awaitLaunchProject = projectId
  return self:Enqueue(command)
end

function T3:Interrupt(threadId)
  return self:Enqueue({ kind = "interrupt", threadId = threadId })
end

-- T3 compacts the conversation when a message is exactly "/compact". Queued,
-- so it waits for a running turn instead of steering into it.
T3.COMPACT = "/compact"

function T3:Compact(threadId)
  return self:Enqueue({ kind = "send", threadId = threadId, text = T3.COMPACT, mode = "queue" })
end

-- T3 resolves a request with the first response, so a request with several
-- questions is answered all at once: answers wait in T3WowDB.answers until the
-- request's last question has one too.
function T3:Answer(threadId, question, text)
  return self:AnswerWith(threadId, question, U.AnswerFor(question, text))
end

--- Answer with a value: text, or a list for a multi-select question.
function T3:AnswerWith(threadId, question, answer)
  local db = T3WowDB
  db.answers = db.answers or {}
  self.picks[U.PickKey(question)] = nil
  local draft = db.answers[question.requestId] or { threadId = threadId, answers = {} }
  draft.answers[question.id] = answer
  local thread = self:Thread(threadId)
  local lines = {}
  for _, other in ipairs(thread and thread.questions or { question }) do
    if other.requestId == question.requestId then
      local answer = draft.answers[other.id]
      if answer == nil then
        db.answers[question.requestId] = draft
        self:Fire("DATA")
        return
      end
      lines[#lines + 1] = U.AnswerText(answer)
    end
  end
  db.answers[question.requestId] = nil
  return self:Enqueue({
    kind = "answer",
    threadId = threadId,
    requestId = question.requestId,
    answers = draft.answers,
    text = table.concat(lines, "\n"),
  })
end

-- Ticked multi-select options by U.PickKey, until sent (not saved).
T3.picks = {}

--- A click on a question option or the Send choices button.
function T3:QuestionAction(action)
  local threadId = T3WowDB.focus.threadId
  local question = action.question
  if not threadId or not self:CanSend() then
    return
  end
  if action.kind == "submit" then
    local values = U.PickedValues(question, self.picks[U.PickKey(question)])
    if #values > 0 then
      self:AnswerWith(threadId, question, values)
    end
  elseif question.multi then
    local key = U.PickKey(question)
    local ticked = self.picks[key] or {}
    ticked[action.index] = not ticked[action.index] or nil
    self.picks[key] = ticked
    self:Fire("DATA")
  else
    local option = question.options[action.index]
    self:AnswerWith(threadId, question, option.value or option.label)
  end
end

--- Drop answers kept for requests of `thread` that are no longer pending.
function T3:PruneAnswers(thread)
  local drafts = T3WowDB.answers
  if not drafts or not thread or thread.partial then
    return
  end
  local live = {}
  for _, question in ipairs(thread.questions or {}) do
    live[question.requestId] = true
  end
  for requestId, draft in pairs(drafts) do
    if draft.threadId == thread.id and not live[requestId] then
      drafts[requestId] = nil
    end
  end
end

-- Why fresh data needs a reload right now, or nil when loading the next live
-- slot is enough: queued commands only reach the bridge through SavedVariables,
-- which WoW writes on reload; the same goes for telling the bridge to follow
-- the open thread in full; and live slots load once per UI session.
function T3:ReloadReason()
  local _, unsaved = self:OutboxCounts()
  if unsaved > 0 then
    return "send"
  end
  local threadId = T3WowDB.focus.threadId
  if threadId and self.snapshot then
    local thread = self:Thread(threadId)
    if not thread or thread.partial then
      return "follow"
    end
  end
  if self:LiveRemaining() == 0 then
    return "refresh"
  end
end

-- Get the latest data, reloading the UI only when needed (or when forced).
function T3:Sync(force)
  local reason = self:ReloadReason()
  if not force and not reason then
    self:PollLive()
    return
  end
  if not force and reason == "send" and self:FastSend() then
    return
  end
  if not force and reason == "follow" and self:FastFollow() then
    return
  end
  if InCombatLockdown() then
    self.syncAfterCombat = true
    self:Print("In combat: will be sent after combat when you press Sync.")
    self:Fire("STATUS")
    return
  end
  ReloadUI()
end

function T3:Select(projectId, threadId)
  local db = T3WowDB
  db.focus.projectId = projectId
  db.focus.threadId = threadId
  if threadId then
    db.unread[threadId] = nil
    db.watch[threadId] = time()
    -- Keep only the most recent few so the bridge does not sync everything.
    local ids = {}
    for id in pairs(db.watch) do
      table.insert(ids, id)
    end
    table.sort(ids, function(a, b)
      return db.watch[a] > db.watch[b]
    end)
    for i = MAX_WATCHED + 1, #ids do
      db.watch[ids[i]] = nil
    end
  end
  self:Fire("SELECT")
end

---------------------------------------------------------------------------
-- Events and slash commands
---------------------------------------------------------------------------

local events = CreateFrame("Frame")
events:RegisterEvent("ADDON_LOADED")
events:RegisterEvent("PLAYER_LOGIN")
events:RegisterEvent("PLAYER_REGEN_ENABLED")
events:SetScript("OnEvent", function(_, event, arg1)
  local self = T3
  if event == "ADDON_LOADED" and arg1 == ADDON then
    T3WowDB = applyDefaults(T3WowDB or {}, DEFAULTS)
    if not T3WowDB.instanceId then
      T3WowDB.instanceId = newInstanceId()
    end
    -- Everything already in the outbox was written to disk by the last reload.
    for _, command in ipairs(T3WowDB.outbox) do
      command.saved = true
    end
    self.dbReady = true
    if self.snapshot then
      self:ApplyAcks()
      self:CheckTransitions()
    end
  elseif event == "PLAYER_LOGIN" then
    self:CountLiveSlots()
    self.ready = true
    -- The inbox loaded with the UI; ask for fresher data shortly after.
    self.lastPoll = GetTime() - self:PollInterval() + FIRST_POLL_DELAY
    C_Timer.NewTicker(1, function()
      self:OnTick()
    end)
    self:CreateUI()
    self:CreateMinimapButton()
    if T3WowDB.ui.open or #T3WowDB.outbox > 0 then
      self.frame:Show()
    end
    self:FlushNotices()
  elseif event == "PLAYER_REGEN_ENABLED" and self.syncAfterCombat then
    self.syncAfterCombat = false
    self:Print("Combat over: press Sync to send your queued message.")
    self:Fire("STATUS")
  end
end)

SLASH_T3WOW1 = "/t3"
SLASH_T3WOW2 = "/t3wow"
SlashCmdList.T3WOW = function(input)
  local command, rest = U.Trim(input):match("^(%S*)%s*(.-)$")
  command = (command or ""):lower()
  local settings = T3WowDB.settings
  if command == "" or command == "show" or command == "toggle" then
    T3:Toggle()
  elseif command == "sync" then
    T3:Sync()
  elseif command == "reload" then
    T3:Sync(true)
  elseif command == "send" and rest ~= "" then
    local threadId = T3WowDB.focus.threadId
    if not threadId then
      T3:Print("Open a thread first.")
    else
      T3:SendMessage(threadId, rest)
    end
  elseif command == "autoreload" then
    settings.autoReload = rest ~= "off"
    T3:Print("Reload after sending: " .. (settings.autoReload and "on" or "off"))
  elseif command == "interval" and tonumber(rest) then
    settings.liveInterval = math.max(1, tonumber(rest))
    T3:Print("Live update interval: " .. settings.liveInterval .. "s")
  elseif command == "font" and tonumber(rest) then
    settings.fontSize = math.min(24, math.max(9, tonumber(rest)))
    T3:Fire("FONT")
  elseif command == "compact" then
    local threadId = T3WowDB.focus.threadId
    if threadId then
      T3:Compact(threadId)
    else
      T3:Print("Open a thread first.")
    end
  elseif command == "notify" or command == "sound" then
    settings[command] = rest ~= "off"
    T3:Print((command == "notify" and "Notifications when a thread finishes: " or "Notification sound: ")
      .. (settings[command] and "on" or "off"))
  elseif command == "fastsend" then
    settings.fastSend = rest ~= "off"
    T3.fastSendBroken = false
    T3:Print("Send without reloading (by screenshot): " .. (settings.fastSend and "on" or "off"))
  elseif command == "minimap" then
    T3:SetMinimapShown(rest ~= "off")
  elseif command == "clear" then
    T3WowDB.outbox = {}
    T3:Print("Cleared unsent commands.")
    T3:Fire("DATA")
  elseif command == "reset" then
    T3WowDB.ui = {}
    T3:Print("Window position reset. /reload to apply.")
  elseif command == "status" then
    local state, message = T3:BridgeState()
    T3:Print(string.format("bridge %s%s, live updates left %d/%d, unsent %d", state,
      message and (": " .. message) or "", T3:LiveRemaining(), T3.liveSlots, #T3WowDB.outbox))
  else
    T3:Print("/t3 - toggle window | /t3 sync | /t3 reload | /t3 send <text> | /t3 compact | /t3 status | /t3 autoreload on|off | /t3 interval <sec> | /t3 font <size> | /t3 notify on|off | /t3 sound on|off | /t3 minimap on|off | /t3 fastsend on|off | /t3 clear | /t3 reset")
  end
end
