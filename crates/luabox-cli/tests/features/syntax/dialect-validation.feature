Feature: Dialect-legality validation
  SPEC.md §2 + §2.1 — the parser accepts the union of every Lua dialect's
  grammar; `luabox check` diagnoses constructs that parsed but are illegal
  in the project's configured `edition`. Legal rows must produce no
  diagnostic at all — the union grammar accepting a construct is not
  itself a report. The future fix for legitimately-newer code is
  `luabox build --target <older-edition>` (SPEC.md §2.1 lowering table).

  Scenario Outline: dialect-illegal constructs are diagnosed per edition
    Given a project with edition "<edition>"
    And a Lua file containing '<source>'
    When I run "luabox check"
    Then <outcome>

    Examples: goto / labels (LB0010) — 5.1 lacks goto entirely; `goto` there
      is a plain identifier, so only a bare label can appear in 5.1 source.
      | edition | source                | outcome                                |
      | 5.1     | ::top::               | diagnostic LB0010 is reported          |
      | 5.2     | ::top:: goto top      | no dialect diagnostic is reported      |
      | 5.4     | ::top:: goto top      | no dialect diagnostic is reported      |
      | luajit  | ::top:: goto top      | no dialect diagnostic is reported      |

    Examples: integer division (LB0011) — 5.3+ only, never on LuaJIT
      | edition | source        | outcome                                |
      | 5.1     | x = a // b    | diagnostic LB0011 is reported          |
      | 5.2     | x = a // b    | diagnostic LB0011 is reported          |
      | luajit  | x = a // b    | diagnostic LB0011 is reported          |
      | 5.3     | x = a // b    | no dialect diagnostic is reported      |
      | 5.4     | x = a // b    | no dialect diagnostic is reported      |

    Examples: bitwise operators (LB0012) — 5.3+ only, never on LuaJIT
      | edition | source        | outcome                                |
      | 5.1     | x = a & b     | diagnostic LB0012 is reported          |
      | 5.2     | x = ~a        | diagnostic LB0012 is reported          |
      | luajit  | x = a << b    | diagnostic LB0012 is reported          |
      | 5.3     | x = a >> b    | no dialect diagnostic is reported      |
      | 5.4     | x = a & b     | no dialect diagnostic is reported      |
      | 5.1     | x = a ~= b    | no dialect diagnostic is reported      |

    Examples: <const>/<close> attribs (LB0013) — 5.4 only
      | edition | source                    | outcome                           |
      | 5.1     | local x <const> = 1       | diagnostic LB0013 is reported     |
      | 5.3     | local x <const> = 1       | diagnostic LB0013 is reported     |
      | luajit  | local x <close> = f()     | diagnostic LB0013 is reported     |
      | 5.4     | local x <const> = 1       | no dialect diagnostic is reported |

    Examples: hex float literals (LB0014) — 5.2+ and LuaJIT
      | edition | source              | outcome                              |
      | 5.1     | local x = 0x1p4     | diagnostic LB0014 is reported        |
      | 5.2     | local x = 0x1p4     | no dialect diagnostic is reported    |
      | luajit  | local x = 0x1.8p3   | no dialect diagnostic is reported    |
      | 5.1     | local x = 0xBEBADA  | no dialect diagnostic is reported    |

    Examples: \z / \x string escapes (LB0015) — 5.2+ and LuaJIT
      | edition | source                | outcome                            |
      | 5.1     | local s = "a\x41"     | diagnostic LB0015 is reported      |
      | 5.1     | local s = "a\z b"     | diagnostic LB0015 is reported      |
      | 5.2     | local s = "a\x41"     | no dialect diagnostic is reported  |
      | luajit  | local s = "a\z b"     | no dialect diagnostic is reported  |

    Examples: \u{...} string escape (LB0016) — 5.3+ only, never on LuaJIT
      | edition | source                | outcome                            |
      | 5.1     | local s = "a\u{48}"   | diagnostic LB0016 is reported      |
      | 5.2     | local s = "a\u{48}"   | diagnostic LB0016 is reported      |
      | luajit  | local s = "a\u{48}"   | diagnostic LB0016 is reported      |
      | 5.3     | local s = "a\u{48}"   | no dialect diagnostic is reported  |
      | 5.4     | local s = "a\u{48}"   | no dialect diagnostic is reported  |
