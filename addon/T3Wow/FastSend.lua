-- Sending without a reload: the outbox goes to the bridge as a screenshot of
-- a cell grid (see Shot.lua). The grid shows for a frame or two in the
-- top-left corner. When the bridge does not acknowledge in time, or the
-- screenshot fails, the addon falls back to the reload.
local _, ns = ...
local T3, S = ns.T3, ns.Shot

-- Seconds to wait for the bridge to acknowledge commands sent this way.
local ACK_TIMEOUT = 20
-- Seconds to wait for the screenshot itself.
local SHOT_TIMEOUT = 5
-- Sync only re-sends the open thread this often (it waits for the bridge).
local FOLLOW_EVERY = 10

local GetCVar = C_CVar and C_CVar.GetCVar or GetCVar
local SetCVar = C_CVar and C_CVar.SetCVar or SetCVar
local floor = math.floor

-- Outbox seqs sent by screenshot this session, with the time they were taken.
T3.shotAt = {}

function T3:CanFastSend()
  local bridge = self.snapshot and self.snapshot.bridge
  if T3WowDB.settings.fastSend == false or self.fastSendBroken then
    return false
  end
  if not (bridge and bridge.shots) or not Screenshot or not GetPhysicalScreenSize then
    return false
  end
  local width, height = GetPhysicalScreenSize()
  return (width or 0) >= S.GRID_WIDTH * S.CELL and (height or 0) >= (S.MAX_ROWS + 1) * S.CELL
end

-- The parts of SavedVariables the bridge reads.
function T3:ShotPayload()
  local db = T3WowDB
  return S.JSON({ v = 1, instanceId = db.instanceId, outbox = db.outbox, focus = db.focus, watch = db.watch })
end

--- Send the outbox (and the open thread) to the bridge now. False when this
--- channel is not available and a reload is needed instead.
function T3:FastSend()
  if not self:CanFastSend() then
    return false
  end
  if self.shot then
    -- One is being taken; send again afterwards with whatever was added.
    self.shot.again = true
    return true
  end
  local chunks = S.Chunks(self:ShotPayload(), math.random(1, 2147483646))
  if #chunks > S.MAX_CHUNKS then
    return false
  end
  local seqs = {}
  for _, command in ipairs(T3WowDB.outbox) do
    seqs[#seqs + 1] = command.seq
  end
  self.shot = { chunks = chunks, index = 1, seqs = seqs }
  self:ShootChunk()
  return true
end

function T3:ShotFrame()
  local frame = self.shotFrame
  if frame then
    return frame
  end
  -- No parent: drawn even with the interface hidden (Alt+Z).
  frame = CreateFrame("Frame", "T3WowShot")
  frame:SetFrameStrata("TOOLTIP")
  frame:SetPoint("TOPLEFT", 0, 0)
  frame.cells = {}
  frame:Hide()
  frame:RegisterEvent("SCREENSHOT_SUCCEEDED")
  frame:RegisterEvent("SCREENSHOT_FAILED")
  frame:SetScript("OnEvent", function(_, event)
    self:OnShot(event == "SCREENSHOT_SUCCEEDED")
  end)
  self.shotFrame = frame
  return frame
end

function T3:DrawChunk(bytes)
  local frame = self:ShotFrame()
  local _, height = GetPhysicalScreenSize()
  -- One unit is one physical pixel, so cells land exactly on the pixel grid.
  frame:SetScale(768 / height)
  local size = S.CELL
  local grid = S.Layout(bytes)
  frame:SetSize(S.GRID_WIDTH * size, #grid * size)
  local used = 0
  for y, row in ipairs(grid) do
    for x, value in ipairs(row) do
      used = used + 1
      local cell = frame.cells[used]
      if not cell then
        cell = frame:CreateTexture(nil, "ARTWORK")
        cell:SetSize(size, size)
        frame.cells[used] = cell
      end
      cell:ClearAllPoints()
      cell:SetPoint("TOPLEFT", frame, "TOPLEFT", (x - 1) * size, -(y - 1) * size)
      cell:SetColorTexture(floor(value / 4) % 2, floor(value / 2) % 2, value % 2, 1)
      cell:Show()
    end
  end
  for i = used + 1, #frame.cells do
    frame.cells[i]:Hide()
  end
end

function T3:ShootChunk()
  local shot = self.shot
  self:DrawChunk(shot.chunks[shot.index])
  if not shot.cvars and GetCVar and SetCVar then
    -- JPEG at full quality: small files the bridge reads quickly.
    shot.cvars = { format = GetCVar("screenshotFormat"), quality = GetCVar("screenshotQuality") }
    SetCVar("screenshotFormat", "jpeg")
    SetCVar("screenshotQuality", "10")
  end
  shot.deadline = GetTime() + SHOT_TIMEOUT
  local frame = self.shotFrame
  frame:Show()
  -- Take it on the next frame, once the grid has been drawn.
  frame:SetScript("OnUpdate", function(f)
    f:SetScript("OnUpdate", nil)
    Screenshot()
  end)
end

function T3:EndShot()
  local shot = self.shot
  self.shot = nil
  if self.shotFrame then
    self.shotFrame:Hide()
    self.shotFrame:SetScript("OnUpdate", nil)
  end
  if shot and shot.cvars then
    SetCVar("screenshotFormat", shot.cvars.format or "jpeg")
    SetCVar("screenshotQuality", shot.cvars.quality or "3")
  end
  return shot
end

function T3:OnShot(ok)
  local shot = self.shot
  if not shot then
    -- The player's own screenshot.
    return
  end
  self.shotFrame:Hide()
  -- Skip the "Screen captured" message for these.
  if ActionStatus and ActionStatus.Hide then
    C_Timer.After(0, function()
      ActionStatus:Hide()
    end)
  end
  if ok and shot.index < #shot.chunks then
    shot.index = shot.index + 1
    self:ShootChunk()
    return
  end
  self:EndShot()
  if not ok then
    self:FastSendFailed("The screenshot failed")
    return
  end
  local now = GetTime()
  for _, seq in ipairs(shot.seqs) do
    self.shotAt[seq] = self.shotAt[seq] or now
  end
  -- Look for the acknowledgement soon.
  self.lastPoll = now - self:PollInterval() + 2
  if shot.again then
    self:FastSend()
  end
  self:Fire("DATA")
end

function T3:FastSendFailed(reason)
  self.fastSendBroken = true
  self.shotAt = {}
  self:Print(reason .. "; sending with a reload instead.")
  if T3WowDB.settings.autoReload then
    self:Sync(true)
  else
    self:Print("Press Sync to send.")
  end
  self:Fire("DATA")
end

-- Called every second: give up on stuck screenshots and unanswered sends.
function T3:CheckFastSend()
  local now = GetTime()
  if self.shot and now > (self.shot.deadline or now) then
    self:EndShot()
    self:FastSendFailed("Could not take the screenshot")
    return
  end
  local waiting = {}
  for _, command in ipairs(T3WowDB.outbox) do
    local at = self.shotAt[command.seq]
    if at then
      waiting[command.seq] = at
      if now - at > ACK_TIMEOUT then
        self:FastSendFailed("The bridge did not pick up the message")
        return
      end
    end
  end
  -- Acknowledged commands left the outbox.
  self.shotAt = waiting
end

-- Sync for a thread the bridge only previews: tell it which thread is open.
function T3:FastFollow()
  local now = GetTime()
  if now - (self.lastFollowShot or -FOLLOW_EVERY) < FOLLOW_EVERY then
    return true
  end
  if not self:FastSend() then
    return false
  end
  self.lastFollowShot = now
  return true
end
