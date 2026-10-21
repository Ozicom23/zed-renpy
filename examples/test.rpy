# Ren'Py extension smoke test: exercises every construct the grammar supports.

define e = Character("Eileen", color="#c8ffc8")
define config.name = "Zed Demo"
default points = 0

image bg room = "images/bg_room.png"
image eileen happy = "images/eileen_happy.png"

init python:
    def add_points(amount):
        global points
        points += amount

label start:
    scene bg room with dissolve
    play music "audio/theme.ogg" fadein 1.0 loop
    window auto True

    "Welcome to the demo."
    show eileen happy at right
    e "Hi, I'm Eileen!"
    e happy "You have [points] points. {b}Nice.{/b}"
    e @ happy "This attribute only lasts one line."
    "Eileen" "A quoted speaker works too." with vpunch

    $ add_points(5)
    pause 0.5

    menu:
        "What should we do next?"

        "Visit the shop" if points >= 5:
            call shop from _call_shop
            jump ending
        "Do nothing":
            jump ending

label shop:
    e "Welcome to the shop."
    if points > 10:
        e "So rich!"
    elif points > 0:
        e "A modest sum."
    else:
        e "Flat broke."

    while points > 0:
        $ points -= 1

    voice "voice/eileen_bye.ogg"
    e "Goodbye!"
    hide eileen with dissolve
    stop music fadeout 0.5
    return

label ending:
    scene expression "black" with fade
    "The end."
    return

# --- Multi-line python values: a statement continues while a bracket or a
# --- triple-quoted string is open (regression: used to break highlighting
# --- for the rest of the file)
define gui.about = _p("""
About text, with a "quote".

Second paragraph.
""")

define narrator2 = Character(None,
    what_italic=True,
    what_color="#c8c8ff")

default party = [
    "Ciri",
    "Geralt",
]

# --- One line closing several blocks at once (regression: only the first
# --- DEDENT used to be emitted)
label double_dedent:
    menu:
        "Stay":
            $ stayed = True
    return

init python:
    ## a comment-led python block
    after_double_dedent = True

# --- Text inside dialogue: interpolation, text tags, escapes
label text_parts:
    e "Hello, [player_name]! You have {b}[gold]{/b} gold.{w=0.5} \"Quoted\" [[not interpolated] {{not a tag}"
    nvl "NVL narration."
    nvl clear
    _("Farmers") "A translated speaker name."
    e "With a translation id." id text_parts_1a2b3c4d
    window auto hide
    pause short_pause
    show eileen happy at left:
        xalign 0.0
        linear 1.0 xalign 1.0
    show screen inventory(["sword"], gold=10) with dissolve
    call screen confirm(_("Sure?"), yes_action=Return(True))
    hide screen inventory
    return

# --- Screen language
screen inventory(items, gold=0):
    tag menu
    modal True
    default page = 0
    frame:
        xalign 0.5 yalign 0.5
        has vbox spacing 10
        for i in items:
            textbutton i action [SetScreenVariable("page", 1),
                                 Return(i)]
        text _("Gold: [gold]") size 24 color "#fc0"
        if gold > 10:
            add "coin.png" at bounce
        else:
            null height 20
        $ total = len(items)
        python:
            page_count = total // 10

# --- ATL and transforms
transform bounce(height=20):
    yoffset 0
    easein 0.3 yoffset -height
    easeout 0.3 yoffset 0
    repeat

image eileen wave:
    "eileen_wave_1.png"
    pause 0.2
    "eileen_wave_2.png"
    pause 0.2
    repeat

# --- Styles
style menu_button is button:
    hover_color "#fff"
    idle_background Frame("gui/button.png", 10, 10)

style menu_text is default size 22

# --- Layered images
layeredimage ciri:
    always:
        "ciri_base"
    group outfit:
        attribute casual default
        attribute armor

# --- Translations
translate ukrainian text_parts_1a2b3c4d:
    e "З ідентифікатором перекладу."

translate ukrainian strings:
    old "Sure?"
    new "Певно?"

translate ukrainian python:
    gui.text_font = "DejaVuSans.ttf"

# --- Other statements
init offset = -1
init -1 style default_text size 20
python early:
    renpy.register_statement("achieve", parse=lambda l: l.rest())
label achievements:
    achieve first_kiss
    return
