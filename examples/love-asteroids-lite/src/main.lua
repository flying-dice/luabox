-- asteroids-lite: the tiniest LÖVE game skeleton — a rectangle you move with
-- the arrow keys. Kept minimal on purpose; it's a starting point, not a game.
--
-- The `love.*` callbacks are typed against defs/love2d.d.lua, so `luabox check`
-- verifies your calls into the LÖVE API (arity and types).

local player = { x = 150, y = 110, size = 20, speed = 160 }

function love.load()
    player.x = 150
    player.y = 110
end

---@param dt number
function love.update(dt)
    if love.keyboard.isDown("left") then
        player.x = player.x - player.speed * dt
    end
    if love.keyboard.isDown("right") then
        player.x = player.x + player.speed * dt
    end
    if love.keyboard.isDown("up") then
        player.y = player.y - player.speed * dt
    end
    if love.keyboard.isDown("down") then
        player.y = player.y + player.speed * dt
    end
end

function love.draw()
    love.graphics.rectangle("fill", player.x, player.y, player.size, player.size)
end
