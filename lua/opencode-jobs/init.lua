-- opencode-jobs/init.lua — Maps opencode.nvim SSE events to oryx-jobs keyboard LEDs.
--
-- Tracks opencode sessions as jobs on the ZSA keyboard, showing LED feedback
-- for session activity, tool usage stages, and permission prompts.
--
-- Usage:
--   require("opencode-jobs").setup()
--
-- The opencode.nvim plugin must already be set up and emitting OpencodeEvent:*
-- autocmds. This module also requires the `jobs` module (../jobs/init.lua) on
-- the runtimepath.

local a    = require("plenary.async")
local jobs = require("jobs")

local M = {}

-- How long (ms) the finished LED stays lit before auto-clearing.
local FINISH_OK_TIMEOUT_MS    = 3000
local FINISH_ERR_TIMEOUT_MS   = 5000
local FINISH_DELETE_TIMEOUT_MS = 2000

local active  = {} -- session_id -> { job = Job, tool_count = number, pending_permissions = { [perm_id] = function } }
local augroup = nil

-- ── logging ───────────────────────────────────────────────────────────────────

local function log(msg)
    -- vim.notify("[opencode_jobs] " .. msg, vim.log.levels.INFO)
end

local function warn(msg)
    vim.notify("[opencode_jobs] " .. msg, vim.log.levels.WARN)
end

--- Truncate a session ID to 8 chars for readable log lines.
local function sid_short(sid)
    return tostring(sid):sub(1, 8)
end

-- ── metadata helpers ──────────────────────────────────────────────────────────

--- Build a flat metadata table from the session.status `status` sub-object.
--- Only includes non-nil scalar fields (message, attempt, next).
--- @param status table?
--- @return table<string, string|number|boolean>
local function status_meta(status)
    if not status then return {} end
    local m = {}
    if status.message ~= nil then m.status_message = tostring(status.message) end
    if status.attempt ~= nil then m.status_attempt = status.attempt end
    if status.next    ~= nil then m.status_next    = status.next end
    return m
end

--- Flatten a message part into a metadata table with dot-notation keys.
--- @param part table
--- @return table<string, string|number|boolean>
local function part_meta(part)
    local m = {}
    if part.type      ~= nil then m.part_type  = tostring(part.type) end
    if part.id        ~= nil then m.part_id    = tostring(part.id) end
    if part.sessionID ~= nil then m.sessionID  = tostring(part.sessionID) end
    if part.messageID ~= nil then m.messageID  = tostring(part.messageID) end
    if part.callID    ~= nil then m.callID     = tostring(part.callID) end
    if part.name      ~= nil then m.part_name  = tostring(part.name) end
    if part.synthetic ~= nil then m.synthetic  = part.synthetic end

    -- Tool info (may be a string or a table with .name).
    local tool = part.tool
    if type(tool) == "table" then
        if tool.name ~= nil then m.tool_name = tostring(tool.name) end
    elseif type(tool) == "string" then
        m.tool_name = tool
    end

    -- State sub-object.
    local state = part.state
    if type(state) == "table" then
        if state.status ~= nil then m.state_status = tostring(state.status) end
        if state.title  ~= nil then m.state_title  = tostring(state.title) end
    elseif type(state) == "string" then
        m.state_status = state
    end

    return m
end

--- Flatten a permission properties table into metadata.
--- @param props table
--- @return table<string, string|number|boolean>
local function permission_meta(props)
    local m = {}
    if props.id        ~= nil then m.perm_id   = tostring(props.id) end
    if props.type      ~= nil then m.perm_type  = tostring(props.type) end
    if props.title     ~= nil then m.perm_title = tostring(props.title) end
    if props.messageID ~= nil then m.messageID  = tostring(props.messageID) end
    if props.callID    ~= nil then m.callID     = tostring(props.callID) end

    -- Pattern may be a string or array of strings.
    local pat = props.pattern
    if type(pat) == "table" then
        m.pattern = table.concat(pat, ", ")
    elseif pat ~= nil then
        m.pattern = tostring(pat)
    end

    return m
end

--- Flatten an error table into metadata.
--- Falls back to json-encoding if the error is a table.
--- @param err any
--- @return table<string, string|number|boolean>
local function error_meta(err)
    if err == nil then return {} end
    if type(err) == "string" then
        return { error = err }
    elseif type(err) == "table" then
        local ok, json = pcall(vim.json.encode, err)
        return { error = ok and json or tostring(err) }
    else
        return { error = tostring(err) }
    end
end

