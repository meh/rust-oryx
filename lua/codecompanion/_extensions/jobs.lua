-- codecompanion/_extensions/jobs.lua
-- Tracks CodeCompanion chat lifecycle on keyboard LEDs via the oryx-jobs service.
--
-- Enable by adding to your codecompanion setup:
--   require("codecompanion").setup({
--     extensions = { jobs = {} },
--   })

local Extension = {}

function Extension.setup(_opts)
    local a    = require("plenary.async")
    local jobs = require("jobs")
    local active = {} -- chat_id -> Job

    vim.api.nvim_create_autocmd("User", {
        pattern = "CodeCompanionChatCreated",
        callback = a.void(function(args)
            local id = args.data.id
            if active[id] then return end
            local job = jobs.create({ name = "CodeCompanion", chat_id = tostring(id) })
            if not job then return end
            job:start()
            active[id] = job
        end),
    })

    vim.api.nvim_create_autocmd("User", {
        pattern = "CodeCompanionChatSubmitted",
        callback = a.void(function(args)
            local job = active[args.data.id]
            if job then job:start() end
        end),
    })

    vim.api.nvim_create_autocmd("User", {
        pattern = "CodeCompanionChatDone",
        callback = a.void(function(args)
            local job = active[args.data.id]
            if job then job:finish(0) end
        end),
    })

    vim.api.nvim_create_autocmd("User", {
        pattern = "CodeCompanionChatClosed",
        callback = a.void(function(args)
            local id = args.data.id
            local job = active[id]
            if job then
                job:finish(0)
                active[id] = nil
            end
        end),
    })
end

return Extension
