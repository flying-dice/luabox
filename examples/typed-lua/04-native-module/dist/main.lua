


local socket = require("socket.core")

local function                    fetch_status(       host)
  local conn, err = socket.connect(host, 80)
  if not conn then
    return nil, err
  end
  conn:settimeout(5)
  conn:send("HEAD / HTTP/1.0\r\nHost: " .. host .. "\r\n\r\n")
  local line, recv_err = conn:receive("*l")
  conn:close()
  return line, recv_err
end

local started = socket.gettime()
local status, err = fetch_status("example.com")
print(status or ("failed: " .. tostring(err)))
print(string.format("took %.3fs", socket.gettime() - started))
