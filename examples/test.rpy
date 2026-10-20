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
