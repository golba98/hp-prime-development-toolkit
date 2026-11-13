"""OS-level window, pixel and input drivers. Used only by explicit GUI tests."""
import ctypes as C
import ctypes.util
import os
import struct
import subprocess
import sys
import time
from ctypes import wintypes as W

def bind(lib, name, result, *args):
    f = getattr(lib, name)
    f.restype, f.argtypes = result, list(args)
    return f

class Image:
    def __init__(self, width, height, pixels):
        self.width, self.height, self.pixels = width, height, pixels
    def at(self, x, y):
        return self.pixels[y*self.width+x]
    def ppm(self, path):
        path.write_bytes(f"P6\n{self.width} {self.height}\n255\n".encode()
                         + bytes(c for p in self.pixels for c in ((p>>16)&255,(p>>8)&255,p&255)))

class Windows:
    def __init__(self):
        self.user = C.WinDLL("user32", use_last_error=True)
        self.gdi = C.WinDLL("gdi32", use_last_error=True)
        u, g = self.user, self.gdi
        class StationFlags(C.Structure):
            _fields_=[("inherit",W.BOOL),("reserved",W.BOOL),("flags",W.DWORD)]
        station=bind(u,"GetProcessWindowStation",W.HANDLE)()
        flags=StationFlags();needed=W.DWORD()
        query=bind(u,"GetUserObjectInformationW",W.BOOL,W.HANDLE,C.c_int,C.c_void_p,W.DWORD,C.POINTER(W.DWORD))
        assert query(station,1,C.byref(flags),C.sizeof(flags),C.byref(needed)) and flags.flags&1,"No visible interactive Windows window station"
        desktop=bind(u,"OpenInputDesktop",W.HANDLE,W.DWORD,W.BOOL,W.DWORD)(0,False,1)
        assert desktop,"No accessible Windows input desktop (service/session-0 runners cannot inject input)"
        bind(u,"CloseDesktop",W.BOOL,W.HANDLE)(desktop)
        self.dpi_context = bind(u,"SetProcessDpiAwarenessContext",W.BOOL,C.c_void_p)
        self.dpi_context(C.c_void_p(-4))  # Per-monitor V2; test coordinates are physical.
        self.enum = bind(u,"EnumWindows",W.BOOL,C.c_void_p,W.LPARAM)
        self.pid = bind(u,"GetWindowThreadProcessId",W.DWORD,W.HWND,C.POINTER(W.DWORD))
        self.visible = bind(u,"IsWindowVisible",W.BOOL,W.HWND)
        self.get_title = bind(u,"GetWindowTextW",C.c_int,W.HWND,W.LPWSTR,C.c_int)
        self.rect = bind(u,"GetClientRect",W.BOOL,W.HWND,C.POINTER(W.RECT))
        self.origin = bind(u,"ClientToScreen",W.BOOL,W.HWND,C.POINTER(W.POINT))
        self.foreground = bind(u,"SetForegroundWindow",W.BOOL,W.HWND)
        self.get_foreground = bind(u,"GetForegroundWindow",W.HWND)
        self.show = bind(u,"ShowWindow",W.BOOL,W.HWND,C.c_int)
        self.dpi_for = bind(u,"GetDpiForWindow",C.c_uint,W.HWND)
        self.post = bind(u,"PostMessageW",W.BOOL,W.HWND,W.UINT,W.WPARAM,W.LPARAM)
        self.move = bind(u,"SetWindowPos",W.BOOL,W.HWND,W.HWND,C.c_int,C.c_int,C.c_int,C.c_int,W.UINT)
        self.adjust = bind(u,"AdjustWindowRectExForDpi",W.BOOL,C.POINTER(W.RECT),W.DWORD,W.BOOL,W.DWORD,W.UINT)
        self.style = bind(u,"GetWindowLongW",C.c_long,W.HWND,C.c_int)
        self.cursor = bind(u,"SetCursorPos",W.BOOL,C.c_int,C.c_int)
        self.vk = bind(u,"VkKeyScanW",C.c_short,W.WCHAR)
        self.create = bind(u,"CreateWindowExW",W.HWND,W.DWORD,W.LPCWSTR,W.LPCWSTR,W.DWORD,C.c_int,C.c_int,C.c_int,C.c_int,W.HWND,W.HMENU,W.HINSTANCE,C.c_void_p)
        self.destroy = bind(u,"DestroyWindow",W.BOOL,W.HWND)
        self.getdc = bind(u,"GetDC",W.HDC,W.HWND)
        self.releasedc = bind(u,"ReleaseDC",C.c_int,W.HWND,W.HDC)
        self.createdc = bind(g,"CreateCompatibleDC",W.HDC,W.HDC)
        self.deletedc = bind(g,"DeleteDC",W.BOOL,W.HDC)
        self.bitmap = bind(g,"CreateDIBSection",W.HBITMAP,W.HDC,C.c_void_p,W.UINT,C.POINTER(C.c_void_p),W.HANDLE,W.DWORD)
        self.select = bind(g,"SelectObject",W.HANDLE,W.HDC,W.HANDLE)
        self.delete = bind(g,"DeleteObject",W.BOOL,W.HANDLE)
        self.blit = bind(g,"BitBlt",W.BOOL,W.HDC,C.c_int,C.c_int,C.c_int,C.c_int,W.HDC,C.c_int,C.c_int,W.DWORD)
        class Mouse(C.Structure):
            _fields_=[("dx",W.LONG),("dy",W.LONG),("data",W.DWORD),("flags",W.DWORD),("time",W.DWORD),("extra",C.c_size_t)]
        class Keyboard(C.Structure):
            _fields_=[("vk",W.WORD),("scan",W.WORD),("flags",W.DWORD),("time",W.DWORD),("extra",C.c_size_t)]
        class Union(C.Union):
            _fields_=[("mouse",Mouse),("key",Keyboard)]
        class Input(C.Structure):
            _anonymous_=("value",)
            _fields_=[("type",W.DWORD),("value",Union)]
        self.Input = Input
        self.send = bind(u,"SendInput",W.UINT,W.UINT,C.POINTER(Input),C.c_int)
        self.sink = None
    def find(self, pid):
        found=[]
        callback=C.WINFUNCTYPE(W.BOOL,W.HWND,W.LPARAM)
        @callback
        def visit(window,_):
            owner=W.DWORD()
            self.pid(window,C.byref(owner))
            if owner.value==pid and self.visible(window) and self.title(window).startswith("hpdev"):
                found.append(window)
            return True
        self.enum(C.cast(visit,C.c_void_p),0)
        return found[0] if found else None
    def title(self,w):
        out=C.create_unicode_buffer(4096);self.get_title(w,out,len(out));return out.value
    def dpi(self,w): return self.dpi_for(w)/96
    def size(self,w):
        r=W.RECT();assert self.rect(w,C.byref(r));return r.right,r.bottom
    def focus(self,w):
        self.foreground(w)
        for _ in range(20):
            if self.get_foreground()==w:return
            time.sleep(.025)
        raise AssertionError("Test window could not acquire the interactive foreground desktop")
    def blur(self,w):
        self.sink=self.create(0,"STATIC","hpdev native focus test",0x10CF0000,10,10,120,80,None,None,None,None)
        assert self.sink
        self.foreground(self.sink)
    def cleanup(self):
        if self.sink:self.destroy(self.sink);self.sink=None
    def _key(self,vk,down):
        event=self.Input(type=1);event.key.vk=vk;event.key.flags=(0 if down else 2)|(1 if 0x21<=vk<=0x28 else 0)
        assert self.send(1,C.byref(event),C.sizeof(event))==1,"SendInput unavailable (desktop/UIPI)"
    def key(self,name,down):
        vk={"up":0x26,"down":0x28,"left":0x25,"right":0x27,"enter":0x0d,"escape":0x1b,"shift":0x10,"control":0x11,"alt":0x12,"command":0x5b,"f12":0x7b}.get(name)
        if vk is None:vk=self.vk(name)&255
        self._key(vk,down)
    def text(self,text):
        for ch in text:
            vk=self.vk(ch);assert vk!=-1,f"no keyboard mapping for {ch!r}"
            modifiers=[key for mask,key in [(1,0x10),(2,0x11),(4,0x12)] if (vk>>8)&mask]
            for key in modifiers:self._key(key,True)
            self._key(vk&255,True);self._key(vk&255,False)
            for key in reversed(modifiers):self._key(key,False)
            time.sleep(.015)
    def click(self,w,x,y):
        point=W.POINT(round(x),round(y));assert self.origin(w,C.byref(point));self.cursor(point.x,point.y)
        # Let the native motion event reach the app before injecting a button.
        time.sleep(.04)
        for flags in [2,4]:
            event=self.Input(type=0);event.mouse.flags=flags
            assert self.send(1,C.byref(event),C.sizeof(event))==1
            time.sleep(.02)
    def close(self,w):self.post(w,0x10,0,0)
    def minimize(self,w):self.show(w,6)
    def maximize(self,w):self.show(w,3)
    def restore(self,w):self.show(w,9);self.focus(w)
    def resize(self,w,width,height):
        r=W.RECT(0,0,width,height)
        assert self.adjust(C.byref(r),self.style(w,-16)&0xffffffff,False,self.style(w,-20)&0xffffffff,self.dpi_for(w))
        assert self.move(w,None,0,0,r.right-r.left,r.bottom-r.top,0x6)
    def image(self,w):
        width,height=self.size(w)
        dc=self.getdc(w);memory=self.createdc(dc);data=C.c_void_p()
        header=struct.pack("<IiiHHIIiiII",40,width,-height,1,32,0,width*height*4,0,0,0,0)
        info=C.create_string_buffer(header+b"\0"*4)
        bitmap=self.bitmap(dc,info,0,C.byref(data),None,0)
        assert bitmap and data.value
        previous=self.select(memory,bitmap)
        try:
            assert self.blit(memory,0,0,width,height,dc,0,0,0x00CC0020)
            pixels=[p[0]&0xffffff for p in struct.iter_unpack("<I",C.string_at(data,width*height*4))]
            return Image(width,height,pixels)
        finally:
            self.select(memory,previous);self.delete(bitmap);self.deletedc(memory);self.releasedc(w,dc)

