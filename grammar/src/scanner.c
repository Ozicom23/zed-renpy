#include "tree_sitter/array.h"
#include "tree_sitter/parser.h"

#include <assert.h>
#include <stdint.h>
#include <string.h>

// Must match order in externals in grammar.js
enum TokenType {
    NEWLINE,
    INDENT,
    DEDENT,
    PYTHON_CONTENT,
    BRACKETED,
};

typedef struct {
    Array(uint16_t) indents;
    // Further DEDENTs owed by the line just measured: one line can close
    // several blocks (`return` after a menu choice), but the measurement only
    // happens at column 0, before the first zero-width DEDENT moves us off it.
    uint8_t pending_dedents;
    // A file that does not end in a newline still ends its last line: one
    // zero-width NEWLINE is emitted at EOF, and only one.
    bool eof_newline_done;
} Scanner;

static inline void advance(TSLexer *lexer) { lexer->advance(lexer, false); }
static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

static inline bool at_line_end(TSLexer *lexer) {
    return lexer->eof(lexer) || lexer->lookahead == '\n' || lexer->lookahead == '\r';
}

static inline bool is_space(int32_t c) { return c == ' ' || c == '\t' || c == '\f'; }

static inline bool is_closing(int32_t c) { return c == ')' || c == ']' || c == '}'; }

// Consume one Python string literal starting at its opening quote. Triple-
// quoted strings may span lines; single-quoted ones stop at the line end
// (unterminated) so a stray quote cannot swallow the rest of the file.
static void scan_python_string(TSLexer *lexer) {
    int32_t quote = lexer->lookahead;
    advance(lexer);
    bool triple = false;
    if (lexer->lookahead == quote) {
        advance(lexer);
        if (lexer->lookahead != quote) {
            return; // empty string
        }
        advance(lexer);
        triple = true;
    }
    while (!lexer->eof(lexer)) {
        int32_t c = lexer->lookahead;
        if (c == '\\') {
            advance(lexer);
            if (!lexer->eof(lexer)) {
                advance(lexer);
            }
            continue;
        }
        if (!triple && (c == '\n' || c == '\r')) {
            return;
        }
        advance(lexer);
        if (c == quote) {
            if (!triple) {
                return;
            }
            if (lexer->lookahead == quote) {
                advance(lexer);
                if (lexer->lookahead == quote) {
                    advance(lexer);
                    return;
                }
            }
        }
    }
}

// Python source up to the end of its logical line: newlines inside open
// brackets or triple-quoted strings do not end it (`define x = _p("""` +
// following lines, `Character("e",\n    color=...)`). A trailing `#` comment
// at bracket depth 0 is left to the comment extra.
//
// Inside brackets, a following line that starts at column 0 with anything but
// a closing bracket ends the token at the previous line instead: Ren'Py
// continuation lines are indented, and this keeps a half-typed `foo(` from
// swallowing the rest of the file while editing.
static bool scan_python_content(TSLexer *lexer) {
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
        skip(lexer);
    }
    if (at_line_end(lexer) || lexer->lookahead == '#') {
        return false;
    }
    uint32_t depth = 0;
    bool consumed = false;
    while (!lexer->eof(lexer)) {
        int32_t c = lexer->lookahead;
        if (c == '\n' || c == '\r') {
            if (depth == 0) {
                break;
            }
            advance(lexer);
            if (c == '\r' && lexer->lookahead == '\n') {
                advance(lexer);
            }
            int32_t next = lexer->lookahead;
            if (!lexer->eof(lexer) && next != ' ' && next != '\t' && next != '\n' &&
                next != '\r' && next != ')' && next != ']' && next != '}') {
                break; // token ends at the last mark_end
            }
            continue;
        }
        if (c == '#') {
            if (depth == 0) {
                break;
            }
            while (!at_line_end(lexer)) {
                advance(lexer);
            }
            lexer->mark_end(lexer);
            continue;
        }
        if (c == '"' || c == '\'') {
            scan_python_string(lexer);
            lexer->mark_end(lexer);
            consumed = true;
            continue;
        }
        if (c == '(' || c == '[' || c == '{') {
            depth++;
        } else if ((c == ')' || c == ']' || c == '}') && depth > 0) {
            depth--;
        }
        advance(lexer);
        if (c != ' ' && c != '\t') {
            lexer->mark_end(lexer);
            consumed = true;
        }
    }
    return consumed;
}

