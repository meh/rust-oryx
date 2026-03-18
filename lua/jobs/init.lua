-- jobs/init.lua — Neovim interface to the zsa.oryx.Jobs DBus service.
--
-- Requires the `dbus_proxy` luarocks package:
--   luarocks install dbus_proxy
--
-- Add the parent directory to runtimepath, then:
--   local jobs = require("jobs")
--   local a    = require("plenary.async")
--
-- All Job methods are async and must be called from within an a.void / a.run
-- coroutine context:
--
--   a.void(function()
--     local job = jobs.create({ name = "build", source = "terminal" })
--     job:start()
--     job:progress(50, 100)
--     job:finish()
--   end)()
--
--   a.void(function()
--     local accepted = job:prompt("allow this?")
--     ...
--   end)()

local a          = require("plenary.async")
local dbus_proxy = require("dbus_proxy")
local GLib       = require("lgi").GLib

-- ── internals ─────────────────────────────────────────────────────────────────

local DEST  = "zsa.oryx.Jobs"
local PATH  = "/zsa/oryx/Jobs"
local IFACE = "zsa.oryx.Jobs"

local _proxy     = nil
local _callbacks = {}
local _timer     = nil
local _connected = false

-- Start pumping the GLib main context from Neovim's libuv loop so that
-- DBus async call callbacks and signal callbacks are processed promptly.
local function ensure_timer()
    if _timer then return end
    local ctx = GLib.MainContext.default()
    _timer = vim.uv.new_timer()
    _timer:start(0, 20, function()
        ctx:iteration(false)
    end)
end

local function get_proxy()
    if _proxy then return _proxy end
    local ok, p = pcall(dbus_proxy.Proxy.new, dbus_proxy.Proxy, {
        bus       = dbus_proxy.Bus.SESSION,
        name      = DEST,
        interface = IFACE,
        path      = PATH,
    })
    if not ok or not p then
        vim.notify("[jobs] could not connect to " .. DEST, vim.log.levels.ERROR)
        return nil
    end
    _proxy = p
    -- Ensure the GLib main context is being pumped so that async DBus call
    -- callbacks and signal callbacks are processed promptly.
    ensure_timer()
    return _proxy
end

-- Build a plain Lua table of GLib.Variant values from a flat Lua table.
-- dbus_proxy reads the `a{sv}` signature from DBus introspection and passes
-- this table to lgi's variant builder, which calls Variant.new_variant() on
-- each value — so each value must be a GLib.Variant, but the outer a{sv}
-- wrapper must NOT be pre-built (that would cause double-encoding).
-- Supported value types: string, number (integer or float), boolean.
-- Note: math.type() is Lua 5.3+; LuaJIT (Neovim) is 5.1-based, so floats
-- are detected via v ~= math.floor(v) instead.
local function make_metadata(t)
    local entries = {}
    for k, v in pairs(t or {}) do
        local tv = type(v)
        if tv == "string" then
            entries[k] = GLib.Variant("s", v)
        elseif tv == "number" then
            entries[k] = GLib.Variant(v ~= math.floor(v) and "d" or "i", v)
        elseif tv == "boolean" then
            entries[k] = GLib.Variant("b", v)
        end
    end
    return entries
end

-- Unwrap a GLib a{sv} variant into a plain Lua table.
local function strip_meta(raw)
    if not raw then return {} end
    return dbus_proxy.variant.strip(raw)
end

-- Connect the State signal once, routing to all registered callbacks.
local function ensure_signal()
    if _connected then return end
    local p = get_proxy()
    if not p then return end
    p:connect_signal(function(_, job_id, state_str, raw_meta)
        local meta = strip_meta(raw_meta)
        vim.schedule(function()
            for _, cb in ipairs(_callbacks) do
                pcall(cb, job_id, state_str, meta)
            end
        end)
    end, "State")
    _connected = true
end

