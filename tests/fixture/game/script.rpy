define e = Character("Eileen")
default points = 0

label start:
    e "hi"
    jump shop
    call screen inventory() with Dissolve(0.3)