// One balanced bracket group starting at its opening bracket: `(0.5, 1.0)`,
// `[Jump("a"), Return()]`, `("start", x=1)`. Like python_content it spans
// lines while the brackets are open (with the same column-0 bail-out), so
// screen-language and ATL arguments may continue across lines.
static bool scan_bracketed(TSLexer *lexer) {
    uint32_t depth = 0;
    while (!lexer->eof(lexer)) {
        int32_t c = lexer->lookahead;
        if (c == '\n' || c == '\r') {
            advance(lexer);
            if (c == '\r' && lexer->lookahead == '\n') {
                advance(lexer);
            }
            int32_t next = lexer->lookahead;
            if (!lexer->eof(lexer) && !is_space(next) && next != '\n' && next != '\r' &&
                !is_closing(next)) {
                break; // unterminated: ends at the last mark_end
            }
            continue;
        }
        if (c == '#') {
            while (!at_line_end(lexer)) {
                advance(lexer);
            }
            continue;
        }
        if (c == '"' || c == '\'') {
            scan_python_string(lexer);
            lexer->mark_end(lexer);
            continue;
        }
        advance(lexer);
        if (c == '(' || c == '[' || c == '{') {
            depth++;
        } else if (is_closing(c)) {
            lexer->mark_end(lexer);
            if (depth <= 1) {
                return true;
            }
            depth--;
            continue;
        }
        if (!is_space(c)) {
            lexer->mark_end(lexer);
        }
    }
    return true;
}

void *tree_sitter_renpy_external_scanner_create() {
    Scanner *scanner = calloc(1, sizeof(Scanner));
    array_init(&scanner->indents);
    // Push base indentation level of 0
    array_push(&scanner->indents, 0);
    return scanner;
}

void tree_sitter_renpy_external_scanner_destroy(void *payload) {
    Scanner *scanner = (Scanner *)payload;
    array_delete(&scanner->indents);
    free(scanner);
}

unsigned tree_sitter_renpy_external_scanner_serialize(void *payload, char *buffer) {
    Scanner *scanner = (Scanner *)payload;
    size_t size = 0;
    buffer[size++] = (char)scanner->pending_dedents;
    buffer[size++] = (char)scanner->eof_newline_done;
    uint32_t iter = 1;
    for (; iter < scanner->indents.size && size + 1 < TREE_SITTER_SERIALIZATION_BUFFER_SIZE; ++iter) {
        uint16_t indent_value = *array_get(&scanner->indents, iter);
        buffer[size++] = (char)(indent_value & 0xFF);
        buffer[size++] = (char)((indent_value >> 8) & 0xFF);
    }
    return size;
}

void tree_sitter_renpy_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
    Scanner *scanner = (Scanner *)payload;
    array_delete(&scanner->indents);
    array_push(&scanner->indents, 0);
    scanner->pending_dedents = 0;
    scanner->eof_newline_done = false;
    if (length > 1) {
        scanner->pending_dedents = (uint8_t)buffer[0];
        scanner->eof_newline_done = buffer[1] != 0;
        for (size_t size = 2; size + 1 < length; size += 2) {
            uint16_t indent_value = (unsigned char)buffer[size] | ((unsigned char)buffer[size + 1] << 8);
            array_push(&scanner->indents, indent_value);
        }
    }
}

// At the end of the file: close open blocks, then end the last line once.
static bool scan_eof(Scanner *scanner, TSLexer *lexer, const bool *valid_symbols) {
    if (valid_symbols[DEDENT] && scanner->indents.size > 1) {
        array_pop(&scanner->indents);
        lexer->result_symbol = DEDENT;
        return true;
    }
    if (valid_symbols[NEWLINE] && !scanner->eof_newline_done) {
        scanner->eof_newline_done = true;
        lexer->result_symbol = NEWLINE;
        return true;
    }
    return false;
}

static bool scan_newline_char(TSLexer *lexer) {
    if (lexer->lookahead == '\n') {
        advance(lexer);
    } else if (lexer->lookahead == '\r') {
        advance(lexer);
        if (lexer->lookahead == '\n') {
            advance(lexer);
        }
    } else {
        return false;
    }
    lexer->mark_end(lexer);
    lexer->result_symbol = NEWLINE;
    return true;
}

