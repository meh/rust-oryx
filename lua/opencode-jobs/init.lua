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

local active  = {} -- session_id -> { job = Job, tool_count = number, pending_permission = function? }
local augroup = nil

-- ── logging ───────────────────────────────────────────────────────────────────

local function log(msg)
    vim.notify("[opencode_jobs] " .. msg, vim.log.levels.INFO)
end

local function warn(msg)
    vim.notify("[opencode_jobs] " .. msg, vim.log.levels.WARN)
end

--- Truncate a session ID to 8 chars for readable log lines.
local function sid_short(sid)
    return tostring(sid):sub(1, 8)
end

-- ── internals ─────────────────────────────────────────────────────────────────

--- Get or create a job for the given session, starting it immediately.
--- Must be called from within an a.void / a.run coroutine context.
--- @param session_id string
--- @return table? job
local function get_or_create(session_id)
    local entry = active[session_id]
    if entry then return entry.job end

    local job = jobs.create({ name = "opencode", session_id = session_id })
    if not job then
        warn("session " .. sid_short(session_id) .. ": failed to create job (service unavailable?)")
        return nil
    end
    job:start()
    active[session_id] = { job = job, tool_count = 0 }
    log("session " .. sid_short(session_id) .. ": job " .. tostring(job.id) .. " created")
    return job
end

--- Finish and clean up the active entry for a session.
--- Must be called from within an a.void / a.run coroutine context.
--- @param session_id string
--- @param value integer  0 = ok, 1 = error
--- @param timeout_ms integer  ms before auto-clear
--- @param reason string  log label
local function finish_session(session_id, value, timeout_ms, reason)
    local entry = active[session_id]
    if not entry then return end
    if value == 0 then
        log("session " .. sid_short(session_id) .. ": " .. reason)
    else
        warn("session " .. sid_short(session_id) .. ": " .. reason)
    end
    entry.job:finish(value, timeout_ms)
    active[session_id] = nil
end

-- ── setup / teardown ──────────────────────────────────────────────────────────

function M.setup()
    if augroup then return end -- already set up
    augroup = vim.api.nvim_create_augroup("OryxOpencode", { clear = true })

    -- session.status { type = "busy" }  → create + start the job.
    -- session.status { type = "idle" }  → finish the job (LLM done thinking).
    -- Other status types (e.g. "retry") are ignored.
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
            if status_type == "busy" then
                get_or_create(sid)
            elseif status_type == "idle" then
                finish_session(sid, 0, FINISH_OK_TIMEOUT_MS, "idle → finish")
            end
        end),
    })

    -- Tool call started/updated: show as a stage.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:message.part.updated",
        callback = a.void(function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local part = props.part or props
            if part.type ~= "tool" then return end
            local sid = part.sessionID
            if not sid then return end
            local entry = active[sid]
            if not entry then return end

            local tool_name = (part.tool and part.tool.name) or part.name or "tool"

            local state = part.state
            local status = state and (state.status or state)
            if status == "completed" or status == "error" or status == "failed" then
                entry.tool_count = math.max(0, entry.tool_count - 1)
                if entry.tool_count == 0 then
                    entry.job:start() -- back to started (running, no active tool)
                    log("session " .. sid_short(sid) .. ": tool done → started (active: 0)")
                else
                    log("session " .. sid_short(sid) .. ": tool done (active: " .. entry.tool_count .. ")")
                end
            elseif status == "running" or status == "pending" then
                entry.tool_count = entry.tool_count + 1
                entry.job:stage(tool_name)
                log("session " .. sid_short(sid) .. ": stage → " .. tool_name .. " (active: " .. entry.tool_count .. ")")
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
            finish_session(sid, 1, FINISH_ERR_TIMEOUT_MS, "error → finish")
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

    -- Permission asked: start a keyboard prompt and wire up a resolve_once
    -- function that whichever path fires first (keyboard or Neovim UI) will
    -- call. Subsequent calls are no-ops, preventing double API calls.
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
            if not entry then return end

            local title = props.title or "permission"

            -- Build a one-shot resolver: whichever path fires first wins.
            local called = false
            local function resolve_once(accepted)
                if called then return end
                called = true
                entry.pending_permission = nil

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
                entry.job:prompt_resolve(accepted)
            end

            entry.pending_permission = resolve_once

            log("session " .. sid_short(sid) .. ': prompt "' .. title .. '" — awaiting keyboard or UI')

            -- Kick off the keyboard prompt in the background.
            -- When the keyboard responds it calls resolve_once; if the UI
            -- responded first, resolve_once is already a no-op.
            a.void(function()
                local accepted = entry.job:prompt(title)
                resolve_once(accepted)
            end)()
        end),
    })

    -- Permission replied (from server, after UI or keyboard resolved it):
    -- call resolve_once so that if the UI acted first the keyboard LED is
    -- updated; if the keyboard acted first this is a harmless no-op.
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
            if not entry or not entry.pending_permission then return end

            local response = props.reply or "reject"
            local accepted = response ~= "reject"
            log("session " .. sid_short(sid) .. ": permission.replied (" .. response .. ")")
            entry.pending_permission(accepted)
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
