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
};

typedef struct {
    Array(uint16_t) indents;
    // Further DEDENTs owed by the line just measured: one line can close
    // several blocks (`return` after a menu choice), but the measurement only
    // happens at column 0, before the first zero-width DEDENT moves us off it.
    uint8_t pending_dedents;
} Scanner;

static inline void advance(TSLexer *lexer) { lexer->advance(lexer, false); }
static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

static inline bool at_line_end(TSLexer *lexer) {
    return lexer->eof(lexer) || lexer->lookahead == '\n' || lexer->lookahead == '\r';
}

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
    uint32_t iter = 1;
    for (; iter < scanner->indents.size && size < TREE_SITTER_SERIALIZATION_BUFFER_SIZE; ++iter) {
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
    if (length > 0) {
        scanner->pending_dedents = (uint8_t)buffer[0];
        for (size_t size = 1; size + 1 < length; size += 2) {
            uint16_t indent_value = (unsigned char)buffer[size] | ((unsigned char)buffer[size + 1] << 8);
            array_push(&scanner->indents, indent_value);
        }
    }
}

bool tree_sitter_renpy_external_scanner_scan(
    void *payload,
    TSLexer *lexer,
    const bool *valid_symbols
) {
    // 0. Scanner declaration
    Scanner *scanner = (Scanner *)payload;
    // During error recovery every symbol is valid; never let python_content
    // swallow lines then, or one typo would turn the file into opaque python.
    bool error_recovery = valid_symbols[NEWLINE] && valid_symbols[INDENT] &&
                          valid_symbols[DEDENT] && valid_symbols[PYTHON_CONTENT];
    // 1. Handle EOF
    // If file ended but still had open bloc, force end it with DEDENT
    if (lexer->eof(lexer)) {
        if (valid_symbols[DEDENT] && scanner->indents.size > 1) {
            array_pop(&scanner->indents);
            lexer->result_symbol = DEDENT;
            return true;
        }
        return false;
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

    // 2. Handle NEWLINE
    // If parser ask NEWLINE token and we find \n or \r character
    if (valid_symbols[NEWLINE]) {
        if (lexer->lookahead == '\n') {
            advance(lexer);
            lexer->mark_end(lexer);
            lexer->result_symbol = NEWLINE;
            return true;
        }
        if (lexer->lookahead == '\r') {
            advance(lexer);
            if (lexer->lookahead == '\n') {
                advance(lexer);
            }
            lexer->mark_end(lexer);
            lexer->result_symbol = NEWLINE;
            return true;
        }
    }

    // 3. Handle INDENT & DEDENT
    // Vital Requirement: We only count space if cursor really exist
    // on the left edge of line (column 0) and parser asks INDENT/DEDENT Token
    if ((valid_symbols[INDENT] || valid_symbols[DEDENT]) && lexer->get_column(lexer) == 0) {
        uint32_t indent_length = 0;

        // skip all space and tabs with counting them
        for (;;) {
            if (lexer->lookahead == ' ') {
                indent_length++;
                skip(lexer);
            } else if (lexer->lookahead == '\t') {
                indent_length += 4;
                skip(lexer);
            } else {
                break;
            }
        }

        //ignore empty line or comment line
        // don't print INDENT/DEDENT for line without any content
        if (lexer->lookahead == '\n' || lexer->lookahead == '\r' || lexer->lookahead == '#') {
            return false;
        }

        // compare current indentation with previous indentation (top of stack)
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
    return false;
}