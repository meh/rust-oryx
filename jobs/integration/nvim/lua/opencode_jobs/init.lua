-- opencode_jobs/init.lua — Maps opencode.nvim SSE events to oryx-jobs keyboard LEDs.
--
-- Tracks opencode sessions as jobs on the ZSA keyboard, showing LED feedback
-- for session activity, tool usage stages, and permission prompts.
--
-- Usage:
--   require("opencode_jobs").setup()
--
-- The opencode.nvim plugin must already be set up and emitting OpencodeEvent:*
-- autocmds. This module also requires the `jobs` module (../jobs/init.lua) on
-- the runtimepath.

local jobs = require("jobs")

local M = {}

local active = {} -- session_id -> { job = Job, tool_count = number }
local augroup = nil

--- Get or create a job for the given session.
--- @param session_id string
--- @return table? job
local function get_or_create(session_id)
    local entry = active[session_id]
    if entry then return entry.job end

    local job = jobs.create({ name = "opencode", session_id = session_id })
    if not job then return nil end
    job:start()
    active[session_id] = { job = job, tool_count = 0 }
    return job
end

--- Clean up a session entry.
--- @param session_id string
local function cleanup(session_id)
    active[session_id] = nil
end

function M.setup()
    if augroup then return end -- already set up
    augroup = vim.api.nvim_create_augroup("OryxOpencode", { clear = true })

    -- Session becomes active (new or resumed): create/start a job.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:session.updated",
        callback = function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local info = props.info or props
            local sid = info.id or (info.session and info.session.id)
            if not sid then return end
            get_or_create(sid)
        end,
    })

    -- Tool call started/updated: show as a stage.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:message.part.updated",
        callback = function(args)
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

            -- Only transition to stage when a tool is running (not completed).
            local state = part.state
            local status = state and (state.status or state)
            if status == "completed" or status == "error" then
                entry.tool_count = math.max(0, entry.tool_count - 1)
                if entry.tool_count == 0 then
                    entry.job:start() -- back to started (running, no active tool)
                end
            else
                entry.tool_count = entry.tool_count + 1
                entry.job:stage(tool_name)
            end
        end,
    })

    -- Session idle: all work done.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:session.idle",
        callback = function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local sid = props.sessionID or props.id
            if not sid then return end
            local entry = active[sid]
            if not entry then return end
            entry.job:finish(0)
        end,
    })

    -- Session error.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:session.error",
        callback = function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local sid = props.sessionID or props.id
            if not sid then return end
            local entry = active[sid]
            if not entry then return end
            entry.job:finish(1)
        end,
    })

    -- Session deleted.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:session.deleted",
        callback = function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local sid = props.sessionID or props.id
            if not sid then return end
            local entry = active[sid]
            if not entry then return end
            entry.job:finish(0)
            cleanup(sid)
        end,
    })

    -- Permission asked: prompt on the keyboard, then auto-reply.
    vim.api.nvim_create_autocmd("User", {
        group = augroup,
        pattern = "OpencodeEvent:permission.asked",
        callback = function(args)
            local event = args.data and args.data.event
            if not event then return end
            local props = event.properties or {}
            local sid = props.sessionID
            if not sid then return end
            local entry = active[sid]
            if not entry then return end

            local title = props.title or "permission"

            entry.job:prompt(title, function(accepted)
                local api_ok, api = pcall(require, "opencode.api")
                if not api_ok then return end
                if accepted then
                    api.permission_accept()
                else
                    api.permission_deny()
                end
            end)
        end,
    })
end

function M.teardown()
    if augroup then
        vim.api.nvim_del_augroup_by_id(augroup)
        augroup = nil
    end
    for sid, entry in pairs(active) do
        pcall(function() entry.job:finish(0) end)
    end
    active = {}
end

return M
