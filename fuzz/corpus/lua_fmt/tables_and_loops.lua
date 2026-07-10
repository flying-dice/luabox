local t <const> = { 1, 2, x = 'y', ['z'] = [[w]], f(); }
for i = 1, #t, 2 do t[i] = t[i] * 2 ^ i end
for k, v in pairs(t) do io.write(k, '=', tostring(v), '\n') end
