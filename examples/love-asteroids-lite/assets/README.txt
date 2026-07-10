Drop your sprites, sounds, and fonts here.

`luabox bundle --mode love` copies this assets/ directory into the .love
archive verbatim, so at runtime you can load files with paths relative to the
archive root, e.g.:

    local ship = love.graphics.newImage("assets/ship.png")

This placeholder keeps the directory tracked in git. Replace it with real
assets for your game.
