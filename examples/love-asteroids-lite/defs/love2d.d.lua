---@meta
-- Minimal LÖVE (love2d) starter definitions — a STARTER SUBSET, not the full
-- API. Just enough of love.graphics / love.keyboard and the callbacks to type
-- this example. For a real project, pull in the community's full LÖVE defs.
--
-- Wired in via `[types] defs = ["love2d"]` in luabox.toml (the file stem
-- `love2d` is the package name).

love = {}
love.graphics = {}
love.keyboard = {}

--- Draw a rectangle. `mode` is "fill" or "line".
---@param mode string
---@param x number
---@param y number
---@param width number
---@param height number
function love.graphics.rectangle(mode, x, y, width, height) end

--- Is a keyboard key held down this frame?
---@param key string
---@return boolean
function love.keyboard.isDown(key) end

--- Configure the game before the window opens (love reads this from conf.lua).
---@param t table
function love.conf(t) end

--- Called once on startup.
function love.load() end

--- Called every frame with the delta time in seconds.
---@param dt number
function love.update(dt) end

--- Called every frame to render.
function love.draw() end
