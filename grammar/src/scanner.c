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
};

typedef struct {
    Array(uint16_t) indents;
} Scanner;

static inline void advance(TSLexer *lexer) { lexer->advance(lexer, false); }
static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }

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
    if (length > 0) {
        for (size_t size = 0; size + 1 < length; size += 2) {
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
            lexer->result_symbol = DEDENT;
            return true;
        }
    }
    return false;
}