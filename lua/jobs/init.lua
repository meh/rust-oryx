-- jobs/init.lua — Neovim interface to the zsa.oryx.Jobs DBus service.
--
-- Requires the `dbus_proxy` luarocks package:
--   luarocks install dbus_proxy
--
-- Add the parent directory to runtimepath, then:
--   local jobs = require("jobs")
--   local a    = require("plenary.async")
--   local job  = jobs.create({ name = "build", source = "terminal" })
--   job:start()
--   job:progress(50, 100)
--   job:finish()
--
-- For the blocking prompt, use inside an async function:
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

-- Start pumping the GLib main context from Neovim's libuv loop so that
-- DBus signal callbacks fire. Called lazily when the first on_state is registered.
local function ensure_timer()
    if _timer then return end
    local ctx = GLib.MainContext.default()
    _timer = vim.uv.new_timer()
    _timer:start(0, 20, function()
        ctx:iteration(false)
    end)
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

function Job:start()
    get_proxy():Start(self.id)
end

function Job:progress(current, total)
    get_proxy():Progress(self.id, current, total)
end

function Job:stage(name)
    get_proxy():Stage(self.id, name)
end

--- @param value string|number|boolean|nil  finish value matched against config colors
--- @param timeout_ms integer?  ms after which the slot auto-clears; -1 (default) waits for key press
function Job:finish(value, timeout_ms)
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
    get_proxy():Finish(self.id, v, timeout_ms or -1)
end

function Job:get_state()
    local p = get_proxy()
    if not p then return nil end
    local state_str, raw_meta = p:GetState(self.id)
    return { state = state_str, metadata = strip_meta(raw_meta) }
end

--- Enter prompt state: the slot LED breathes until the user taps (accept) or
--- holds (reject) the key. Async — call from within a plenary.async context
--- (a.void / a.run) and the result is returned directly (no callback needed).
--- @param text string  prompt text (stored as metadata, visible via signals)
--- @return boolean  true if the user accepted (tap), false if rejected (hold)
Job.prompt = a.wrap(function(self, text, callback)
    local job_id = self.id
    -- The DBus Prompt call blocks until the user responds on the keyboard.
    -- Offload it to a libuv thread-pool worker so Neovim's event loop is
    -- not blocked; resume the calling coroutine via the after-function.
    local work = vim.uv.new_work(
        -- Runs in a separate Lua state (no shared upvalues).
        function(id, prompt_text)
            local dp = require("dbus_proxy")
            local p = dp.Proxy:new({
                bus       = dp.Bus.SESSION,
                name      = "zsa.oryx.Jobs",
                interface = "zsa.oryx.Jobs",
                path      = "/zsa/oryx/Jobs",
            })
            if not p then return false end
            return p:Prompt(id, prompt_text)
        end,
        -- After-function runs on the main thread; resume the coroutine.
        function(accepted)
            vim.schedule(function()
                callback(accepted)
            end)
        end
    )
    work:queue(job_id, text)
end, 3)

-- ── Public API ────────────────────────────────────────────────────────────────

local M = {}

--- Create a new job. Returns a Job object, or nil if the service is unavailable.
--- @param metadata table<string, string|number|boolean>?
--- @return table?
function M.create(metadata)
    local p = get_proxy()
    if not p then return nil end
    local id = p:Create(make_metadata(metadata), -1, -1)
    return setmetatable({ id = id }, Job)
end

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
