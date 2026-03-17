-- jobs/init.lua — Neovim interface to the zsa.oryx.Jobs DBus service.
--
-- Requires the `dbus_proxy` luarocks package:
--   luarocks install dbus_proxy
--
-- Add the parent directory to runtimepath, then:
--   local jobs = require("jobs")
--   local job = jobs.create({ name = "build", source = "terminal" })
--   job:start()
--   job:progress(50, 100)
--   job:finish()

local dbus_proxy = require("dbus_proxy")
local GLib = require("lgi").GLib

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

-- Build a GLib.Variant of type a{sv} from a flat Lua table.
-- Supported value types: string, number (integer or float), boolean.
local function to_asv(t)
    local entries = {}
    for k, v in pairs(t or {}) do
        local tv = type(v)
        if tv == "string" then
            entries[k] = GLib.Variant("s", v)
        elseif tv == "number" then
            entries[k] = GLib.Variant(math.type(v) == "float" and "d" or "i", v)
        elseif tv == "boolean" then
            entries[k] = GLib.Variant("b", v)
        end
    end
    return GLib.Variant("a{sv}", entries)
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

function Job:finish(value)
    local tv = type(value)
    local v
    if tv == "number" then
        v = GLib.Variant(math.type(value) == "float" and "d" or "i", value)
    elseif tv == "string" then
        v = GLib.Variant("s", value)
    elseif tv == "boolean" then
        v = GLib.Variant("b", value)
    else
        v = GLib.Variant("b", true)
    end
    get_proxy():Finish(self.id, v)
end

function Job:get_state()
    local p = get_proxy()
    if not p then return nil end
    local state_str, raw_meta = p:GetState(self.id)
    return { state = state_str, metadata = strip_meta(raw_meta) }
end

--- Enter prompt state: the slot LED breathes until the user taps (accept) or
--- holds (reject) the key. This call blocks until the user responds, so it
--- must be run from a thread (vim.uv.new_thread or coroutine that yields).
--- @param text string  prompt text (stored as metadata, visible via signals)
--- @return boolean  true if the user accepted (tap), false if rejected (hold)
function Job:prompt_sync(text)
    local p = get_proxy()
    if not p then return false end
    return p:Prompt(self.id, text)
end

--- Async version of prompt: spawns a thread so Neovim's event loop is not
--- blocked, then calls `callback(accepted)` on the main thread.
--- @param text string
--- @param callback fun(accepted: boolean)
function Job:prompt(text, callback)
    local job_id = self.id
    -- Use vim.system with dbus-send as a simple async mechanism, but actually
    -- the cleaner approach is to use GLib async iteration: the proxy call will
    -- complete during a future GLib main context iteration pumped by our timer.
    -- However, dbus_proxy calls are synchronous and would block.
    -- So we use a Lua coroutine + vim.uv.new_work for true async.
    local work = vim.uv.new_work(
        -- Work function runs in a separate Lua state (thread).
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
        -- After function runs on the main thread.
        function(accepted)
            vim.schedule(function()
                callback(accepted)
            end)
        end
    )
    work:queue(job_id, text)
end

-- ── Public API ────────────────────────────────────────────────────────────────

local M = {}

--- Create a new job. Returns a Job object, or nil if the service is unavailable.
--- @param metadata table<string, string|number|boolean>?
--- @return table?
function M.create(metadata)
    local p = get_proxy()
    if not p then return nil end
    local id = p:Create(to_asv(metadata), -1, -1)
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
