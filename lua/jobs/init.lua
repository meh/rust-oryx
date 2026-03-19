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
--     job:start()           -- or job:start({ key = "value" })
--     job:progress(50, 100) -- or job:progress(50, 100, { key = "value" })
--     job:finish()          -- or job:finish(0, 3000, { key = "value" })
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
            if v ~= math.floor(v) then
                entries[k] = GLib.Variant("d", v)
            elseif v < -2147483648 or v > 2147483647 then
                entries[k] = GLib.Variant("x", v)
            else
                entries[k] = GLib.Variant("i", v)
            end
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
    end, "StateChanged")
    _connected = true
end

-- ── Job object ────────────────────────────────────────────────────────────────

local Job = {}
Job.__index = Job

--- @param metadata table<string, string|number|boolean>?  optional extra metadata for signal listeners
Job.start = a.wrap(function(self, metadata, cb)
    get_proxy():StartAsync(function(_, _, _, _) cb() end, nil, self.id, make_metadata(metadata))
end, 3)

--- @param metadata table<string, string|number|boolean>?  optional extra metadata for signal listeners
Job.progress = a.wrap(function(self, current, total, metadata, cb)
    get_proxy():ProgressAsync(function(_, _, _, _) cb() end, nil, self.id, current, total, make_metadata(metadata))
end, 5)

--- @param metadata table<string, string|number|boolean>?  optional extra metadata for signal listeners
Job.stage = a.wrap(function(self, name, metadata, cb)
    get_proxy():StageAsync(function(_, _, _, _) cb() end, nil, self.id, name, make_metadata(metadata))
end, 4)

--- @param value string|number|boolean|nil  finish status matched against config colors
--- @param timeout_ms integer?  ms after which the slot auto-clears; -1 (default) waits for key press
--- @param metadata table<string, string|number|boolean>?  optional extra metadata for signal listeners
Job.finish = a.wrap(function(self, value, timeout_ms, metadata, cb)
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
    get_proxy():FinishAsync(function(_, _, _, _) cb() end, nil, self.id, v, timeout_ms or -1, make_metadata(metadata))
end, 5)

--- Return the job's current state and metadata.
--- @return table?  { state: string, state_metadata: table, metadata: table }, or nil on error
Job.get = a.wrap(function(self, cb)
    local p = get_proxy()
    if not p then return cb(nil) end
    p:GetJobAsync(function(_, _, result, err)
        if err or not result then return cb(nil) end
        cb({
            state = result[1],
            state_metadata = strip_meta(result[2]),
            metadata = strip_meta(result[3]),
        })
    end, nil, self.id)
end, 2)

--- Merge key-value pairs into the job's creation metadata.
--- Existing keys are overwritten; keys not in `updates` are preserved.
--- Emits the MetadataChanged signal with the full metadata after merging.
--- @param updates table<string, string|number|boolean>  key-value pairs to merge
Job.update = a.wrap(function(self, updates, cb)
    get_proxy():UpdateJobAsync(function(_, _, _, _) cb() end, nil, self.id, make_metadata(updates))
end, 3)

--- Enter prompt state: the slot LED breathes until the user taps (accept) or
--- holds (reject) the key. Async — call from within a plenary.async context
--- (a.void / a.run) and the result is returned directly (no callback needed).
---
--- The DBus Prompt() call returns immediately. The result arrives via the
--- State signal with state="prompt_resolved" and metadata.accepted=bool.
--- @param question string  prompt question (stored as metadata, visible via signals)
--- @param metadata table<string, string|number|boolean>?  optional extra metadata for signal listeners
--- @return boolean  true if the user accepted (tap), false if rejected (hold)
Job.prompt = a.wrap(function(self, question, metadata, cb)
    local job_id = self.id

    -- Register a one-shot listener on the State signal for this job_id.
    local function on_state(sid, state_str, meta)
        if sid == job_id and state_str == "prompt_resolved" then
            -- Unsubscribe ourselves immediately.
            for i, c in ipairs(_callbacks) do
                if c == on_state then
                    table.remove(_callbacks, i)
                    break
                end
            end
            cb(meta.accepted == true)
        end
    end
    table.insert(_callbacks, on_state)
    ensure_signal()

    -- Fire-and-forget: Prompt() returns () now, no result to wait for.
    get_proxy():PromptAsync(function(_, _, _, _) end, nil, job_id, question, make_metadata(metadata))
end, 4)

--- Resolve a pending prompt externally without keyboard input.
--- Safe to call even if the prompt was already resolved by the keyboard (no-op).
--- @param accepted boolean  true to accept, false to reject
--- @param metadata table<string, string|number|boolean>?  optional extra metadata for signal listeners
Job.prompt_resolve = a.wrap(function(self, accepted, metadata, cb)
    get_proxy():PromptResolveAsync(function(_, _, _, _) cb() end, nil, self.id, accepted == true, make_metadata(metadata))
end, 4)

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

--- Return all active jobs.
--- @return table<number, table>?  map from job_id to { state, state_metadata, metadata }, or nil
M.get_jobs = a.wrap(function(cb)
    local p = get_proxy()
    if not p then return cb(nil) end
    p:GetJobsAsync(function(_, _, result, err)
        if err or not result then return cb(nil) end
        local out = {}
        for id, tuple in pairs(result) do
            out[id] = {
                state = tuple[1],
                state_metadata = strip_meta(tuple[2]),
                metadata = strip_meta(tuple[3]),
            }
        end
        cb(out)
    end, nil)
end, 1)

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

-- ── MetadataChanged signal ────────────────────────────────────────────────────

local _metadata_callbacks = {}
local _metadata_connected = false

local function ensure_metadata_signal()
    if _metadata_connected then return end
    local p = get_proxy()
    if not p then return end
    p:connect_signal(function(_, job_id, raw_meta)
        local meta = strip_meta(raw_meta)
        vim.schedule(function()
            for _, cb in ipairs(_metadata_callbacks) do
                pcall(cb, job_id, meta)
            end
        end)
    end, "MetadataChanged")
    _metadata_connected = true
end

--- Subscribe to metadata changes for all jobs.
--- @param callback fun(job_id: number, metadata: table)
function M.on_metadata(callback)
    table.insert(_metadata_callbacks, callback)
    ensure_timer()
    ensure_metadata_signal()
end

--- Unsubscribe a previously registered metadata callback.
--- @param callback fun(job_id: number, metadata: table)
function M.off_metadata(callback)
    for i, cb in ipairs(_metadata_callbacks) do
        if cb == callback then
            table.remove(_metadata_callbacks, i)
            return
        end
    end
end

return M