class Mac:
    def __init__(self):
        self.cg=C.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
        self.cf=C.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        self.ax=C.CDLL("/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices")
        assert bind(self.cg,"CGPreflightScreenCaptureAccess",C.c_bool)(),"Screen Recording permission is not available to this test process"
        assert bind(self.ax,"AXIsProcessTrusted",C.c_bool)(),"Accessibility permission is not available to this test process"
        self.release=bind(self.cf,"CFRelease",None,C.c_void_p)
        self.arraylen=bind(self.cf,"CFArrayGetCount",C.c_long,C.c_void_p)
        self.arrayget=bind(self.cf,"CFArrayGetValueAtIndex",C.c_void_p,C.c_void_p,C.c_long)
        self.dictget=bind(self.cf,"CFDictionaryGetValue",C.c_void_p,C.c_void_p,C.c_void_p)
        self.stringnew=bind(self.cf,"CFStringCreateWithCString",C.c_void_p,C.c_void_p,C.c_char_p,C.c_uint)
        self.stringget=bind(self.cf,"CFStringGetCString",C.c_bool,C.c_void_p,C.c_char_p,C.c_long,C.c_uint)
        self.numberget=bind(self.cf,"CFNumberGetValue",C.c_bool,C.c_void_p,C.c_int,C.c_void_p)
        self.windowlist=bind(self.cg,"CGWindowListCopyWindowInfo",C.c_void_p,C.c_uint,C.c_uint)
        class Point(C.Structure):_fields_=[("x",C.c_double),("y",C.c_double)]
        class Size(C.Structure):_fields_=[("width",C.c_double),("height",C.c_double)]
        class Rect(C.Structure):_fields_=[("origin",Point),("size",Size)]
        self.Point,self.Size,self.Rect=Point,Size,Rect
        self.makeimage=bind(self.cg,"CGWindowListCreateImage",C.c_void_p,Rect,C.c_uint,C.c_uint,C.c_uint)
        self.imgw=bind(self.cg,"CGImageGetWidth",C.c_size_t,C.c_void_p)
        self.imgh=bind(self.cg,"CGImageGetHeight",C.c_size_t,C.c_void_p)
        self.color=bind(self.cg,"CGColorSpaceCreateDeviceRGB",C.c_void_p)
        self.context=bind(self.cg,"CGBitmapContextCreate",C.c_void_p,C.c_void_p,C.c_size_t,C.c_size_t,C.c_size_t,C.c_size_t,C.c_void_p,C.c_uint)
        self.draw=bind(self.cg,"CGContextDrawImage",None,C.c_void_p,Rect,C.c_void_p)
        self.keyevent=bind(self.cg,"CGEventCreateKeyboardEvent",C.c_void_p,C.c_void_p,C.c_ushort,C.c_bool)
        self.mouseevent=bind(self.cg,"CGEventCreateMouseEvent",C.c_void_p,C.c_void_p,C.c_uint,Point,C.c_uint)
        self.post=bind(self.cg,"CGEventPost",None,C.c_uint,C.c_void_p)
        self.unicode=bind(self.cg,"CGEventKeyboardSetUnicodeString",None,C.c_void_p,C.c_ulong,C.c_void_p)
        self.windows={};self._dpi={};self.offset={};self.flags=0;self.ax_windows={};self.positioned=set()
        self.eventflags=bind(self.cg,"CGEventSetFlags",None,C.c_void_p,C.c_ulonglong)
    def value(self,dictionary,name,number=False):
        key=self.stringnew(None,name.encode(),0x08000100)
        try:v=self.dictget(dictionary,key)
        finally:self.release(key)
        if not v:return None
        if number:
            out=C.c_longlong();assert self.numberget(v,4,C.byref(out));return out.value
        text=C.create_string_buffer(4096);self.stringget(v,text,len(text),0x08000100);return text.value.decode("utf-8")
    def refresh(self):
        array=self.windowlist(1,0)
        assert array,"no WindowServer session"
        try:
            rows=[]
            for i in range(self.arraylen(array)):
                d=self.arrayget(array,i)
                key=self.stringnew(None,b"kCGWindowBounds",0x08000100)
                bounds=self.dictget(d,key);self.release(key)
                def number(name):
                    key=self.stringnew(None,name.encode(),0x08000100)
                    v=self.dictget(bounds,key);self.release(key)
                    out=C.c_double();assert self.numberget(v,13,C.byref(out));return out.value
                row={"id":self.value(d,"kCGWindowNumber",True),"pid":self.value(d,"kCGWindowOwnerPID",True),"title":self.value(d,"kCGWindowName") or "",
                     "x":number("X"),"y":number("Y"),"width":number("Width"),"height":number("Height")}
                rows.append(row);self.windows[row["id"]]=row
            return rows
        finally:self.release(array)
    def find(self,pid):
        return next((r["id"] for r in self.refresh() if r["pid"]==pid and r["title"].startswith("hpdev")),None)
    def title(self,w):self.refresh();return self.windows[w]["title"]
    def _frontmost(self,pid,activate=False):
        app=bind(self.ax,"AXUIElementCreateApplication",C.c_void_p,C.c_int)(pid)
        key=self.stringnew(None,b"AXFrontmost",0x08000100);out=C.c_void_p()
        try:
            if activate:
                boolean=C.c_void_p.in_dll(self.cf,"kCFBooleanTrue")
                assert bind(self.ax,"AXUIElementSetAttributeValue",C.c_int,C.c_void_p,C.c_void_p,C.c_void_p)(app,key,boolean)==0
            assert bind(self.ax,"AXUIElementCopyAttributeValue",C.c_int,C.c_void_p,C.c_void_p,C.POINTER(C.c_void_p))(app,key,C.byref(out))==0
            return bind(self.cf,"CFBooleanGetValue",C.c_bool,C.c_void_p)(out)
        finally:
            if out.value:self.release(out)
            self.release(key);self.release(app)
    def focus(self,w):
        pid=self.windows[w]["pid"]
        self._frontmost(pid,activate=True)
        deadline=time.monotonic()+4
        while not self._frontmost(pid):
            assert time.monotonic()<deadline,"Calculator did not acquire application focus"
            time.sleep(.04)
        # Hosted Mac screens can be only 1024px wide. Keep debugger buttons
        # reachable by real mouse events even when its window grows to 1080px.
        if w not in self.positioned:
            window=self._ax_window(w);key=self.stringnew(None,b"AXPosition",0x08000100)
            point=self.Point(0,25)
            value=bind(self.ax,"AXValueCreate",C.c_void_p,C.c_int,C.c_void_p)(1,C.byref(point))
            try:assert bind(self.ax,"AXUIElementSetAttributeValue",C.c_int,C.c_void_p,C.c_void_p,C.c_void_p)(window,key,value)==0
            finally:self.release(value);self.release(key);self.release(window)
            self.positioned.add(w)
    def blur(self,w):
        # An unbundled Python process cannot always become a regular Cocoa
        # application. Activate Finder and verify the calculator loses focus.
        subprocess.run(["/usr/bin/open","-a","Finder"],check=True,timeout=5)
        deadline=time.monotonic()+4
        while self._frontmost(self.windows[w]["pid"]):
            assert time.monotonic()<deadline,"Finder did not acquire application focus"
            time.sleep(.04)
    def cleanup(self):
        for window in self.ax_windows.values():self.release(window)
        self.ax_windows.clear()
    def key(self,name,down):
        modifier={"shift":1<<17,"control":1<<18,"alt":1<<19,"command":1<<20}.get(name,0)
        if down:self.flags|=modifier
        else:self.flags&=~modifier
        codes={"up":126,"down":125,"left":123,"right":124,"enter":36,"escape":53,"shift":56,"control":59,"alt":58,"command":55,"f12":111}
        if name in codes:
            event=self.keyevent(None,codes[name],down)
        else:
            layout="asdfhgzxcv?bqwerty123465=97-80]ou[ip\nlj'k;\\,/nm."
            # ANSI virtual keycodes 0..47, excluding return marker at code 36.
            code=layout.find(name)
            assert code>=0,f"unsupported native key {name!r}"
            event=self.keyevent(None,code,down)
            typed="_" if name=="-" and self.flags&(1<<17) else name
            units=(C.c_ushort*(len(typed.encode("utf-16-le"))//2)).from_buffer_copy(typed.encode("utf-16-le"))
            self.unicode(event,len(units),units)
        assert event;self.eventflags(event,self.flags);self.post(0,event);self.release(event)
    def text(self,text):
        # Unicode text is posted as genuine Quartz keyboard events, with key
        # identity supplied for the ASCII keys exercised by the calculator.
        for ch in text:
            if ch=="_":
                self.key("shift",True);self.key("-",True);self.key("-",False);self.key("shift",False)
            elif ch==" ":
                event=self.keyevent(None,49,True);units=(C.c_ushort*1)(32);self.unicode(event,1,units);self.post(0,event);self.release(event)
                event=self.keyevent(None,49,False);self.post(0,event);self.release(event)
            else:self.key(ch,True);self.key(ch,False)
            time.sleep(.02)
    def click(self,w,x,y):
        self.image(w)
        row=self.windows[w];dpi=self._dpi[w]
        point=self.Point(row["x"]+x/dpi,row["y"]+(y+self.offset[w])/dpi)
        for kind in [5,1,2]:
            event=self.mouseevent(None,kind,point,0);self.post(0,event);self.release(event);time.sleep(.025)
    def _ax_window(self,w):
        # Accessibility is used only for lifecycle operations unavailable through
        # Quartz's read-only window list; never change global TCC settings.
        retain=bind(self.cf,"CFRetain",C.c_void_p,C.c_void_p)
        if w in self.ax_windows:return retain(self.ax_windows[w])
        create=bind(self.ax,"AXUIElementCreateApplication",C.c_void_p,C.c_int)
        copy=bind(self.ax,"AXUIElementCopyAttributeValue",C.c_int,C.c_void_p,C.c_void_p,C.POINTER(C.c_void_p))
        app=create(self.windows[w]["pid"]);key=self.stringnew(None,b"AXWindows",0x08000100);out=C.c_void_p()
        try:
            deadline=time.monotonic()+4
            while True:
                assert copy(app,key,C.byref(out))==0
                if out.value and self.arraylen(out)>0:break
                if out.value:self.release(out);out=C.c_void_p()
                assert time.monotonic()<deadline,"Application has no accessible native window"
                time.sleep(.04)
            window=self.arrayget(out,0)
            self.ax_windows[w]=retain(window)
            return retain(window)
        finally:
            if out.value:self.release(out)
            self.release(key);self.release(app)
    def _press_button(self,w,name):
        window=self._ax_window(w);key=self.stringnew(None,name.encode(),0x08000100);button=C.c_void_p()
        action=self.stringnew(None,b"AXPress",0x08000100)
        try:
            assert bind(self.ax,"AXUIElementCopyAttributeValue",C.c_int,C.c_void_p,C.c_void_p,C.POINTER(C.c_void_p))(window,key,C.byref(button))==0
            assert bind(self.ax,"AXUIElementPerformAction",C.c_int,C.c_void_p,C.c_void_p)(button,action)==0
        finally:
            if button.value:self.release(button)
            self.release(key);self.release(action);self.release(window)
    def close(self,w):self._press_button(w,"AXCloseButton")
    def _minimized(self,w,value):
        window=self._ax_window(w);key=self.stringnew(None,b"AXMinimized",0x08000100)
        boolean=C.c_void_p.in_dll(self.cf,"kCFBooleanTrue" if value else "kCFBooleanFalse")
        try:assert bind(self.ax,"AXUIElementSetAttributeValue",C.c_int,C.c_void_p,C.c_void_p,C.c_void_p)(window,key,boolean)==0
        finally:self.release(key);self.release(window)
    def minimize(self,w):self._minimized(w,True)
    def restore(self,w):self._minimized(w,False);self.focus(w)
    def resize(self,w,width,height):
        window=self._ax_window(w);key=self.stringnew(None,b"AXSize",0x08000100)
        dpi=self.dpi(w);size=self.Size(width/dpi,(height+self.offset[w])/dpi)
        value=bind(self.ax,"AXValueCreate",C.c_void_p,C.c_int,C.c_void_p)(2,C.byref(size))
        try:assert bind(self.ax,"AXUIElementSetAttributeValue",C.c_int,C.c_void_p,C.c_void_p,C.c_void_p)(window,key,value)==0
        finally:self.release(value);self.release(key);self.release(window)
    def maximize(self,w):self._press_button(w,"AXZoomButton")
    def dpi(self,w):
        deadline=time.monotonic()+4
        while w not in self._dpi:
            try:self.image(w)
            except AssertionError:
                if time.monotonic()>=deadline:raise
                time.sleep(.04)
        return self._dpi[w]
    def size(self,w):
        image=self.image(w);return image.width,image.height
    def image(self,w):
        self.refresh()
        rect=self.Rect(self.Point(float("inf"),float("inf")),self.Size(0,0))
        image=self.makeimage(rect,8,w,1)
        assert image,"native window capture unavailable"
        width,height=self.imgw(image),self.imgh(image)
        data=C.create_string_buffer(width*height*4)
        color=self.color();ctx=self.context(data,width,height,8,width*4,color,0x2002)
        try:
            assert ctx
            self.draw(ctx,self.Rect(self.Point(0,0),self.Size(width,height)),image)
            pixels=[p[0]&0xffffff for p in struct.iter_unpack("<I",data.raw)]
        finally:
            if ctx:self.release(ctx)
            self.release(color);self.release(image)
        dpi=width/self.windows[w]["width"]
        # Detect the application's own toolbar, excluding the native title bar.
        def toolbar_at(y):
            def near(pixel):
                return sum(abs(((pixel>>shift)&255)-((0x273344>>shift)&255)) for shift in (0,8,16))<=9
            # These columns lie between Stop and the status label. Button
            # interiors differ from the toolbar; using them crops off 31 rows.
            return all(near(pixels[y*width+min(width-1,round(x*dpi))]) for x in (218,219))
        rows=[y for y in range(height) if toolbar_at(y)]
        assert rows,"captured image does not contain the actual hpdev toolbar"
        if min(rows)>height//2:
            pixels=[p for y in reversed(range(height)) for p in pixels[y*width:(y+1)*width]]
            rows=[height-1-y for y in rows]
        # Retain the one-pixel native content separator above the first plain
        # toolbar row. Dropping it changes a 512px client into 511px, which
        # incorrectly reduces the inferred integer framebuffer scale to 1x.
        offset=max(0,min(rows)-1);self._dpi[w]=dpi;self.offset[w]=offset
        return Image(width,height-offset,pixels[offset*width:])

def driver():
    if sys.platform=="win32":return Windows()
    if sys.platform=="darwin":return Mac()
    from x11 import Linux
    return Linux()
