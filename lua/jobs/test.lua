-- jobs/test.lua — interactive test suite for the zsa.oryx.Jobs DBus service.
--
-- Source from Neovim while oryx-jobs is running:
--   :luafile /path/to/lua/jobs/test.lua
--
-- Then call individual tests:
--   :lua require("jobs.test").lifecycle()
--   :lua require("jobs.test").prompt_race()
--   :lua require("jobs.test").all()

local a    = require("plenary.async")
local jobs = require("jobs")

local M = {}

-- ── helpers ───────────────────────────────────────────────────────────────────

local function log(msg)
    vim.notify("[jobs.test] " .. msg, vim.log.levels.INFO)
end

local function fail(msg)
    vim.notify("[jobs.test] FAIL: " .. msg, vim.log.levels.ERROR)
end

-- Async sleep usable inside a.void/a.run coroutines.
local sleep = a.wrap(function(ms, cb)
    local t = vim.uv.new_timer()
    t:start(ms, 0, function()
        t:stop()
        t:close()
        vim.schedule(cb)
    end)
end, 2)

local function new_job(name, meta)
    local m = vim.tbl_extend("force", { name = name }, meta or {})
    local job = jobs.create(m)
    if not job then
        fail(name .. ": create failed (service down or no free slot)")
    end
    return job
end

-- ── tests ─────────────────────────────────────────────────────────────────────

--- Basic lifecycle: created → started → progress (×4) → stage → finish(0).
function M.lifecycle()
    a.void(function()
        log("lifecycle: start")
        local job = new_job("lifecycle")
        if not job then return end

        sleep(300)  ; job:start()             ; log("lifecycle: started")
        sleep(400)  ; job:progress(25, 100)
        sleep(400)  ; job:progress(50, 100)
        sleep(400)  ; job:progress(75, 100)
        sleep(1000) ; job:stage("compiling")  ; log("lifecycle: stage compiling")
        sleep(2000) ; job:stage("compiling")  ; log("lifecycle: stage tool")
        sleep(2000) ; job:progress(100, 100)
        sleep(400)  ; job:finish(0, 2000)
        log("lifecycle: done — auto-clear in 2 s")
    end)()
end

--- Finish with error value (exit code 1 → red LED).
function M.finish_error()
    a.void(function()
        log("finish_error: start")
        local job = new_job("finish-err")
        if not job then return end
        job:start()
        sleep(800)
        job:finish(1, 3000)
        log("finish_error: done — auto-clear in 3 s")
    end)()
end

--- Prompt driven by the physical keyboard key (tap = accept, hold = reject).
function M.prompt_keyboard()
    a.void(function()
        log("prompt_keyboard: tap the slot key to accept, hold to reject")
        local job = new_job("prompt-kbd")
        if not job then return end
        job:start() ; sleep(300)

        local accepted = job:prompt("allow keyboard test?")
        log("prompt_keyboard: → " .. (accepted and "ACCEPTED" or "REJECTED"))
        job:finish(accepted and 0 or 1, 2000)
    end)()
end

--- Programmatic accept via prompt_resolve after 1 s.
function M.prompt_resolve_accept()
    a.void(function()
        log("prompt_resolve_accept: start")
        local job = new_job("resolve-accept")
        if not job then return end
        job:start() ; sleep(300)

        a.void(function()
            local r = job:prompt("programmatic accept test")
            if r ~= true then
                fail("prompt_resolve_accept: expected true, got " .. tostring(r))
            else
                log("prompt_resolve_accept: prompt → ACCEPTED (correct)")
            end
        end)()

        sleep(1000)
        log("prompt_resolve_accept: calling prompt_resolve(true)")
        job:prompt_resolve(true)
        sleep(800) ; job:finish(0, 2000)
        log("prompt_resolve_accept: done")
    end)()
end

--- Programmatic reject via prompt_resolve after 1 s.
function M.prompt_resolve_reject()
    a.void(function()
        log("prompt_resolve_reject: start")
        local job = new_job("resolve-reject")
        if not job then return end
        job:start() ; sleep(300)

        a.void(function()
            local r = job:prompt("programmatic reject test")
            if r ~= false then
                fail("prompt_resolve_reject: expected false, got " .. tostring(r))
            else
                log("prompt_resolve_reject: prompt → REJECTED (correct)")
            end
        end)()

        sleep(1000)
        log("prompt_resolve_reject: calling prompt_resolve(false)")
        job:prompt_resolve(false)
        sleep(800) ; job:finish(1, 2000)
        log("prompt_resolve_reject: done")
    end)()
end

--- Race: prompt_resolve fires BEFORE prompt() reaches the server.
--- Exercises the pre_resolved fast path in the Rust service.
function M.prompt_race()
    a.void(function()
        log("prompt_race: prompt_resolve will fire before Prompt() DBus call")
        local job = new_job("prompt-race")
        if not job then return end
        job:start() ; sleep(300)

        -- Fire resolve immediately — no Prompt() call has been made yet.
        job:prompt_resolve(true)
        log("prompt_race: prompt_resolve(true) sent")

        -- prompt() should return immediately once the server sees the pre-resolve.
        a.void(function()
            local r = job:prompt("race test")
            if r ~= true then
                fail("prompt_race: expected true, got " .. tostring(r))
            else
                log("prompt_race: prompt returned immediately → ACCEPTED (correct)")
            end
        end)()

        sleep(2000) ; job:finish(0, 2000)
        log("prompt_race: done")
    end)()
