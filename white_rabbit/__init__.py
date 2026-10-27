# White Rabbit: the led_mon system monitor and the LED Matrix controls in one app.

# inputmodule.inputmodule and inputmodule.gui import each other; importing the gui package first
# (as Framework's ledmatrixgui does) is the order that works
import inputmodule.gui  # noqa: F401
