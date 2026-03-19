-- lualine/components/jobs.lua — Lualine component for oryx-jobs.
--
-- Shows one icon per active job on the ZSA keyboard, colored by state, with
-- breathe/bounce animations mirroring the noctalia desktop bar widget.
--
-- Usage in lualine config:
--   lualine_x = {
--       {
--           "jobs",
--           show_empty = true,      -- show idle icon when no jobs (default: true)
--           separator  = " ",       -- between job icons (default: " ")
--           colors = { ... },       -- per-state color spec overrides
--           icons  = { ... },       -- per-state icon overrides
--       },
--   }
--
-- Requires the `jobs` module (../jobs/init.lua) on the runtimepath.

local component = require("lualine.component"):extend()
local a         = require("plenary.async")

-- ── Debug logging ────────────────────────────────────────────────────────────
-- Remove once things work.

local function log(msg)
    vim.schedule(function()
        vim.notify("[jobs-lualine] " .. msg, vim.log.levels.DEBUG)
    end)
end

log("module loaded")

-- ── Nerd Font icons ──────────────────────────────────────────────────────────

local STATE_ICONS = {
    created  = "\u{f0415}", -- nf-md-plus_circle_outline
    started  = "\u{f040a}", -- nf-md-play
    progress = true,        -- sentinel: use progress_icons below
    stage    = "\u{f018b}", -- nf-md-layers
    prompt   = "\u{f059}",  -- nf-fa-question_circle
    finished = "\u{f0132}", -- nf-md-check_circle_outline
}
local IDLE_ICON = "\u{f0765}" -- nf-md-circle_outline

-- Circle-slice glyphs for progress animation (0/8 .. 8/8).
local PROGRESS_ICONS = {
    "\u{f0a9e}", "\u{f0a9f}", "\u{f0aa0}", "\u{f0aa1}",
    "\u{f0aa2}", "\u{f0aa3}", "\u{f0aa4}", "\u{f0aa5}",
}

-- ── Default color specs (hardcoded fallback, matching noctalia manifest.json) ─

local DEFAULT_SPECS = {
    idle           = { type = "static",  color = "#555555" },
    created        = { type = "static",  color = "#0064FF" },
    started        = { type = "static",  color = "#0064FF" },
    progress_start = { type = "static",  color = "#0064FF" },
    progress_end   = { type = "static",  color = "#00FF64" },
    stage          = { type = "static",  color = "#FFC800" },
    prompt         = { type = "breathe", color = "#C800FF", period_ms = 1500 },
    prompt_accept  = { type = "static",  color = "#007A00" },
    prompt_reject  = { type = "static",  color = "#CC0000" },
    finished       = { type = "static",  color = "#B4B4B4" },
}

-- Map daemon GetColors keys to our internal spec keys.
local DAEMON_KEY_MAP = {
    ["idle"]             = "idle",
    ["started"]          = "started",
    ["progress-start"]   = "progress_start",
    ["progress-end"]     = "progress_end",
    ["stage-default"]    = "stage",
    ["prompt-waiting"]   = "prompt",
    ["prompt-accept"]    = "prompt_accept",
    ["prompt-reject"]    = "prompt_reject",
    ["finished-default"] = "finished",
}

-- Daemon color specs fetched via GetColors, mapped to our internal keys.
-- Populated asynchronously on startup.  nil until fetched.
local daemon_specs = nil

-- Map job state to the spec key used for color resolution.
local STATE_SPEC_KEY = {
    created  = "started", -- created uses the same color as started
    started  = "started",
    stage    = "stage",
    prompt   = "prompt",
    finished = "finished",
}

-- ── Color math ───────────────────────────────────────────────────────────────

local function parse_hex(hex)
    hex = (hex or "#888888"):gsub("^#", "")
    return {
        tonumber(hex:sub(1, 2), 16) or 0,
        tonumber(hex:sub(3, 4), 16) or 0,
        tonumber(hex:sub(5, 6), 16) or 0,
    }
end

local function rgb_to_hex(r, g, b)
    return string.format("#%02x%02x%02x",
        math.max(0, math.min(255, math.floor(r + 0.5))),
        math.max(0, math.min(255, math.floor(g + 0.5))),
        math.max(0, math.min(255, math.floor(b + 0.5))))
end

