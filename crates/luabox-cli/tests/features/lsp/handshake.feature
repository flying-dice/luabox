Feature: luabox lsp — initialize handshake
  SPEC.md §8 — `luabox lsp` speaks LSP over stdio. The first thing an
  editor learns is the capability set, so the handshake is the contract:
  what the server answers, and how documents are synchronised. Every
  scenario below drives the real binary over its stdin/stdout.

  Background:
    Given a project with edition "5.4"

  Scenario: the server names itself in the initialize result
    When the language server starts
    Then the server identifies itself as "luabox-lsp"

  Scenario: documents are synchronised incrementally
    When the language server starts
    Then capability "textDocumentSync" equals 2

  Scenario: the navigation providers are advertised
    When the language server starts
    Then capability "hoverProvider" is advertised
    And capability "definitionProvider" is advertised
    And capability "typeDefinitionProvider" is advertised
    And capability "implementationProvider" is advertised
    And capability "referencesProvider" is advertised
    And capability "callHierarchyProvider" is advertised

  Scenario: the editing providers are advertised
    When the language server starts
    Then capability "documentFormattingProvider" is advertised
    And capability "documentRangeFormattingProvider" is advertised
    And capability "renameProvider.prepareProvider" is advertised
    And capability "codeActionProvider" is advertised
    And capability "inlayHintProvider" is advertised

  Scenario: the presentation providers are advertised
    When the language server starts
    Then capability "documentSymbolProvider" is advertised
    And capability "workspaceSymbolProvider" is advertised
    And capability "documentHighlightProvider" is advertised
    And capability "foldingRangeProvider" is advertised
    And capability "selectionRangeProvider" is advertised
    And capability "semanticTokensProvider.full" is advertised

  Scenario: completion is triggered on member access
    When the language server starts
    Then capability "completionProvider.triggerCharacters" includes "."
    And capability "completionProvider.triggerCharacters" includes ":"

  Scenario: signature help is triggered inside argument lists
    When the language server starts
    Then capability "signatureHelpProvider.triggerCharacters" includes "("
    And capability "signatureHelpProvider.retriggerCharacters" includes ","

  Scenario: the semantic token legend covers the types luabox emits
    When the language server starts
    Then capability "semanticTokensProvider.legend.tokenTypes" includes "variable"
    And capability "semanticTokensProvider.legend.tokenTypes" includes "parameter"
    And capability "semanticTokensProvider.legend.tokenTypes" includes "function"
    And capability "semanticTokensProvider.legend.tokenTypes" includes "comment"

  Scenario: an unsupported request is refused, not ignored
    Given the language server is running
    When I send an unsupported request
    Then the server replies with an error mentioning "unhandled method"