bool tree_sitter_renpy_external_scanner_scan(
    void *payload,
    TSLexer *lexer,
    const bool *valid_symbols
) {
    Scanner *scanner = (Scanner *)payload;
    // During error recovery every symbol is valid; never let python_content
    // or a bracket group swallow lines then, or one typo would turn the file
    // into opaque python.
    bool error_recovery = valid_symbols[NEWLINE] && valid_symbols[INDENT] &&
                          valid_symbols[DEDENT] && valid_symbols[PYTHON_CONTENT] &&
                          valid_symbols[BRACKETED];

    // 1. End of file
    if (lexer->eof(lexer)) {
        return scan_eof(scanner, lexer, valid_symbols);
    }

    // 1b. Further DEDENTs for a line that closes several blocks at once
    if (scanner->pending_dedents > 0) {
        if (valid_symbols[DEDENT] && !error_recovery && scanner->indents.size > 1) {
            scanner->pending_dedents--;
            array_pop(&scanner->indents);
            lexer->result_symbol = DEDENT;
            return true;
        }
        scanner->pending_dedents = 0;
    }

    bool at_column_0 = lexer->get_column(lexer) == 0;
    bool indent_valid = valid_symbols[INDENT] || valid_symbols[DEDENT];

    // 2. NEWLINE, after any trailing whitespace. At column 0 the leading
    // whitespace is left for step 3 to measure.
    if (valid_symbols[NEWLINE]) {
        if (!at_column_0 || !indent_valid) {
            while (is_space(lexer->lookahead)) {
                skip(lexer);
            }
            if (lexer->eof(lexer)) {
                return scan_eof(scanner, lexer, valid_symbols);
            }
        }
        if (scan_newline_char(lexer)) {
            return true;
        }
    }

    // 3. INDENT / DEDENT, measured at the start of a line
    if (indent_valid && at_column_0) {
        uint32_t indent_length = 0;
        for (;;) {
            if (lexer->lookahead == ' ') {
                indent_length++;
                skip(lexer);
            } else if (lexer->lookahead == '\t') {
                indent_length += 4;
                skip(lexer);
            } else if (lexer->lookahead == '\f') {
                skip(lexer);
            } else {
                break;
            }
        }

        if (lexer->eof(lexer)) {
            return scan_eof(scanner, lexer, valid_symbols);
        }
        // Blank lines and comment-only lines never change the indentation
        if (lexer->lookahead == '\n' || lexer->lookahead == '\r') {
            return valid_symbols[NEWLINE] && scan_newline_char(lexer);
        }
        if (lexer->lookahead == '#') {
            return false;
        }

        uint16_t current_indent = *array_back(&scanner->indents);

        if (valid_symbols[INDENT] && indent_length > current_indent) {
            array_push(&scanner->indents, indent_length);
            lexer->result_symbol = INDENT;
            return true;
        }

        if (valid_symbols[DEDENT] && indent_length < current_indent) {
            array_pop(&scanner->indents);
            uint8_t owed = 0;
            for (uint32_t i = scanner->indents.size; i > 1; i--) {
                if (*array_get(&scanner->indents, i - 1) <= indent_length || owed == UINT8_MAX) {
                    break;
                }
                owed++;
            }
            scanner->pending_dedents = owed;
            lexer->result_symbol = DEDENT;
            return true;
        }
    }

    // 4. Python logical line (block body lines arrive here with their
    // indentation already skipped above)
    if (valid_symbols[PYTHON_CONTENT] && !error_recovery && scan_python_content(lexer)) {
        lexer->result_symbol = PYTHON_CONTENT;
        return true;
    }

    // 5. A bracket group in screen-language / ATL arguments
    if (valid_symbols[BRACKETED] && !error_recovery) {
        while (is_space(lexer->lookahead)) {
            skip(lexer);
        }
        int32_t c = lexer->lookahead;
        if ((c == '(' || c == '[' || c == '{') && scan_bracketed(lexer)) {
            lexer->result_symbol = BRACKETED;
            return true;
        }
    }
    return false;
}
