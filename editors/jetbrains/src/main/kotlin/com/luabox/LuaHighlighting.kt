package com.luabox

import com.intellij.lexer.LexerBase
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.fileTypes.SyntaxHighlighterBase
import com.intellij.openapi.fileTypes.SyntaxHighlighterFactory
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType

/**
 * Base lexical highlighting for `.lua` files: keywords, comments, strings,
 * numbers. The luabox LSP semantic tokens overlay semantic colour (locals vs
 * globals, doc comments) on top — the server deliberately does not emit
 * tokens for plain syntax, expecting the client to own the base layer (in
 * VS Code that's a TextMate grammar; here it's this lexer).
 *
 * The rules mirror `luabox-syntax`'s lexer where it matters for colouring:
 * long brackets at any level (`[[ ]]`, `[=[ ]=]`) for strings and comments,
 * escape-aware short strings, hex/float numbers.
 */
object LuaTokenTypes {
    @JvmField val KEYWORD = IElementType("LUA_KEYWORD", LuaLanguage)
    @JvmField val STRING = IElementType("LUA_STRING", LuaLanguage)
    @JvmField val NUMBER = IElementType("LUA_NUMBER", LuaLanguage)
    @JvmField val COMMENT = IElementType("LUA_COMMENT", LuaLanguage)
    @JvmField val IDENTIFIER = IElementType("LUA_IDENTIFIER", LuaLanguage)
    @JvmField val PUNCT = IElementType("LUA_PUNCT", LuaLanguage)
}

private val KEYWORDS = setOf(
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function",
    "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return",
    "then", "true", "until", "while",
)

/** Single-pass scanner; every token is self-delimiting so state is always 0. */
class LuaHighlightingLexer : LexerBase() {
    private var buffer: CharSequence = ""
    private var endOffset = 0
    private var tokenStart = 0
    private var tokenEnd = 0
    private var tokenType: IElementType? = null

    override fun start(buffer: CharSequence, startOffset: Int, endOffset: Int, initialState: Int) {
        this.buffer = buffer
        this.endOffset = endOffset
        tokenStart = startOffset
        tokenEnd = startOffset
        advance()
    }

    override fun getState(): Int = 0
    override fun getTokenType(): IElementType? = tokenType
    override fun getTokenStart(): Int = tokenStart
    override fun getTokenEnd(): Int = tokenEnd
    override fun getBufferSequence(): CharSequence = buffer
    override fun getBufferEnd(): Int = endOffset

    override fun advance() {
        tokenStart = tokenEnd
        if (tokenStart >= endOffset) {
            tokenType = null
            return
        }
        val c = buffer[tokenStart]
        var pos = tokenStart
        when {
            c.isWhitespace() -> {
                while (pos < endOffset && buffer[pos].isWhitespace()) pos++
                tokenType = TokenType.WHITE_SPACE
            }
            c == '-' && peek(pos + 1) == '-' -> {
                pos += 2
                val close = longBracketClose(pos)
                pos = if (close != null) close else run {
                    var p = pos
                    while (p < endOffset && buffer[p] != '\n') p++
                    p
                }
                tokenType = LuaTokenTypes.COMMENT
            }
            c == '[' && longBracketClose(tokenStart) != null -> {
                pos = longBracketClose(tokenStart)!!
                tokenType = LuaTokenTypes.STRING
            }
            c == '\'' || c == '"' -> {
                pos++
                while (pos < endOffset && buffer[pos] != c && buffer[pos] != '\n') {
                    pos += if (buffer[pos] == '\\' && pos + 1 < endOffset) 2 else 1
                }
                if (pos < endOffset && buffer[pos] == c) pos++
                tokenType = LuaTokenTypes.STRING
            }
            c.isDigit() || (c == '.' && peek(pos + 1)?.isDigit() == true) -> {
                pos = scanNumber(pos)
                tokenType = LuaTokenTypes.NUMBER
            }
            c.isLetter() || c == '_' -> {
                while (pos < endOffset && (buffer[pos].isLetterOrDigit() || buffer[pos] == '_')) pos++
                val word = buffer.subSequence(tokenStart, pos).toString()
                tokenType = if (word in KEYWORDS) LuaTokenTypes.KEYWORD else LuaTokenTypes.IDENTIFIER
            }
            else -> {
                pos++
                tokenType = LuaTokenTypes.PUNCT
            }
        }
        tokenEnd = pos
    }

    private fun peek(at: Int): Char? = if (at < endOffset) buffer[at] else null

    /**
     * If a long bracket `[=*[` opens at [open], return the offset just past
     * its matching `]=*]` (or [endOffset] when unterminated); null otherwise.
     */
    private fun longBracketClose(open: Int): Int? {
        if (peek(open) != '[') return null
        var level = 0
        while (peek(open + 1 + level) == '=') level++
        if (peek(open + 1 + level) != '[') return null
        var p = open + 2 + level
        while (p < endOffset) {
            if (buffer[p] == ']') {
                var eq = 0
                while (peek(p + 1 + eq) == '=') eq++
                if (eq == level && peek(p + 1 + eq) == ']') return p + 2 + level
            }
            p++
        }
        return endOffset
    }

    private fun scanNumber(start: Int): Int {
        var p = start
        val isHex = peek(p) == '0' && (peek(p + 1) == 'x' || peek(p + 1) == 'X')
        if (isHex) p += 2
        val digit: (Char) -> Boolean =
            if (isHex) ({ it.isDigit() || it in 'a'..'f' || it in 'A'..'F' }) else ({ it.isDigit() })
        while (p < endOffset && digit(buffer[p])) p++
        if (peek(p) == '.' && peek(p + 1)?.let(digit) == true) {
            p++
            while (p < endOffset && digit(buffer[p])) p++
        }
        val expChar = peek(p)?.lowercaseChar()
        if (expChar == (if (isHex) 'p' else 'e')) {
            val sign = if (peek(p + 1) == '+' || peek(p + 1) == '-') 1 else 0
            if (peek(p + 1 + sign)?.isDigit() == true) {
                p += 1 + sign
                while (p < endOffset && buffer[p].isDigit()) p++
            }
        }
        return p
    }
}

class LuaSyntaxHighlighter : SyntaxHighlighterBase() {
    override fun getHighlightingLexer() = LuaHighlightingLexer()

    override fun getTokenHighlights(tokenType: IElementType?): Array<TextAttributesKey> =
        when (tokenType) {
            LuaTokenTypes.KEYWORD -> pack(DefaultLanguageHighlighterColors.KEYWORD)
            LuaTokenTypes.STRING -> pack(DefaultLanguageHighlighterColors.STRING)
            LuaTokenTypes.NUMBER -> pack(DefaultLanguageHighlighterColors.NUMBER)
            LuaTokenTypes.COMMENT -> pack(DefaultLanguageHighlighterColors.LINE_COMMENT)
            LuaTokenTypes.IDENTIFIER -> pack(DefaultLanguageHighlighterColors.IDENTIFIER)
            LuaTokenTypes.PUNCT -> pack(DefaultLanguageHighlighterColors.OPERATION_SIGN)
            else -> TextAttributesKey.EMPTY_ARRAY
        }
}

class LuaSyntaxHighlighterFactory : SyntaxHighlighterFactory() {
    override fun getSyntaxHighlighter(project: Project?, virtualFile: VirtualFile?) =
        LuaSyntaxHighlighter()
}
