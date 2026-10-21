image eileen happy = "eileen_happy.png"

label gallery:
    scene bg room
    show eileen happy at left
    show eilen happy
    show cg ending
    $ renpy.pause(1.0, hard=True)
    e "Look at all [points] {b}points{/b}."
    return

style gallery_text:
    color "#c8c8ff"
