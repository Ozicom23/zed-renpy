// A quoted string, parsed into Ren'Py text parts. `content` is a run of plain
// characters for this quote style.
function stringOf($, quote, content) {
  return seq(
    token(seq(optional(/[rRuU]/), quote)),
    repeat(choice(
      token.immediate(prec(1, content)),
      token.immediate(prec(1, /[\[{]/)), // a bracket that opens no tag
      $.escape_sequence,
      $.interpolation,
      $.text_tag,
    )),
    token.immediate(quote),
  );
}

function tripleStringOf($, quote, content) {
  const triple = quote.repeat(3);
  return seq(
    token(seq(optional(/[rRuU]/), triple)),
    repeat(choice(
      token.immediate(prec(1, content)),
      token.immediate(prec(1, /[\[{]/)),
      token.immediate(prec(1, quote)), // a lone quote inside
      $.escape_sequence,
      $.interpolation,
      $.text_tag,
    )),
    token.immediate(prec(1, triple)),
  );
}

// Arguments of a screen-language, ATL or style line: names, literals,
// operators and bracket groups, up to the `:` or the end of the line.
function args($) {
  return repeat(choice(
    $.identifier,
    $.string,
    $.number,
    $.bracketed,
    $.operator,
    ',',
    '.',
  ));
}

function elementBlock($) {
  return seq(
    repeat($._newline),
    $._indent,
    repeat1(choice(
      $.element,
      $.python_line,
      $.python_block,
      $.default_statement,
      $._newline,
    )),
    $._dedent,
  );
}

module.exports = grammar({
  name: 'renpy',

  externals: $ => [
    $._newline,
    $._indent,
    $._dedent,
    // A Python logical line: runs to end of line, but continues across
    // newlines inside open brackets and triple-quoted strings, as Python
    // (and Ren'Py) do. Lexed in src/scanner.c.
    $.python_content,
    // One balanced (...) / [...] / {...} group, which may span lines while
    // open. Arguments in screen language, ATL and style blocks.
    $.bracketed,
  ],

  extras: $ => [
    /[ \t]/,  // only spaces and tabs, NOT newlines
    $.comment, // comment would also categorized as extras
  ],

  conflicts: $ => [
    [$.label_statement], // it use optional($.block) in its structure so that's why
    // a line starting with a name may be dialogue or a creator-defined
    // statement until the parser sees more of it
    [$.dotted_name, $.user_statement],
  ],


  rules: {
    source_file: $ => repeat($._statement),

    _statement: $ => choice(
      // ── Basics & Helpers
      $._newline,

      // ▓▓▓▓▓ CHAPTER 1 ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓

      // ── Section 2: Control Flow
      $.label_statement,
      $.jump_statement,
      $.call_statement,
      $.return_statement,
      $.pause_statement,

      // ── Section 3: Dialogue & Character
      $.say_statement,
      $.define_statement,
      $.default_statement,
      $.window_statement,
      $.rpy_statement,

      // ── Section 4: Displaying Images
      $.image_statement,
      $.show_statement,
      $.scene_statement,
      $.hide_statement,
      $.with_statement,
      $.camera_statement,
      $.show_layer_statement,

      // ── Section 5: Menus
      $.menu_statement,

      // ── Section 6: Python
      $.python_block,
      $.python_line,
      $.init_statement,

      // ── Section 7: Conditionals
      $.if_statement,
      $.while_statement,
      $.pass_statement,

      // ── Section 8: Audio
      $.play_statement,
      $.stop_statement,
      $.queue_statement,

      // ── Section 11: Voice
      $.voice_statement,

      // ── Chapter 2: screens, transforms, styles, translations
      $.screen_statement,
      $.show_screen_statement,
      $.hide_screen_statement,
      $.call_screen_statement,
      $.transform_statement,
      $.style_statement,
      $.translate_statement,
      $.layeredimage_statement,
      $.testcase_statement,
      $.nvl_statement,

      // anything else that looks like a statement: creator-defined
      // statements registered with renpy.register_statement
      $.user_statement,
    ),

    // ── Blocks ───────────────────────────────────────────────

    block: $ => seq(
      repeat($._newline),
      $._indent,
      repeat1($._statement),
      $._dedent,
    ),

    // ── Expression Helpers ───────────────────────────────────────────────

    // it helps to covers Python syntax parts individually
    _python_tokens: $ => choice(
      $.identifier,
      $.string,
      $.number,
      /[+\-*/%&|^=<>!~]+/, // mathematic operator and logic
      /[()\[\]{},.]/ // parenthesses and punctuation marks
    ),

    // combination of tokens above. It will stops read while meets Ren'Py keywords such as 'pass' or 'from'
    simple_expression: $ => repeat1($._python_tokens),


    // opaque block for raw python content
    indented_block: $ => seq(
      repeat($._newline),
      $._indent,
      repeat1(choice(
        seq($.python_content, $._newline),
        $._newline // letting a line only contains \n (empty line)
      )),
      $._dedent,
    ),

    //any text followed not by double dot (:) or enter (\n)
    python_expression: $ => /[^:\n]+/,

    rpy_statement: $ => seq(
      'rpy',
      repeat1(choice($.identifier, $.number)), // intended to be able handling repeated clauses such as 'monologue single', 'monologue none', or any rpy rules on future
      $._newline,
    ),

    // ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓
    //
    //                      CHAPTER 1: THE REN'PY LANGUAGE
    //                   (It covers most of Ren'Py syntaxes)
    //
    // ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓

    // =========================================================================
    // SECTION 1: LANGUAGE BASICS & PRIMITIVES
    // (Comment, String, Identifier, Number)
    // =========================================================================

    identifier: $ => /[a-zA-Z_\u00a0-\ufffd][a-zA-Z0-9_\u00a0-\ufffd]*/,

    dotted_name: $ => seq(
      $.identifier,
      repeat(seq('.', $.identifier)),
    ),

    // Strings are parsed into their parts so that Ren'Py text features can
    // be highlighted: `[interpolation]`, `{text tags}` and escapes
    // (`\n`, `[[`, `{{`). Dialogue strings may span lines.
    string: $ => choice(
      stringOf($, '"', /[^"\\\[{]+/),
      stringOf($, "'", /[^'\\\[{]+/),
      tripleStringOf($, '"', /[^"\\\[{]+/),
      tripleStringOf($, "'", /[^'\\\[{]+/),
      // backtick strings stay opaque
      token(seq(optional(/[rRuU]/), '`', /([^`\\]|\\.)*/, '`')),
    ),

    escape_sequence: $ => token.immediate(prec(1, choice(/\\(.|\n)/, '[[', '{{'))),

    interpolation: $ => token.immediate(prec(1, /\[[^\[\]\n]+\]/)),

    text_tag: $ => token.immediate(prec(1, /\{[^{}\n]+\}/)),

    comment: $ => /#[^\n]*/,

    number: $ => /\d+(\.\d+)?/,

    // =========================================================================
    // SECTION 2: LABEL & CONTROL FLOW FUNCTIONS
    // (Label Statement, Jump Statement, Call Statement, Return Statement, Special Labels)
    // =========================================================================

    // ── 2.1. Label Statement ────────────────────────

    label_statement: $ => seq(
      'label',
      field('name', $.label_name),
      optional($.parameters), // allows declaration of (a="default")
      ':',
      $._newline,
      optional($.block),
    ),

    label_name: $ => choice(
      $.dotted_name,  // it covers 'single_label' or 'global_label.local_label'
      seq('.', $.identifier), // if the second condition applied, it covers 'local_label'
    ),

    // `(a, b=f(1))`: a balanced bracket group, which may span lines
    parameters: $ => $.bracketed,

    // ── 2.2. Jump Statement ────────────────────────

    jump_statement: $ => seq(
      'jump',
      choice(
        field('target', $.label_name), // scenario 1: jump followed by static label
        seq('expression', field('expression', $.simple_expression)), // scenario 2: jump followed by expression
      ),
      $._newline,
    ),

    // ── 2.3. Call Statement ────────────────────────

    call_statement: $ => seq(
      'call',
      choice(
        // Scenario 1 - Static (it also can use parameters)
        // ex: call subroutine OR call subroutine(2)
        seq(
          field('target', $.label_name),
          optional($.parameters),
        ),
        // Scenario 2 - Expression (it should use pass if parameter exists)
        // ex: call expression "sub" + "routine" pass (count=3)
        seq(
          'expression',
          field('expression', $.simple_expression),
          optional(seq('pass', $.parameters)),
        ),
      ),

      // Scenario 3 - From clause added on the end of line (optionally, of course)
      // ex: from _call_subroutine_1
      optional(seq('from', field('from', $.label_name))),

      $._newline,
    ),

    // ── 2.4. Return Statement ────────────────────────

    return_statement: $ => seq(
      'return',
      optional(field('value', $.python_content)),
      $._newline,
    ),

    // ── 2.4.5. Pause Statement ───────────────────────

    pause_statement: $ => seq(
      'pause',
      optional(field('duration', $.simple_expression)),
      $._newline,
    ),

    // ── 2.5. Special Labels ───────────────────────────

    // NOTE: Special Labels (start, quit) and Special Characters (adv, nvl, extend)
    // are syntactically standard words. They are fully covered by $.identifier.
    // Their special behaviors are semantic and handled by the Ren'Py engine.

    // ── 2.6. Label & Control Flow Functions ───────────

    // NOTE: Ren'Py built-in Python functions (renpy.*), Contexts,
    // and Statement Equivalents are automatically covered by $.python_line
    // and $.python_content via language injection. No specific rules needed.

    // ── 2.7. Contexts ──────────────────────────────────

    // NOTE: Ren'Py built-in Python functions (renpy.*), Contexts,
    // and Statement Equivalents are automatically covered by $.python_line
    // and $.python_content via language injection. No specific rules needed.

    // =========================================================================
    // SECTION 3: DIALOGUE AND NARRATION
    // (Say Statement, Character Object Definition, Say with Image Attributes,
    // Special Characters, Special Labels)
    // =========================================================================

    // ── 3.1. Say Statement ────────────────────────
    say_statement: $ => seq(
      choice(
        // case 1 - Narration
        // ex: "This is narration line."
        field('what', $.string),

        // case 2 - Explicit character name
        // ex: "Eileen" "This is Eileen's dialogue."
        seq(
          field('who', $.string),
          field('what', $.string),
        ),

        // case 3 - Character object (identifier or dotted_name)
        // ex: e "This is Eileen's dialogue." OR mc.name "Hello!"
        seq(
          // `nvl` is also a statement keyword (`nvl clear`); the token
          // after it decides which one this is
          field('who', choice($.dotted_name, alias('nvl', $.dotted_name))),
          // a translated speaker name: _("Farmers") "Let's drink."
          optional(field('who_arguments', $.bracketed)),
          optional(field('attributes', $.say_attributes)), // to cover Subsection 3.3.
          field('what', $.string),
        ),
      ),

      // to cover Subsection 3.6.
      optional(field('arguments', $.parameters)),

      // translation identifier: e "Hello." id start_a170b500
      optional(seq('id', field('id', $.identifier))),

      // case 4 - optional 'with' clause in the end of line
      // ex: "Bam!!" with vpunch
      optional(seq('with', field('transition', $.simple_expression))),

      $._newline,
    ),

    // ── 3.2. Defining Character Objects ────────────────────────────

    // expanded version from dotted_name to covers indexing (ex: a.b["c"])
    variable_name: $ => seq(
      $.identifier,
      repeat(choice(
        seq('.', $.identifier),
        seq('[', /[^\]\n]+/, ']')
      ))
    ),

    define_statement: $ => seq(
      'define',
      // Optional priority number (support negatives as -1)
      optional(field('priority', /-?\d+/)),
      // changed to $.variable_name to support indexing as documented in Subsection 6.4.
      field('name', $.variable_name),
      // expanded operators (such as '+=' OR '|=') to support indexing as documented in Subsection 6.4.
      field('operator', choice('=', '+=', '|=')),
      field('value', $.python_content),
      $._newline,
    ),

    default_statement: $ => seq(
      'default',
      field('name', $.variable_name),
      '=',
      field('value', $.python_content),
      $._newline,
    ),

    // NOTE: Character definition via Python built-in syntaxes
    // (ex: Character("Eileen", color="#000")) are covered by
    // $.python_line and $.python_content via language injection.
    // No specific rules needed.

    // ── 3.3. Say with Image Attributes ────────────────────────

    say_attribute: $ => choice(
      $.identifier,
      seq('-', $.identifier),
      '@'
    ),

    say_attributes: $ => repeat1($.say_attribute),

    // the implementation is covered in Subsection 3.1.

    // ── 3.4. Special Characters ──────────────────────────────

    // covered by $.identifier at general
    //
    // Anyway, 'extend' will treated as variable as e OR mc
    // since it was covered on Subsection 3.1. code
    // in function field('who', $.identifier).
    // It decided to be left since parser only need to understand
    // it as some different syntaxes, so the function understanding and
    // implementation (semantic) will be left to Ren'Py Engine.
    //
    // But, if you think we can improve it further, just improve the
    // logic on this grammar or in the highlights.scm on the extension
    // repository.

    // ── 3.5. Dialogue Window Management  ──────────────────────

    window_statement: $ => seq(
      'window',
      choice(
        // 1. window show [transition]
        seq(
          'show',
          // usage of $.simple_expression is to cover Subsection 4.7.
          optional(field('transition', $.simple_expression))
        ),

        // 2. window hide [transition]
        seq(
          'hide',
          // usage of $.simple_expression is to cover Subsection 4.7.
          optional(field('transition', $.simple_expression))
        ),

        // 3. window auto [True|False|show|hide] [transition]
        seq(
          'auto',
          // sometimes some developer only writes 'window auto' on older version
          optional(choice('True', 'False', 'show', 'hide')),
          optional(field('transition', $.simple_expression))
        ),
      ),
      $._newline,
    ),

    // ── 3.6. Say with Arguments  ───────────────────────────────

    // covered in Subsection 3.1. code

    // ── 3.7. Monologue Mode  ───────────────────────────────────

    // covered in string tokenization code
    //
    // Anyway, rpy keyword now handled on the simple code above
    // per chapter division. Further development is needed if it
    // still not covers the rpy logic.

    // ── 3.7. The 'character' Store  ─────────────────────────────

    // covered in dotted_name code
    //
    // As explained in some subsections, 'e' is treated as 'eileen'
    // (an identifer). This tree-sitter is only parsing the syntaxes
    // and not caring the semantic. Semantic things is Ren'Py Engine
    // task and not this grammar nor extension task.

    // =========================================================================
    // SECTION 4: DISPLAYING IMAGES
    // (Concept, Defining Images, Show Statement, Scene Statement, Hide
    // Statement, With Statement, Camera and Show Layer Statements, Hide
    // and Show Window, Image Functions)
    // =========================================================================

    // ── 4.1. Concept  ─────────────────────────────────────────

    image_name: $ => prec.left(repeat1($.identifier)),

    // ── 4.2. Defining Images  ─────────────────────────────────

    image_statement: $ => seq(
      'image',
      field('name', $.image_name),
      choice(
        seq('=', field('displayable', $.python_content)),
        // image eileen happy:  (an ATL image)
        seq(':', $._newline, field('atl', $.atl_block)),
      ),
    ),

    // ── 4.3. Show Statement  ──────────────────────────────────

    // capture images and attributes written in a single line
    image_specifier: $ => prec.right(repeat1(choice(
      $.identifier,
      seq('-', $.identifier),
      $.string, // show text "Hello"
    ))),

    // capture optional clauses of show/scene statement
    show_property: $ => choice(
      seq('as', field('alias', $.identifier)),
      seq('at', field('transforms', $.simple_expression)),
      seq('behind', field('targets', $.simple_expression)),
      seq('onlayer', field('layer', $.identifier)),
      seq('zorder', field('zorder', $.simple_expression)),
    ),

    show_statement: $ => seq(
      'show',
      choice(
        // Case 1: static. ex: show mary night -happy
        field('image', $.image_specifier),

        // Case 2: dynamic. Ex: show expression "moon.png"
        seq('expression', field('expression', $.simple_expression))
      ),

      // repeat() used since properties can be declared 0 times or multiple times
      repeat($.show_property),

      // optional With clause
      optional(seq('with', field('transition', $.simple_expression))),

      $._atl_or_newline,
    ),

    // ends a show/scene line, or opens its ATL block
    _atl_or_newline: $ => choice(
      $._newline,
      seq(':', $._newline, field('atl', $.atl_block)),
    ),

    // ── 4.3. Scene Statement  ─────────────────────────────────

    scene_statement: $ => seq(
      'scene',

      // image decision declared as optional() to cover "clearing a layer"
      optional(
        choice(
          // Case 1: Static
          field('image', $.image_specifier),

          // Case 2: Dynamic
          seq('expression', field('expression', $.simple_expression))
        )
      ),

      // repeat() used since properties can be declared 0 times or multiple times
      repeat($.show_property),

      // optional With clause
      optional(seq('with', field('transition', $.simple_expression))),

      $._atl_or_newline,
    ),

    // ── 4.4. Hide Statement  ──────────────────────────────────

    hide_statement: $ => seq(
      'hide',
      // hiding an image is not need minus (ex: -happy) so it just cover 1 case
      field('image', $.image_name),
      // repeat() used since properties can be declared 0 times or multiple times
      repeat($.show_property),

      // optional With clause
      optional(seq('with', field('transition', $.simple_expression))),

      $._newline,
    ),

    // ── 4.5. With Statement ──────────────────────────────────

    with_statement: $ => seq(
      'with',
      field('transition', $.simple_expression),
      $._newline,
    ),

    // ── 4.6. Camera and Show Layer Statements ─────────────────

    camera_statement: $ => seq(
      'camera',
      // layer declaration is optional
      optional(field('layer', $.identifier)),
      // optional at clause
      optional(
        seq(
          'at',
          field('transforms', $.simple_expression)
        )
      ),
      // can just ended with enter, or followed by colon and ATL block
      $._atl_or_newline,
    ),

    show_layer_statement: $ => prec(1, seq(
      'show',
      'layer',
      // layer declaration must exists
      field('layer', $.identifier),
      // optional at clause
      optional(seq('at', field('transforms', $.simple_expression))),
      // can just ended with enter, or followed by colon and ATL block
      $._atl_or_newline,
    )),

    // ── 4.7. Hide and Show Window ─────────────────────────────

    // covered in Subsection 3.5. code

    // ── 4.8. Image Functions ──────────────────────────────────

    // NOTE: Ren'Py built-in Python functions (renpy.*), Contexts,
    // and Statement Equivalents are automatically covered by $.python_line
    // and $.python_content via language injection. No specific rules needed.

    // =========================================================================
    // SECTION 5: IN-GAME MENUS
    // (Menu Set, Menu Arguments)
    // =========================================================================

    menu_statement: $ => seq(
      'menu',
      // optional label name for menu
      optional(field('name', $.label_name)),
      // optional argument to support Subsection 5.2. Ex: (screen="airport")
      optional(field('arguments', $.parameters)),
      ':',
      $._newline,
      repeat($._newline),
      $._indent,
      // menu block can contains choices, dialogue, or just an empty line
      repeat1(choice(
        $.menu_choice,
        $.menu_set, // Subsection 5.1. coverage
        $.say_statement,
        $._newline
      )),
      $._dedent,
    ),

    menu_choice: $ => seq(
      $.string,
      // optional argument to support Subsection 5.2. Ex: (150, sale=True)
      optional(field('arguments', $.parameters)),
      // optional 'if' clause for conditional choices
      optional(seq('if', field('condition', $.python_expression))),
      ':',
      $._newline,
      $.block,
    ),

    // ── 5.1. Menu Set ────────────────────────────────────────

    menu_set: $ => seq(
      'set',
      field('set', $.python_expression), // uses $.python_expression to cover Subsection 5.2.
      $._newline,
    ),

    // ── 5.2. Menu Arguments ──────────────────────────────────

    // Covered in Section 5 introduction and Subsection 5.1. codes

    // =========================================================================
    // SECTION 6: PYTHON STATEMENTS
    // (Python, One-line Python Statement, Init Python Statement, Define
    // Statement, Default Statement, Names in the Store, Other Named Stores,
    // Constant Stores, JSONDB)
    // =========================================================================


    // ── 6.1. Python ──────────────────────────────────────────
    // ── Python Expression Syntax Coverage ────────────────────

    python_block: $ => seq(
      'python',
      optional($._python_modifiers),
      ':',
      $._newline,
      $.indented_block,
    ),

    // `early`, `hide` and `in <store>`, in any order
    _python_modifiers: $ => repeat1(choice(
      'early',
      'hide',
      seq('in', field('store', $.identifier)),
    )),

    // ── 6.2. One-line Python Statement ───────────────────────

    python_line: $ => seq(
      '$',
      $.python_content,
      $._newline,
    ),

    // ── 6.3. Init Python Statement ───────────────────────────

    init_statement: $ => choice(
      // init offset = -1
      seq('init', 'offset', '=', field('offset', /-?\d+/), $._newline),

      // init [priority] followed by one statement: init label x:, init -1 style
      seq(
        'init',
        optional(field('priority', /-?\d+/)),
        choice(
          $.label_statement,
          $.screen_statement,
          $.transform_statement,
          $.style_statement,
          $.image_statement,
          $.define_statement,
          $.default_statement,
        ),
      ),

      // Case 1 : standard init declaration (ex: 'init:' OR 'init 5:')
      seq(
        'init',
        optional(field('priority', /-?\d+/)),
        ':',
        $._newline,
        $.block
      ),

      // Case 2 : init Python (ex: 'init:' OR 'init 5:')
      seq(
        'init',
        optional(field('priority', /-?\d+/)),
        'python',
        optional($._python_modifiers),
        ':',
        $._newline,
        $.indented_block
      ),
    ),

    // ── 6.4. Define Statement ────────────────────────────────

    // Covered in Subsection 3.2.

    // ── 6.5. Default Statement ───────────────────────────────

    // Covered in Subsection 3.2.

    // ── 6.6. Names in the Store ──────────────────────────────

    // NOTE: The 'in' clause for named stores is already covered in $.python_block
    // and $.init_statement. Complex variable namings are covered by $.variable_name.
    // Variable naming rules (e.g., avoiding reserved names or constant store
    // enforcements) are semantic rules handled by the Ren'Py engine.
    // This grammar only parses the syntaxes.

    // ── 6.7. Other Named Stores ──────────────────────────────

    // NOTE: The 'in' clause for named stores is already covered in $.python_block
    // and $.init_statement. Complex variable namings are covered by $.variable_name.
    // Variable naming rules (e.g., avoiding reserved names or constant store
    // enforcements) are semantic rules handled by the Ren'Py engine.
    // This grammar only parses the syntaxes.

    // ── 6.8. Constant Stores ─────────────────────────────────

    // NOTE: The 'in' clause for named stores is already covered in $.python_block
    // and $.init_statement. Complex variable namings are covered by $.variable_name.
    // Variable naming rules (e.g., avoiding reserved names or constant store
    // enforcements) are semantic rules handled by the Ren'Py engine.
    // This grammar only parses the syntaxes.

    // ── 6.9. JSONDB ──────────────────────────────────────────

    // NOTE: JSONDB instantiations, Python module imports (e.g., 'import dateutil'),
    // and internal Python classes (python_dict, python_list, python_set) are
    // pure Python semantics. They are fully covered by $.define_statement,
    // $.init_statement, and handled via Python language injection in
    // $.python_content and $.indented_block. No new syntax rules are required.

    // ── 6.10. First & Third-Party Python Modules & Packages ──

    // NOTE: JSONDB instantiations, Python module imports (e.g., 'import dateutil'),
    // and internal Python classes (python_dict, python_list, python_set) are
    // pure Python semantics. They are fully covered by $.define_statement,
    // $.init_statement, and handled via Python language injection in
    // $.python_content and $.indented_block. No new syntax rules are required.

    // ── 6.11. Rollback and Isinstance ────────────────────────

    // NOTE: JSONDB instantiations, Python module imports (e.g., 'import dateutil'),
    // and internal Python classes (python_dict, python_list, python_set) are
    // pure Python semantics. They are fully covered by $.define_statement,
    // $.init_statement, and handled via Python language injection in
    // $.python_content and $.indented_block. No new syntax rules are required.

    // =========================================================================
    // SECTION 7: CONDITONAL STATEMENTS
    // (If Statement, While Statement, Pass Statement)
    // =========================================================================

    // ── 7.1. If Statement ────────────────────────────────────

    if_statement: $ => seq(
      'if',
      field('condition', $.python_expression),
      ':',
      $._newline,
      $.block,
      repeat($.elif_clause),
      optional($.else_clause),
    ),

    elif_clause: $ => seq(
      'elif',
      field('condition', $.python_expression),
      ':',
      $._newline,
      $.block,
    ),

    else_clause: $ => seq(
      'else',
      ':',
      $._newline,
      $.block,
    ),

    // ── 7.2. While Statement ─────────────────────────────────

    while_statement: $ => seq(
      'while',
      field('condition', $.python_expression),
      ':',
      $._newline,
      $.block,
    ),

    // ── 7.3. Pass Statement ──────────────────────────────────

    pass_statement: $ => seq(
      'pass',
      $._newline,
    ),

    // =========================================================================
    // SECTION 8: AUDIO
    // (Play Statement, Stop Statement, Queue Statement)
    // =========================================================================

    // ── 8.1. Play Statement ──────────────────────────────────

    play_statement: $ => seq(
      'play',
      field('channel', $.identifier),
      // updated to $.simple_expression to be able handle string, variable,
      // or list (Ex: '["a.ogg", "b.ogg"]')
      field('file', $.simple_expression),

      // handle special keywords. Ex: "fadeout 1.0"
      repeat(choice(
        seq('fadein', field('fadein', $.simple_expression)),
        seq('fadeout', field('fadeout', $.simple_expression)),
        seq('volume', field('volume', $.simple_expression)),
        'loop',
        'noloop',
        'if_changed'
      )),
      $._newline,
    ),


    // ── 8.2. Stop Statement ──────────────────────────────────

    stop_statement: $ => seq(
      'stop',
      field('channel', $.identifier),
      optional(seq('fadeout', field('fadeout', $.simple_expression))),
      $._newline,
    ),

    // ── 8.3. Queue Statement ─────────────────────────────────

    queue_statement: $ => seq(
      'queue',
      field('channel', $.identifier),
      // updated to $.simple_expression to be able handle string, variable,
      // or list (Ex: '["a.ogg", "b.ogg"]')
      field('file', $.simple_expression),
      // handle special keywords as "fadeout 1.0"
      repeat(choice(
        seq('fadein', field('fadein', $.simple_expression)),
        seq('volume', field('volume', $.simple_expression)),
        'loop',
        'noloop',
      )),
      $._newline,
    ),

    // NOTE: Partial playback specs (e.g., "<from 5>file.ogg") are standard strings.
    // Audio functions (renpy.music.*, renpy.sound.*, AudioData) are pure Python
    // semantics covered by $.python_line and $.python_content.
    //
    // If you think you can improve it (e.g. making the partial playback specs can be
    // highlighted with specific color to differ it from standard string), you can
    //  freely contribute on this grammar development.
    
    // =========================================================================
    // SECTION 9: AUDIO FILTERS
    // (Filter Reuse, Silence Padding, Audio Filters)
    // =========================================================================

    // NOTE: Audio filters (renpy.audio.filter.*) are treated as pure Python classes and
    // functions. They are accessed via python statements (e.g., $ renpy.music.set_audio_filter(...))
    // and are fully covered by $.python_line and $.python_content via language injection.
    // No specific syntax rules are needed.
    //
    // Additionally, playback specifications like Silence Padding (e.g., "<silence 10>")
    // are parsed simply as standard strings within $.simple_expression. While they act as
    // injection instructions for the Ren'Py engine, parsing their internal structure here
    // would add unnecessary complexity to the AST. Specific syntax highlighting for these
    // tags is better handled via regex queries (e.g., highlights.scm) on the extension side.
    // However, if you see a better way to implement this at the grammar level,
    // you are welcome to freely contribute to this repository!

    // =========================================================================
    // SECTION 10: MOVIE
    // (Fullscreen Movies, Movie Displayables, Python Functions)
    // =========================================================================

    // NOTE: Movie playback in Ren'Py relies on existing syntaxes. Fullscreen movies
    // use python functions (e.g., $ renpy.movie_cutscene(...)) covered by $.python_line.
    // Movie sprites are defined via the $.image_statement (e.g., image bg = Movie(...))
    // and displayed using the standard $.show_statement. No new syntax rules are needed.

    // =========================================================================
    // SECTION 11: VOICE
    // (Voice Tags, Automatic Voice, Functions, Actions)
    // =========================================================================

    voice_statement: $ => seq(
      'voice',
      choice(
        'sustain',
        field('file', $.simple_expression)
      ),
      $._newline,
    ),

    // NOTE: Voice tags definition (e.g., Character(..., voice_tag="eileen")), 
    // automatic voice config, Voice Functions, and Voice Actions are pure Python
    // semantics and fully covered by $.python_line and $.python_content.

    // ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓
    //
    //         CHAPTER 2: Text, Displayables, Transforms, and Transitions
    //                               (WIP For v0.5.0)
    //
    // ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓

    // Screen language, ATL, style, layered-image and testcase blocks share one
    // shape: a line is a name (`vbox`, `xalign`, `linear`, `"image.png"`)
    // followed by arguments, optionally opening a nested block with `:`.
    // Highlighting tells keywords from properties by name.

    operator: $ => /[+\-*\/%&|^=<>!~@]+/,

    element: $ => seq(
      field('name', choice($.identifier, $.string, $.number)),
      args($),
      choice(
        $._newline,
        seq(':', $._newline, field('body', $.element_block)),
      ),
    ),

    element_block: $ => elementBlock($),

    // ATL is the same shape; its own node so queries can tell it apart.
    atl_block: $ => elementBlock($),

    // ── Screens ──────────────────────────────────────────────

    screen_statement: $ => seq(
      'screen',
      field('name', $.identifier),
      optional(field('parameters', $.bracketed)),
      ':',
      $._newline,
      field('body', $.element_block),
    ),

    show_screen_statement: $ => seq(
      'show',
      'screen',
      field('name', $.identifier),
      args($),
      $._newline,
    ),

    hide_screen_statement: $ => seq(
      'hide',
      'screen',
      field('name', $.identifier),
      args($),
      $._newline,
    ),

    call_screen_statement: $ => seq(
      'call',
      'screen',
      field('name', $.identifier),
      args($),
      $._newline,
    ),

    // ── Transforms ───────────────────────────────────────────

    transform_statement: $ => seq(
      'transform',
      field('name', $.dotted_name),
      optional(field('parameters', $.bracketed)),
      ':',
      $._newline,
      field('body', $.atl_block),
    ),

    // ── Styles ───────────────────────────────────────────────

    style_statement: $ => seq(
      'style',
      field('name', $.identifier),
      optional(seq('is', field('parent', $.identifier))),
      args($),
      choice(
        $._newline,
        seq(':', $._newline, field('body', $.element_block)),
      ),
    ),

    // ── Translations ─────────────────────────────────────────

    translate_statement: $ => seq(
      'translate',
      field('language', $.identifier),
      choice(
        // translate french start_a170b500:  (translated dialogue)
        seq(field('id', $.identifier), ':', $._newline, $.block),
        // translate french strings:  (old/new pairs)
        seq('strings', ':', $._newline, $.translate_strings_block),
        // translate french python:
        seq('python', ':', $._newline, $.indented_block),
        // translate french style default:
        seq(
          'style',
          field('style', $.identifier),
          args($),
          choice($._newline, seq(':', $._newline, field('body', $.element_block))),
        ),
      ),
    ),

    translate_strings_block: $ => seq(
      repeat($._newline),
      $._indent,
      repeat1(choice($.translation_string, $._newline)),
      $._dedent,
    ),

    translation_string: $ => seq(
      field('kind', choice('old', 'new')),
      field('text', $.string),
      $._newline,
    ),

    // ── Layered images and testcases ─────────────────────────

    layeredimage_statement: $ => seq(
      'layeredimage',
      field('name', $.image_name),
      ':',
      $._newline,
      field('body', $.element_block),
    ),

    testcase_statement: $ => seq(
      'testcase',
      field('name', $.identifier),
      ':',
      $._newline,
      field('body', $.element_block),
    ),

    // A creator-defined statement (`achieve first_kiss`, `example:` + block).
    // Only chosen when no built-in statement parses the line.
    user_statement: $ => prec.dynamic(-1, seq(
      field('name', $.identifier),
      args($),
      choice(
        $._newline,
        seq(':', $._newline, field('body', $.block)),
      ),
    )),

    // ── NVL mode ─────────────────────────────────────────────

    nvl_statement: $ => seq(
      'nvl',
      choice('clear', 'show', 'hide'),
      optional(field('transition', $.simple_expression)),
      $._newline,
    ),
  }
});