-- ── Job object ────────────────────────────────────────────────────────────────

local Job = {}
Job.__index = Job

Job.start = a.wrap(function(self, cb)
    get_proxy():StartAsync(function(_, _, _, _) cb() end, nil, self.id)
end, 2)

Job.progress = a.wrap(function(self, current, total, cb)
    get_proxy():ProgressAsync(function(_, _, _, _) cb() end, nil, self.id, current, total)
end, 4)

Job.stage = a.wrap(function(self, name, cb)
    get_proxy():StageAsync(function(_, _, _, _) cb() end, nil, self.id, name)
end, 3)

--- @param value string|number|boolean|nil  finish value matched against config colors
--- @param timeout_ms integer?  ms after which the slot auto-clears; -1 (default) waits for key press
Job.finish = a.wrap(function(self, value, timeout_ms, cb)
    local tv = type(value)
    local v
    if tv == "number" then
        v = GLib.Variant(value ~= math.floor(value) and "d" or "i", value)
    elseif tv == "string" then
        v = GLib.Variant("s", value)
    elseif tv == "boolean" then
        v = GLib.Variant("b", value)
    else
        v = GLib.Variant("b", true)
    end
    get_proxy():FinishAsync(function(_, _, _, _) cb() end, nil, self.id, v, timeout_ms or -1)
end, 4)

Job.get_state = a.wrap(function(self, cb)
    local p = get_proxy()
    if not p then return cb(nil) end
    p:GetStateAsync(function(_, _, result, err)
        if err or not result then return cb(nil) end
        cb({ state = result[1], metadata = strip_meta(result[2]) })
    end, nil, self.id)
end, 2)

--- Enter prompt state: the slot LED breathes until the user taps (accept) or
--- holds (reject) the key. Async — call from within a plenary.async context
--- (a.void / a.run) and the result is returned directly (no callback needed).
--- @param text string  prompt text (stored as metadata, visible via signals)
--- @return boolean  true if the user accepted (tap), false if rejected (hold)
Job.prompt = a.wrap(function(self, text, cb)
    get_proxy():PromptAsync(function(_, _, result, _)
        cb(result == true)
    end, nil, self.id, text)
end, 3)

--- Resolve a pending prompt externally without keyboard input.
--- Safe to call even if the prompt was already resolved by the keyboard (no-op).
--- @param accepted boolean  true to accept, false to reject
Job.prompt_resolve = a.wrap(function(self, accepted, cb)
    get_proxy():PromptResolveAsync(function(_, _, _, _) cb() end, nil, self.id, accepted == true)
end, 3)

-- ── Public API ────────────────────────────────────────────────────────────────

local M = {}

--- Create a new job. Waits until a slot is free (or timeout_ms elapses).
--- @param metadata table<string, string|number|boolean>?
--- @param slot integer?  preferred slot index (-1 = any, default)
--- @param timeout_ms integer?  ms to wait for a free slot (-1 = forever, default)
--- @return table?  Job object, or nil if service unavailable or timed out
M.create = a.wrap(function(metadata, slot, timeout_ms, cb)
    local p = get_proxy()
    if not p then return cb(nil) end
    p:CreateAsync(function(_, _, id, err)
        if err or not id then return cb(nil) end
        cb(setmetatable({ id = id }, Job))
    end, nil, make_metadata(metadata), slot or -1, timeout_ms or -1)
end, 4)

--- Subscribe to state changes for all jobs.
--- @param callback fun(job_id: number, state: string, metadata: table)
function M.on_state(callback)
    table.insert(_callbacks, callback)
    ensure_timer()
    ensure_signal()
end

--- Unsubscribe a previously registered callback.
--- @param callback fun(job_id: number, state: string, metadata: table)
function M.off_state(callback)
    for i, cb in ipairs(_callbacks) do
        if cb == callback then
            table.remove(_callbacks, i)
            return
        end
    end
end

return M