local function lerp_color(hex_a, hex_b, t)
    local a_rgb = parse_hex(hex_a)
    local b_rgb = parse_hex(hex_b)
    return rgb_to_hex(
        a_rgb[1] + (b_rgb[1] - a_rgb[1]) * t,
        a_rgb[2] + (b_rgb[2] - a_rgb[2]) * t,
        a_rgb[3] + (b_rgb[3] - a_rgb[3]) * t)
end

--- Apply an opacity-like effect by blending toward black.
--- @param hex string  base color
--- @param opacity number  0.0..1.0
--- @return string  blended hex color
local function apply_opacity(hex, opacity)
    local rgb = parse_hex(hex)
    return rgb_to_hex(rgb[1] * opacity, rgb[2] * opacity, rgb[3] * opacity)
end

-- ── Animation math ───────────────────────────────────────────────────────────

--- Compute animation opacity for a spec at the given elapsed time.
--- Mirrors noctalia Main.qml animOpacity().
--- @param spec table  { type, color, period_ms? }
--- @param elapsed_ms number
--- @return number  opacity 0.15..1.0 (or 1.0 for static)
local function anim_opacity(spec, elapsed_ms)
    if not spec or spec.type == "static" then
        return 1.0
    end

    local period = spec.period_ms
        or (spec.type == "breathe" and 1500 or 2000)
    local t = (elapsed_ms % period) / period

    if spec.type == "breathe" then
        local phase = t * 2.0 * math.pi
        return 0.15 + 0.85 * ((math.sin(phase) + 1.0) / 2.0)
    end

    if spec.type == "bounce" then
        local pos = t < 0.5 and (t * 2.0) or ((1.0 - t) * 2.0)
        return 0.15 + 0.85 * pos
    end

    return 1.0
end

-- ── Module state (shared across all component instances) ─────────────────────

-- Job cache: job_id -> { state, state_metadata, metadata }
-- Updated reactively via on_state / on_metadata signals.
local jobs_cache = {}

-- Animation state
local anim_elapsed_ms = 0
local anim_timer      = nil
local ANIM_INTERVAL   = 30 -- ms, matching noctalia

-- Signal subscription tracking
local subscribed     = false
local state_callback = nil
local meta_callback  = nil

-- Active component instances (for triggering refresh)
local instances = {}

-- ── Helpers ──────────────────────────────────────────────────────────────────

--- Resolve the color spec for a job state.
--- Fallback chain: user options -> daemon colors -> hardcoded defaults.
--- Mirrors noctalia's pluginSettings -> daemonColors -> manifest defaults.
--- @param state string
--- @param user_specs table?
--- @return table  color spec { type, color, period_ms? }
local function spec_for_state(state, user_specs)
    local key = STATE_SPEC_KEY[state]
    if not key then return DEFAULT_SPECS.idle end

    -- 1. User overrides (highest priority)
    if user_specs and user_specs[key] then
        return user_specs[key]
    end

    -- 2. Daemon colors (fetched via GetColors)
    if daemon_specs and daemon_specs[key] then
        return daemon_specs[key]
    end

    -- 3. Hardcoded defaults
    return DEFAULT_SPECS[key] or DEFAULT_SPECS.idle
end

--- Resolve a spec by key directly (for progress_start/end, prompt_accept/reject, idle).
--- Same three-level fallback chain.
--- @param key string
--- @param user_specs table?
--- @return table
local function spec_for_key(key, user_specs)
    if user_specs and user_specs[key] then
        return user_specs[key]
    end
    if daemon_specs and daemon_specs[key] then
        return daemon_specs[key]
    end
    return DEFAULT_SPECS[key] or DEFAULT_SPECS.idle
end

--- Check whether any cached job has an animated color spec.
local function has_animated_jobs(user_specs)
    for _, info in pairs(jobs_cache) do
        local spec = spec_for_state(info.state, user_specs)
        if spec and spec.type ~= "static" then
            return true
        end
    end
    return false
end

--- Resolve the current display color for a job, including animation.
--- @param info table  { state, state_metadata, metadata }
--- @param user_specs table?
--- @return string  hex color
local function resolve_color(info, user_specs)
    local state = info.state

    -- Progress: lerp between start and end colors.
    if state == "progress" then
        local sm = info.state_metadata or {}
        local total = tonumber(sm.total) or 0
        local current = tonumber(sm.current) or 0
        local t = total > 0 and math.max(0, math.min(1, current / total)) or 0
        local start_spec = spec_for_key("progress_start", user_specs)
        local end_spec   = spec_for_key("progress_end", user_specs)
        return lerp_color(start_spec.color, end_spec.color, t)
    end

    -- Prompt resolved: brief flash of accept/reject color.
    if state == "prompt_resolved" then
        local sm = info.state_metadata or {}
        if sm.accepted then
            return spec_for_key("prompt_accept", user_specs).color
        else
            return spec_for_key("prompt_reject", user_specs).color
        end
    end

    local spec = spec_for_state(state, user_specs)
    local opacity = anim_opacity(spec, anim_elapsed_ms)
    return apply_opacity(spec.color, opacity)
