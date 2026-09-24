-- A rectangle you move with the arrow keys. `love` is declared by
-- headers/love.luah; assigning its callback fields defines the game.




local player         = { x = 150, y = 110, size = 20, speed = 160 }

-- The direction each key moves the player in.
local dx              = { left = -1, right = 1 }
local dy              = { up = -1, down = 1 }

function love.load()
  player.x, player.y = 150, 110
end

-- `dt` takes its type, number, from the `update` field.
function love.update(dt)
  for key, step in pairs(dx) do
    if love.keyboard.isDown(key) then
      player.x = player.x + step * player.speed * dt
    end
  end
  for key, step in pairs(dy) do
    if love.keyboard.isDown(key) then
      player.y = player.y + step * player.speed * dt
    end
  end
  if love.keyboard.isDown("escape") then
    love.event.quit()
  end
end

function love.draw()
  love.graphics.rectangle("fill", player.x, player.y, player.size, player.size)
  love.graphics.print("arrows to move, escape to quit", 10, 10)
end