end

--- Three concurrent prompts on different slots, resolved staggered from software.
function M.prompt_multi()
    a.void(function()
        log("prompt_multi: 3 concurrent prompts (need ≥3 free slots)")

        local j1 = new_job("multi-1")
        local j2 = new_job("multi-2")
        local j3 = new_job("multi-3")
        if not j1 or not j2 or not j3 then return end

        j1:start() ; j2:start() ; j3:start()
        sleep(300)

        a.void(function()
            local r = j1:prompt("multi 1")
            log("prompt_multi: j1 → " .. (r and "ACCEPTED" or "REJECTED"))
        end)()
        a.void(function()
            local r = j2:prompt("multi 2")
            log("prompt_multi: j2 → " .. (r and "ACCEPTED" or "REJECTED"))
        end)()
        a.void(function()
            local r = j3:prompt("multi 3")
            log("prompt_multi: j3 → " .. (r and "ACCEPTED" or "REJECTED"))
        end)()

        sleep(800)  ; log("prompt_multi: resolving j1 accept") ; j1:prompt_resolve(true)
        sleep(600)  ; log("prompt_multi: resolving j2 reject") ; j2:prompt_resolve(false)
        sleep(600)  ; log("prompt_multi: resolving j3 accept") ; j3:prompt_resolve(true)

        sleep(1000)
        j1:finish(0, 1500) ; j2:finish(1, 1500) ; j3:finish(0, 1500)
        log("prompt_multi: done")
    end)()
end

--- Three sequential prompts on the SAME job — confirms no stuck state after resolve.
function M.prompt_sequential()
    a.void(function()
        log("prompt_sequential: 3 prompts in sequence on one job")
        local job = new_job("prompt-seq")
        if not job then return end
        job:start() ; sleep(300)

        for i = 1, 3 do
            local accept = (i % 2 == 1)
            log("prompt_sequential: prompt " .. i .. " — will resolve " .. (accept and "accept" or "reject"))

            a.void(function()
                local r = job:prompt("sequential " .. i)
                if r ~= accept then
                    fail("prompt_sequential: prompt " .. i .. " expected " .. tostring(accept) .. " got " .. tostring(r))
                else
                    log("prompt_sequential: prompt " .. i .. " → " .. tostring(r) .. " (correct)")
                end
            end)()

            sleep(600)
            job:prompt_resolve(accept)
            sleep(900) -- wait for pulse animation before next prompt
        end

        sleep(400) ; job:finish(0, 2000)
        log("prompt_sequential: done")
    end)()
end

--- Rapid stage cycling to stress LED update throughput.
function M.stage_cycle()
    a.void(function()
        log("stage_cycle: start")
        local job = new_job("stage-cycle")
        if not job then return end
        job:start()

        local stages = { "compiling", "linking", "testing", "deploying", "cleaning" }
        for round = 1, 3 do
            for _, s in ipairs(stages) do
                job:stage(s)
                log("stage_cycle: [" .. round .. "] " .. s)
                sleep(250)
            end
        end

        job:finish(0, 2000)
        log("stage_cycle: done")
    end)()
end

--- Fill every available slot simultaneously, then clear them.
function M.fill_slots()
    a.void(function()
        log("fill_slots: filling all available slots")
        local created = {}
        for i = 1, 32 do
            -- timeout_ms=0: return nil immediately rather than blocking if no slot free
            local job = jobs.create({ name = "slot-" .. i }, -1, 0)
            if not job then
                log("fill_slots: stopped at " .. #created .. " slots")
                break
            end
            job:start()
            table.insert(created, job)
        end
        log("fill_slots: " .. #created .. " slots occupied")

        sleep(1500)
        for _, job in ipairs(created) do
            job:finish(0, 0)
            sleep(150)
        end
        log("fill_slots: done")
    end)()
end

--- Run the automated tests (skips prompt_keyboard) in sequence.
function M.all()
    a.void(function()
        log("all: running automated suite")

        M.lifecycle()               ; sleep(5000)
        M.finish_error()            ; sleep(5000)
        M.prompt_resolve_accept()   ; sleep(5000)
        M.prompt_resolve_reject()   ; sleep(5000)
        M.prompt_race()             ; sleep(5000)
        M.prompt_multi()            ; sleep(8000)
        M.prompt_sequential()       ; sleep(10000)
        M.stage_cycle()             ; sleep(5000)

        log("all: suite complete")
    end)()
end

-- ── info on load ──────────────────────────────────────────────────────────────

log("loaded — tests:")
for _, name in ipairs({
    "lifecycle", "finish_error",
    "prompt_keyboard", "prompt_resolve_accept", "prompt_resolve_reject",
    "prompt_race", "prompt_multi", "prompt_sequential",
    "stage_cycle", "fill_slots", "all",
}) do
    log("  require('jobs.test')." .. name .. "()")
end

return M