end

--- Resolve the icon for a job.
--- @param info table  { state, state_metadata, metadata }
--- @param user_icons table?
--- @return string  icon character
local function resolve_icon(info, user_icons)
    local state = info.state

    -- User icon override
    if user_icons and user_icons[state] then
        return user_icons[state]
    end

    -- Progress: cycle through slice glyphs based on fraction.
    if state == "progress" then
        local sm = info.state_metadata or {}
        local total = tonumber(sm.total) or 0
        local current = tonumber(sm.current) or 0
        if total > 0 then
            local frac = math.max(0, math.min(1, current / total))
            local idx = math.floor(frac * (#PROGRESS_ICONS - 1)) + 1
            return PROGRESS_ICONS[idx]
        end
        return PROGRESS_ICONS[1]
    end

    -- Prompt resolved: show accept/reject icon briefly.
    if state == "prompt_resolved" then
        local sm = info.state_metadata or {}
        if sm.accepted then
            return "\u{f0132}" -- nf-md-check_circle_outline
        else
            return "\u{f0156}" -- nf-md-close_circle_outline
        end
    end

    local icon = STATE_ICONS[state]
    if type(icon) == "string" then
        return icon
    end

    return IDLE_ICON
end

-- ── Animation timer ──────────────────────────────────────────────────────────

local function stop_anim_timer()
    if anim_timer then
        anim_timer:stop()
        anim_timer:close()
        anim_timer = nil
    end
end

local function start_anim_timer()
    if anim_timer then return end
    anim_timer = vim.uv.new_timer()
    anim_timer:start(0, ANIM_INTERVAL, vim.schedule_wrap(function()
        anim_elapsed_ms = anim_elapsed_ms + ANIM_INTERVAL
        -- Trigger lualine refresh so the component re-renders with new colors.
        local ok, lualine = pcall(require, "lualine")
        if ok then lualine.refresh() end
    end))
end

--- Ensure the animation timer is running if needed, stopped if not.
local function sync_anim_timer(user_specs)
    if has_animated_jobs(user_specs) then
        start_anim_timer()
    else
        stop_anim_timer()
    end
end

-- ── Signal subscriptions ─────────────────────────────────────────────────────

local function schedule_refresh()
    vim.schedule(function()
        local ok, lualine = pcall(require, "lualine")
        if ok then lualine.refresh() end
    end)
end

local function subscribe(user_specs)
    if subscribed then return end
    local ok, jobs = pcall(require, "jobs")
    if not ok then
        log("subscribe: require('jobs') failed: " .. tostring(jobs))
        return
    end
    log("subscribe: require('jobs') ok")

    state_callback = function(job_id, state_str, meta)
        log("on_state: job=" .. tostring(job_id) .. " state=" .. tostring(state_str))
        if state_str == "cleared" then
            jobs_cache[job_id] = nil
        else
            local entry = jobs_cache[job_id]
            if entry then
                entry.state = state_str
                entry.state_metadata = meta
            else
                jobs_cache[job_id] = {
                    state = state_str,
                    state_metadata = meta,
                    metadata = {},
                }
            end
        end
        sync_anim_timer(user_specs)
        schedule_refresh()
    end

    meta_callback = function(job_id, meta)
        local entry = jobs_cache[job_id]
        if entry then
            entry.metadata = meta
            schedule_refresh()
        end
    end

    jobs.on_state(state_callback)
    jobs.on_metadata(meta_callback)
    subscribed = true
    log("subscribe: signals connected")

    -- Seed the cache and fetch daemon colors.
    a.void(function()
        -- Fetch daemon color config (like noctalia's fetchDaemonColors).
        log("subscribe: fetching daemon colors via get_colors()")
        local colors = jobs.get_colors()
        if colors then
            daemon_specs = {}
            for daemon_key, spec in pairs(colors) do
                local our_key = DAEMON_KEY_MAP[daemon_key]
                if our_key then
                    daemon_specs[our_key] = spec
                end
            end
            log("subscribe: daemon colors loaded (" .. vim.tbl_count(daemon_specs) .. " specs)")
        else
            log("subscribe: get_colors() returned nil (using defaults)")
        end

        -- Seed job cache.
        log("subscribe: seeding cache via get_jobs()")
        local all = jobs.get_jobs()
        if all then
            local count = 0
            for id, info in pairs(all) do
                if not jobs_cache[id] then
                    jobs_cache[id] = info
                    count = count + 1
                end
            end
            log("subscribe: seeded " .. count .. " jobs")
            sync_anim_timer(user_specs)
            schedule_refresh()
        else
            log("subscribe: get_jobs() returned nil")
        end
    end)()
end

-- ── Lualine component ────────────────────────────────────────────────────────

-- We use lualine's create_hl / format_hl API so that highlight groups
-- properly inherit the section's background and clear decorations like
-- underline.  Each "slot" (idle, plus up to N concurrent jobs) gets a
-- highlight created with a dynamic color function that returns the current
-- animated fg color on every render.

-- Per-slot color: set by update_status, read by the color function closure.
-- Indexed by slot name (string).
local slot_colors = {}

--- Build a lualine highlight for a named slot whose fg changes every frame.
--- @param self_ table  component instance (for create_hl / format_hl)
--- @param slot string  unique slot name
local function ensure_slot_hl(self_, slot)
    if self_._slot_hls[slot] then return end
    self_._slot_hls[slot] = self_:create_hl(function()
        return { fg = slot_colors[slot] or "#888888" }
    end, "job_" .. slot)
end

function component:init(options)
    component.super.init(self, options)
    log("init: options=" .. vim.inspect(options))

    self.user_specs  = options.colors or {}
    self.user_icons  = options.icons  or {}
    self.show_empty  = options.show_empty ~= false -- default true
    self.icon_sep    = options.separator or " "
    self._slot_hls   = {} -- slot_name -> hl token from create_hl

    log("init: show_empty=" .. tostring(self.show_empty))
    instances[self] = true
    subscribe(self.user_specs)

    -- Recreate slot highlights on colorscheme change (they get wiped).
    self._augroup = vim.api.nvim_create_augroup("OryxJobsLualine_" .. tostring(self):sub(-8), { clear = true })
    vim.api.nvim_create_autocmd("ColorScheme", {
        group = self._augroup,
        callback = function()
            self._slot_hls = {}
            schedule_refresh()
        end,
    })
end

--- Build sorted list of jobs for deterministic display order.
--- Sorts by job ID (ascending) so the order is stable.
local function sorted_jobs()
    local list = {}
    for id, info in pairs(jobs_cache) do
        list[#list + 1] = { id = id, info = info }
    end
    table.sort(list, function(a, b) return a.id < b.id end)
    return list
end

local _update_count = 0
function component:update_status()
    local jobs_list = sorted_jobs()

    -- Log sparingly (every 50th call) to avoid spam.
    _update_count = _update_count + 1
    if _update_count <= 3 or _update_count % 50 == 0 then
        log("update_status[" .. _update_count .. "]: " .. #jobs_list .. " jobs, show_empty=" .. tostring(self.show_empty))
    end

    -- No active jobs: show idle icon or nothing.
    if #jobs_list == 0 then
        if self.show_empty then
            local idle_spec = spec_for_key("idle", self.user_specs)
            slot_colors["idle"] = idle_spec.color
            ensure_slot_hl(self, "idle")
            local hl = self:format_hl(self._slot_hls["idle"])
            local result = hl .. IDLE_ICON
            if _update_count <= 3 then
                log("update_status: returning idle: " .. result)
            end
            return result
        end
        return ""
    end

    local parts = {}
    for _, entry in ipairs(jobs_list) do
        local info  = entry.info
        local color = resolve_color(info, self.user_specs)
        local icon  = resolve_icon(info, self.user_icons)

        -- Use job_id as the slot key so each job gets its own highlight.
        local slot = tostring(entry.id)
        slot_colors[slot] = color
        ensure_slot_hl(self, slot)
        local hl = self:format_hl(self._slot_hls[slot])
        parts[#parts + 1] = hl .. icon
    end

    local result = table.concat(parts, self.icon_sep)
    if _update_count <= 3 then
        log("update_status: returning: " .. result)
    end
    return result
end

return component
