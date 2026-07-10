-- LÖVE reads conf.lua before the game window opens. `luabox bundle --mode love`
-- packages it into the .love archive alongside main.lua.

---@param t table
function love.conf(t)
    t.window.title = "Asteroids Lite"
    t.window.width = 320
    t.window.height = 240
end