--- Flatten permission.replied properties into metadata.
--- @param props table
--- @return table<string, string|number|boolean>
local function permission_replied_meta(props)
    local m = {}
    if props.sessionID    ~= nil then m.sessionID    = tostring(props.sessionID) end
    if props.permissionID ~= nil then m.permissionID = tostring(props.permissionID) end
    if props.requestID    ~= nil then m.requestID    = tostring(props.requestID) end
    if props.response     ~= nil then m.response     = tostring(props.response) end
    return m
end

-- ── internals ─────────────────────────────────────────────────────────────────

--- Get or create a job for the given session, starting it immediately.
--- Must be called from within an a.void / a.run coroutine context.
--- @param session_id string
--- @param metadata table?  optional metadata forwarded to job:start()
--- @return table? job
local function get_or_create(session_id, metadata)
    local entry = active[session_id]
    if entry then return entry.job end

    -- Reserve the slot immediately before yielding to jobs.create, so that a
    -- second busy event arriving while CreateAsync is in flight sees the
    -- sentinel and bails rather than creating a second job.
    active[session_id] = { job = nil, tool_count = 0, pending_permissions = {} }

    local job = jobs.create({ name = "opencode", session_id = session_id })
    if not job then
        active[session_id] = nil
        warn("session " .. sid_short(session_id) .. ": failed to create job (service unavailable?)")
        return nil
    end
    job:start(metadata)
    active[session_id].job = job
    log("session " .. sid_short(session_id) .. ": job " .. tostring(job.id) .. " created")
    return job
end

--- Finish and clean up the active entry for a session.
--- Must be called from within an a.void / a.run coroutine context.
--- @param session_id string
--- @param value integer  0 = ok, 1 = error
--- @param timeout_ms integer  ms before auto-clear
--- @param reason string  log label
--- @param metadata table?  optional metadata forwarded to job:finish()
local function finish_session(session_id, value, timeout_ms, reason, metadata)
    local entry = active[session_id]
    if not entry or not entry.job then return end
    if value == 0 then
        log("session " .. sid_short(session_id) .. ": " .. reason)
    else
        warn("session " .. sid_short(session_id) .. ": " .. reason)
    end
    active[session_id] = nil
    entry.job:finish(value, timeout_ms, metadata)
end

-- ── setup / teardown ──────────────────────────────────────────────────────────

