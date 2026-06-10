// hello.wren — smallest script: print + show text on the OLED.
//
// Try it live over USB-CDC:  paste the lines, or upload with ^^s … ^^w.

System.print("hello from wren on the deluge")

Oled.clear()
Oled.text(0, 0, "wren: deluge")
Oled.text(0, 12, "hello world")
Oled.show()
