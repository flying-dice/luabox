package.path = "src/?.lua;" .. package.path

local inifile = require("inifile")

local SAMPLE = [[
; a leading comment
name = luabox
mode = friendly

[server]
host = localhost
port = 8080

# a hash comment
[client]
retries = 3
]]

describe("inifile.parse", function()
    it("reads keys from the default (unheadered) section", function()
        local ini = inifile.parse(SAMPLE)
        assert.equal("luabox", ini:get("default", "name"))
        assert.equal("friendly", ini:get("default", "mode"))
    end)

    it("reads keys from named sections", function()
        local ini = inifile.parse(SAMPLE)
        assert.equal("localhost", ini:get("server", "host"))
        assert.equal("8080", ini:get("server", "port"))
        assert.equal("3", ini:get("client", "retries"))
    end)

    it("returns nil for missing sections or keys", function()
        local ini = inifile.parse(SAMPLE)
        assert.is_nil(ini:get("server", "missing"))
        assert.is_nil(ini:get("nope", "host"))
    end)

    it("ignores comments and blank lines", function()
        local ini = inifile.parse(SAMPLE)
        local names = ini:section_names()
        assert.same({ "client", "default", "server" }, names)
    end)

    it("trims whitespace around keys and values", function()
        local ini = inifile.parse("  spaced   =   value here  \n")
        assert.equal("value here", ini:get("default", "spaced"))
    end)
end)