function M.setup()
    if augroup then return end -- already set up
    augroup = vim.api.nvim_create_augroup("OryxOpencode", { clear = true })

    -- session.status { type = "busy" }  → create + start the job.
    -- session.status { type = "idle" }  → finish the job (LLM done thinking).
    -- session.status { type = "retry" } → stage the job as "retry".
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:session.status",
        callback = a.void(function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local sid = props.sessionID
            if not sid then return end
            local status_type = props.status and props.status.type
            local meta = status_meta(props.status)
            if status_type == "busy" then
                get_or_create(sid, meta)
            elseif status_type == "idle" then
                finish_session(sid, 0, FINISH_OK_TIMEOUT_MS, "idle → finish", meta)
            elseif status_type == "retry" then
                local entry = active[sid]
                if entry and entry.job then
                    entry.job:stage("retry", meta)
                    log("session " .. sid_short(sid) .. ": retry → stage")
                end
            end
        end),
    })

    -- Message part updated: show activity for all part types as stages.
    -- Tool parts are ref-counted so the LED returns to "started" only when
    -- every concurrent tool call has completed.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:message.part.updated",
        callback = a.void(function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local part = props.part or props
            local sid = part.sessionID
            if not sid then return end
            local entry = active[sid]
            if not entry or not entry.job then return end

            local meta = part_meta(part)
            local part_type = part.type or "unknown"

            -- Derive a human-readable stage name.
            local stage_name
            if part_type == "tool" then
                stage_name = meta.tool_name or meta.part_name or "tool"
            else
                stage_name = part_type
            end

            local state = part.state
            local status = state and (type(state) == "table" and state.status or state)

            if status == "completed" or status == "error" or status == "failed" then
                if part_type == "tool" then
                    entry.tool_count = math.max(0, entry.tool_count - 1)
                    if entry.tool_count == 0 then
                        entry.job:start(meta)
                        log("session " .. sid_short(sid) .. ": tool done → started (active: 0)")
                    else
                        log("session " .. sid_short(sid) .. ": tool done (active: " .. entry.tool_count .. ")")
                    end
                else
                    -- Non-tool parts finishing don't affect the ref-count.
                    log("session " .. sid_short(sid) .. ": " .. part_type .. " done")
                end
            elseif status == "running" or status == "pending" then
                if part_type == "tool" then
                    entry.tool_count = entry.tool_count + 1
                end
                entry.job:stage(stage_name, meta)
                log("session " .. sid_short(sid) .. ": stage → " .. stage_name .. " (active: " .. entry.tool_count .. ")")
            else
                -- Parts without a recognised status (e.g. text/reasoning
                -- deltas that carry no state) still get staged so the LED
                -- reflects activity.
                entry.job:stage(stage_name, meta)
            end
        end),
    })

    -- Session error.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:session.error",
        callback = a.void(function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local sid = props.sessionID
            if not sid then return end
            local meta = error_meta(props.error)
            finish_session(sid, 1, FINISH_ERR_TIMEOUT_MS, "error → finish", meta)
        end),
    })

    -- Session deleted: payload is { info: Session }, same shape as session.updated.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:session.deleted",
        callback = a.void(function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local sid = props.info and props.info.id
            if not sid then return end
            finish_session(sid, 0, FINISH_DELETE_TIMEOUT_MS, "deleted")
        end),
    })

    -- Question asked: show as a stage so the LED reflects the pending question.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:question.asked",
        callback = a.void(function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local sid = props.sessionID
            if not sid then return end
            local entry = active[sid]
            if not entry or not entry.job then return end
            entry.job:stage("question")
            log("session " .. sid_short(sid) .. ": question.asked → stage")
        end),
    })

    -- File edited by the AI: show as a stage.
    -- This event carries only {file: string} with no sessionID, so we stage
    -- the first active session we find (typically there is only one).
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:file.edited",
        callback = a.void(function(args)
            local event = args.data and args.data.event
            local file = event and event.properties and event.properties.file
            local meta = file and { file = tostring(file) } or {}
            for sid, entry in pairs(active) do
                if entry.job then
                    entry.job:stage("edit", meta)
                    log("session " .. sid_short(sid) .. ": file.edited → stage")
                    break
                end
            end
        end),
    })

    -- Permission asked: start a keyboard prompt and wire up a resolve_once
    -- function that whichever path fires first (keyboard or Neovim UI) will
    -- call. Subsequent calls are no-ops, preventing double API calls.
    -- Each permission is tracked by its unique ID so that concurrent prompts
    -- (e.g. rapid tool calls) don't overwrite each other's resolvers.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:permission.asked",
        callback = a.void(function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local sid = props.sessionID
            if not sid then return end
            local entry = active[sid]
            if not entry or not entry.job then return end

            local perm_id = props.id
            local title   = props.title or "permission"
            local meta    = permission_meta(props)

            -- Build a one-shot resolver keyed by permission ID: whichever path
            -- fires first (keyboard or Neovim UI) wins; subsequent calls are no-ops.
            local called = false
            local function resolve_once(accepted)
                if called then return end
                called = true
                if perm_id then entry.pending_permissions[perm_id] = nil end

                log("session " .. sid_short(sid) .. ": permission → " .. (accepted and "accepted" or "rejected"))

                local api_ok, api = pcall(require, "opencode.api")
                if not api_ok then
                    warn("session " .. sid_short(sid) .. ": permission resolved but opencode.api unavailable")
                else
                    if accepted then
                        api.permission_accept()
                    else
                        api.permission_deny()
                    end
                end

                -- If the keyboard hasn't resolved yet, unblock it (no-op if it already did).
                a.void(function() entry.job:prompt_resolve(accepted) end)()
            end

            if perm_id then entry.pending_permissions[perm_id] = resolve_once end

            log("session " .. sid_short(sid) .. ': prompt "' .. title .. '" — awaiting keyboard or UI')

            -- Kick off the keyboard prompt in the background.
            -- When the keyboard responds it calls resolve_once; if the UI
            -- responded first, resolve_once is already a no-op.
            a.void(function()
                local accepted = entry.job:prompt(title, meta)
                resolve_once(accepted)
            end)()
        end),
    })

    -- Permission replied (from server, after UI or keyboard resolved it):
    -- look up the resolver by permission ID so concurrent prompts don't
    -- cross-resolve each other.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:permission.replied",
        callback = a.void(function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local sid = props.sessionID
            if not sid then return end
            local entry = active[sid]
            if not entry then return end

            local perm_id = props.permissionID or props.requestID
            local resolver = perm_id and entry.pending_permissions[perm_id]
            if not resolver then return end

            local meta     = permission_replied_meta(props)
            local response = props.response or "reject"
            local accepted = response ~= "reject"
            log("session " .. sid_short(sid) .. ": permission.replied (" .. response .. ")")

            -- Resolve the prompt on the keyboard side with metadata.
            if entry.job then
                a.void(function() entry.job:prompt_resolve(accepted, meta) end)()
            end

            resolver(accepted)
        end),
    })
end

function M.teardown()
    if augroup then
        vim.api.nvim_del_augroup_by_id(augroup)
        augroup = nil
    end
    for _, entry in pairs(active) do
        a.void(function() entry.job:finish(0, 0) end)() -- fire-and-forget immediate clear
    end
    active = {}
end

return M
