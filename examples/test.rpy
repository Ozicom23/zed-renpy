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
