Feature: luabox lsp — malformed messages
  SPEC.md §8 — editors send nonsense: params from a protocol revision the
  server does not know, a `file://` URI whose spaces were never
  percent-encoded, a `null` where an object belongs. A language server that
  dies on one of those takes the whole editing session with it — every open
  buffer loses diagnostics, hover, and completion, and the request the client
  was blocked on is never answered at all.

  So a malformed *request* is answered with the protocol's own
  `-32602 InvalidParams`, naming the method; a malformed *notification*, which
  has no id to answer, is logged to the client's pane and dropped. Either way
  the next well-formed message is served normally. The only message that ends
  the process is `exit`, and an `exit` that was not preceded by `shutdown`
  ends it with the exit code the spec assigns: 1.

  Background:
    Given a project with edition "5.4"
    And a file "main.lua" containing:
      """
      local value = 1
      print(value)
      """

  Scenario: a hover with no position is refused, not fatal
    Given the language server is running
    And the document "main.lua" is open
    When I request a hover for "main.lua" with no position
    Then the server replies with error code -32602
    And the server replies with an error mentioning "textDocument/hover"

  Scenario: the session survives a malformed request and serves the next one
    Given the language server is running
    And the document "main.lua" is open
    When I request a hover for "main.lua" with no position
    Then the server replies with error code -32602
    When I hover at 1:8 in "main.lua"
    Then the hover text contains "local value"

  Scenario: a didOpen whose URI has an unencoded space is dropped, not fatal
    Given the language server is running
    When I send a didOpen notification with the raw URI "file:///tmp/a b.lua"
    And I open "main.lua"
    Then the server logged an error containing "textDocument/didOpen"
    When I hover at 1:8 in "main.lua"
    Then the hover text contains "local value"

  Scenario: a didOpen missing its languageId is dropped, not fatal
    Given the language server is running
    When I send a didOpen notification with no languageId for "main.lua"
    And I open "main.lua"
    Then the server logged an error containing "languageId"
    When I hover at 1:8 in "main.lua"
    Then the hover text contains "local value"

  Scenario: exit without a prior shutdown exits with code 1
    Given the language server is running
    When I send exit without a prior shutdown
    Then the server process exits with code 1
