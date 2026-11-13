"""Xlib/XTest implementation used to run the portable GUI suite under Xvfb."""

import ctypes as C
import ctypes.util
import os
import time

from desktop import Image, bind


class Linux:
    def __init__(self):
        self.x = C.CDLL(ctypes.util.find_library("X11"))
        self.xt = C.CDLL(ctypes.util.find_library("Xtst"))
        p = C.c_void_p
        u = C.c_ulong
        i = C.c_int
        self.display = bind(self.x, "XOpenDisplay", p, C.c_char_p)(None)
        assert self.display, "X11 display is required; run explicitly under Xvfb"
        self.root = bind(self.x, "XDefaultRootWindow", u, p)(self.display)
        self.query = bind(
            self.x,
            "XQueryTree",
            i,
            p,
            u,
            C.POINTER(u),
            C.POINTER(u),
            C.POINTER(C.POINTER(u)),
            C.POINTER(C.c_uint),
        )
        self.atom = bind(self.x, "XInternAtom", u, p, C.c_char_p, i)
        self.prop = bind(
            self.x,
            "XGetWindowProperty",
            i,
            p,
            u,
            u,
            C.c_long,
            C.c_long,
            i,
            u,
            C.POINTER(u),
            C.POINTER(i),
            C.POINTER(u),
            C.POINTER(u),
            C.POINTER(p),
        )
        self.free = bind(self.x, "XFree", i, p)
        self.flush = bind(self.x, "XFlush", i, p)
        self.setfocus = bind(self.x, "XSetInputFocus", i, p, u, i, u)
        self.keycode = bind(self.x, "XKeysymToKeycode", C.c_ubyte, p, u)
        self.fakekey = bind(self.xt, "XTestFakeKeyEvent", i, p, C.c_uint, i, u)
        self.button = bind(self.xt, "XTestFakeButtonEvent", i, p, C.c_uint, i, u)
        self.motion = bind(self.xt, "XTestFakeMotionEvent", i, p, i, i, i, u)
        self.translate = bind(
            self.x,
            "XTranslateCoordinates",
            i,
            p,
            u,
            u,
            i,
            i,
            C.POINTER(i),
            C.POINTER(i),
            C.POINTER(u),
        )
        self.geometry = bind(
            self.x,
            "XGetGeometry",
            i,
            p,
            u,
            C.POINTER(u),
            C.POINTER(i),
            C.POINTER(i),
            C.POINTER(C.c_uint),
            C.POINTER(C.c_uint),
            C.POINTER(C.c_uint),
            C.POINTER(C.c_uint),
        )
        self.getimage = bind(
            self.x, "XGetImage", p, p, u, i, i, C.c_uint, C.c_uint, u, i
        )
        self.getpixel = bind(self.x, "XGetPixel", u, p, i, i)
        self.destroyimage = bind(self.x, "XDestroyImage", i, p)
        self.resizing = bind(self.x, "XResizeWindow", i, p, u, C.c_uint, C.c_uint)
        self.unmap = bind(self.x, "XUnmapWindow", i, p, u)
        self.map = bind(self.x, "XMapRaised", i, p, u)
        self.send = bind(self.x, "XSendEvent", i, p, u, i, C.c_long, p)

    def property(self, w, name):
        typ = C.c_ulong()
        fmt = C.c_int()
        count = C.c_ulong()
        left = C.c_ulong()
        data = C.c_void_p()
        self.prop(
            self.display,
            w,
            self.atom(self.display, name.encode(), 0),
            0,
            2048,
            0,
            0,
            C.byref(typ),
            C.byref(fmt),
            C.byref(count),
            C.byref(left),
            C.byref(data),
        )
        try:
            if not data.value:
                return None
            if fmt.value == 32:
                return C.cast(data, C.POINTER(C.c_ulong))[0]
            return C.string_at(data, count.value).decode("utf-8", errors="replace")
        finally:
            if data.value:
                self.free(data)

    def find(self, pid):
        root = C.c_ulong()
        parent = C.c_ulong()
        children = C.POINTER(C.c_ulong)()
        count = C.c_uint()
        self.query(
            self.display,
            self.root,
            C.byref(root),
            C.byref(parent),
            C.byref(children),
            C.byref(count),
        )
        try:
            return next(
                (
                    w
                    for w in children[: count.value]
                    if self.property(w, "_NET_WM_PID") == pid
                    and self.title(w).startswith("hpdev")
                ),
                None,
            )
        finally:
            if children:
                self.free(children)

    def title(self, w):
        return self.property(w, "_NET_WM_NAME") or ""

    def dpi(self, w):
        return float(os.environ.get("WINIT_X11_SCALE_FACTOR", "1"))

    def size(self, w):
        root = C.c_ulong()
        x = C.c_int()
        y = C.c_int()
        width = C.c_uint()
        height = C.c_uint()
        border = C.c_uint()
        depth = C.c_uint()
        self.geometry(
            self.display,
            w,
            C.byref(root),
            C.byref(x),
            C.byref(y),
            C.byref(width),
            C.byref(height),
            C.byref(border),
            C.byref(depth),
        )
        return width.value, height.value

    def focus(self, w):
        self.setfocus(self.display, w, 2, 0)
        self.flush(self.display)

    def blur(self, w):
        self.focus(self.root)

    def cleanup(self):
        pass

    def key(self, name, down):
        symbol = {
            "up": 0xFF52,
            "down": 0xFF54,
            "left": 0xFF51,
            "right": 0xFF53,
            "enter": 0xFF0D,
            "escape": 0xFF1B,
            "shift": 0xFFE1,
            "control": 0xFFE3,
            "alt": 0xFFE9,
            "command": 0xFFEB,
            "f12": 0xFFC9,
        }.get(name, ord(name) if len(name) == 1 else 0)
        code = self.keycode(self.display, symbol)
        assert code
        self.fakekey(self.display, code, down, 0)
        self.flush(self.display)

    def text(self, text):
        for ch in text:
            if ch == "_":
                self.key("shift", True)
                self.key("-", True)
                self.key("-", False)
                self.key("shift", False)
            else:
                self.key(ch, True)
                self.key(ch, False)
            time.sleep(0.015)

    def click(self, w, x, y):
        rx = C.c_int()
        ry = C.c_int()
        child = C.c_ulong()
        self.translate(
            self.display,
            w,
            self.root,
            round(x),
            round(y),
            C.byref(rx),
            C.byref(ry),
            C.byref(child),
        )
        self.motion(self.display, -1, rx.value, ry.value, 0)
        for down in [True, False]:
            self.button(self.display, 1, down, 0)
            self.flush(self.display)
            time.sleep(0.02)

    def close(self, w):
        class Message(C.Structure):
            _fields_ = [
                ("type", C.c_int),
                ("serial", C.c_ulong),
                ("send_event", C.c_int),
                ("display", C.c_void_p),
                ("window", C.c_ulong),
                ("message_type", C.c_ulong),
                ("format", C.c_int),
                ("data", C.c_long * 5),
            ]

        message = Message(
            type=33,
            display=self.display,
            window=w,
            message_type=self.atom(self.display, b"WM_PROTOCOLS", 0),
            format=32,
        )
        message.data[0] = self.atom(self.display, b"WM_DELETE_WINDOW", 0)
        buffer = C.create_string_buffer(192)
        C.memmove(buffer, C.byref(message), C.sizeof(message))
        self.send(self.display, w, 0, 0, buffer)
        self.flush(self.display)

    def resize(self, w, width, height):
        self.resizing(self.display, w, width, height)
        self.flush(self.display)

    def minimize(self, w):
        self.unmap(self.display, w)
        self.flush(self.display)

    def restore(self, w):
        self.map(self.display, w)
        self.focus(w)

    def maximize(self, w):
        self.resize(w, 900, 650)

    def image(self, w):
        width, height = self.size(w)
        image = self.getimage(self.display, w, 0, 0, width, height, 0xFFFFFFFF, 2)
        assert image
        try:

            class XImage(C.Structure):
                _fields_ = [
                    ("width", C.c_int),
                    ("height", C.c_int),
                    ("offset", C.c_int),
                    ("format", C.c_int),
                    ("data", C.c_void_p),
                    ("byte_order", C.c_int),
                    ("unit", C.c_int),
                    ("bit_order", C.c_int),
                    ("pad", C.c_int),
                    ("depth", C.c_int),
                    ("stride", C.c_int),
                    ("bits", C.c_int),
                ]

            view = C.cast(image, C.POINTER(XImage)).contents
            assert view.bits == 32
            import struct

            data = C.string_at(view.data, view.stride * height)
            pixels = []
            for y in range(height):
                pixels.extend(
                    p[0] & 0xFFFFFF
                    for p in struct.iter_unpack(
                        "<I" if view.byte_order == 0 else ">I",
                        data[y * view.stride : y * view.stride + width * 4],
                    )
                )
            return Image(width, height, pixels)
        finally:
            self.destroyimage(image)
