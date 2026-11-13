"""Shared native OS acceptance tests. Requires an explicitly enabled GUI session."""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time
import unittest
from desktop import driver

ROOT=Path(__file__).resolve().parents[2]
BINARY=None
ARTIFACTS=None
OBSERVED_DPI=[]

def wait(predicate, message="native condition"):
    deadline=time.monotonic()+12
    while True:
        result=predicate()
        if result:return result
        if time.monotonic()>deadline:raise AssertionError("Timed out: "+message)
        time.sleep(.04)

class App:
    def __init__(self,desktop,file,args=()):
        self.desktop=desktop
        self.directory=tempfile.TemporaryDirectory(prefix="hpdev-native-")
        self.report=Path(self.directory.name)/"report.json"
        self.logs=[open(Path(self.directory.name)/name,"wb") for name in ("stdout.txt","stderr.txt")]
        self.process=subprocess.Popen([str(BINARY),"run",str(file),"--report",str(self.report),*args],stdin=subprocess.DEVNULL,stdout=self.logs[0],stderr=self.logs[1],env={**os.environ,"WINIT_X11_SCALE_FACTOR":os.environ.get("WINIT_X11_SCALE_FACTOR","1")},cwd=ROOT)
        self.window=None
        try:
            def discover():
                if self.process.poll() is not None:
                    raise AssertionError("App exited before window creation: "+self.diagnostics())
                return desktop.find(self.process.pid)
            self.window=wait(discover,"real OS window creation")
            desktop.focus(self.window)
        except BaseException:
            self.evidence("launch-failure-"+str(self.process.pid))
            self.cleanup();raise
        OBSERVED_DPI.append(desktop.dpi(self.window))
        self.keys="--show-keys" in args
        self.debug=False
    def diagnostics(self):
        return "\n".join((Path(self.directory.name)/n).read_text(errors="replace") for n in ("stdout.txt","stderr.txt"))
    def title(self):return self.desktop.title(self.window)
    def state(self,text):
        wait(lambda:text in self.title(),f"{text!r}, last title={self.title()!r}")
        if text=="Running":time.sleep(.16) # BST's own MOUSE debounce after modal writeback.
    def press(self,key):
        self.desktop.key(key,True);time.sleep(.025);self.desktop.key(key,False)
    def text(self,text):self.desktop.text(text)
    def click(self,x,y):
        dpi=self.desktop.dpi(self.window)
        self.desktop.click(self.window,x*dpi,y*dpi)
    def viewport(self,image):
        dpi=self.desktop.dpi(self.window)
        sidebar=(260 if self.keys else 0)+(440 if self.debug else 0)
        width=image.width-round(sidebar*dpi);bar=round(32*dpi);height=image.height-bar
        scale=min(width//320,height//240)
        assert scale>=1
        return (width-320*scale)//2,bar+(height-240*scale)//2,scale
    def framebuffer(self):
        image=self.desktop.image(self.window)
        x,y,scale=self.viewport(image)
        return [image.at(x+cx*scale,y+cy*scale) for cy in range(240) for cx in range(320)]
    def touch(self,x,y):
        image=self.desktop.image(self.window);left,top,scale=self.viewport(image)
        self.desktop.click(self.window,left+x*scale,top+y*scale)
    def debugger(self):
        self.click(100,15);self.debug=True
        wait(lambda:self.desktop.size(self.window)[0]>=round(1080*self.desktop.dpi(self.window)),"inspector resize")
    def pause(self):
        self.click(670,47);self.state("Paused:")
    def command(self,text):
        self.click(670,80);self.text(text);self.press("enter");time.sleep(.15)
    def finish(self):
        wait(lambda:self.process.poll() is not None,"worker cleanup and CLI exit")
        report=json.loads(self.report.read_text())
        assert self.process.returncode==report["exit_code"],self.diagnostics()
        return report
    def evidence(self,name):
        destination=ARTIFACTS/name;destination.mkdir(parents=True,exist_ok=True)
        for file in Path(self.directory.name).iterdir():
            (destination/file.name).write_bytes(file.read_bytes())
        if self.window and self.process.poll() is None:
            try:self.desktop.image(self.window).ppm(destination/"window.ppm")
            except Exception as error:(destination/"capture-error.txt").write_text(str(error))
    def cleanup(self):
        if self.process.poll() is None:
            if self.window:
                try:self.desktop.close(self.window);self.process.wait(timeout=3)
                except Exception:self.process.kill();self.process.wait()
            else:self.process.kill();self.process.wait()
        for file in self.logs:file.close()
        self.directory.cleanup()

class NativeTests(unittest.TestCase):
    def setUp(self):self.desktop=driver();self.apps=[]
    def tearDown(self):
        for index,app in enumerate(self.apps):
            app.evidence(self.id().split(".")[-1]+f"-{index}")
            app.cleanup()
        self.desktop.cleanup()
    def launch(self,file,args=()):
        app=App(self.desktop,file,args);self.apps.append(app);return app
    def test_bst_real_input_menu_and_debugger(self):
        app=self.launch(ROOT/"tests/fixtures/BSTVisualizer.hpprgm",["--exit-on-finish"])
        app.state("Running");empty=app.framebuffer()
        for value in ("42","21","84"):
            app.touch(25,230);app.state("input: Insert node")
            app.text(value);app.press("enter");app.state("Running")
        tree=app.framebuffer();self.assertNotEqual(tree,empty)
        app.press("down");wait(lambda:app.framebuffer()!=tree,"BST scroll down")
        app.press("up");wait(lambda:app.framebuffer()==tree,"BST scroll up")
        app.debugger();app.pause();app.command("print bst_size");app.command("stack")
        previous=app.title();app.click(840,47)
        wait(lambda:"Paused:" in app.title() and app.title()!=previous,"real statement step")
        app.click(755,47);app.state("Running")
        app.touch(80,230);app.state("input: Delete node")
        app.text("42");app.press("enter");app.state("Running")
        self.assertNotEqual(app.framebuffer(),tree)
        app.touch(132,230);app.state("choose: Clear the tree?")
        app.press("1");app.state("Running");wait(lambda:app.framebuffer()==empty,"BST clear")
        app.press("escape");app.state("choose: Exit BST Visualizer?");app.press("1")
        report=app.finish()
        self.assertEqual(report["status"],"success")
        replies=[e.get("text","") for e in report["data"]["debug_history"]]
        self.assertTrue(any("bst_size = 3" in s for s in replies),replies)
        self.assertGreaterEqual(sum(e["kind"]=="stop" for e in report["data"]["debug_history"]),2)
    def test_live_animation_integer_pixels_resize_and_repeated_launch(self):
        for _ in range(2):
            app=self.launch(ROOT/"examples/LIVE.hpprgm")
            colors=set()
            wait(lambda:colors.add(app.framebuffer()[0]) or "Done:" in app.title(),"live rendering")
            self.assertGreaterEqual(len(colors),3,colors)
            image=self.desktop.image(app.window);x,y,scale=app.viewport(image)
            self.assertEqual((320*scale)%320,0)
            self.assertTrue(all(image.at(x+dx,y+dy)==image.at(x,y) for dx in range(scale) for dy in range(scale)))
            self.desktop.resize(app.window,round(820*self.desktop.dpi(app.window)),round(620*self.desktop.dpi(app.window)))
            time.sleep(.2);self.assertEqual(len(app.framebuffer()),320*240)
            self.desktop.maximize(app.window);time.sleep(.15)
            self.desktop.minimize(app.window);time.sleep(.15)
            self.desktop.restore(app.window);time.sleep(.2)
            self.desktop.close(app.window);self.assertEqual(app.finish()["status"],"success")
    def test_unicode_path_getkey_modifiers_and_keypad(self):
        with tempfile.TemporaryDirectory(prefix="hpdev-計算-é-") as folder:
            file=Path(folder)/"計算 é.ppl"
            file.write_text("EXPORT MAIN()\nBEGIN\n LOCAL K; REPEAT K:=GETKEY; WAIT(0.02); UNTIL K==4; RETURN K;\nEND;",encoding="utf-8")
            app=self.launch(file,["--entry","MAIN","--exit-on-finish"])
            app.state("Running");time.sleep(.3)
            self.assertIn("計算 é.ppl",app.title())
            self.desktop.key("shift",True);app.press("up");self.desktop.key("shift",False)
            app.press("escape");self.assertEqual(app.finish()["return_value"]["value"],"4")
        app=self.launch(ROOT/"tests/fixtures/WAIT.ppl",["--show-keys","--exit-on-finish"])
        app.state("Running");app.click(660,287)
        self.assertEqual(app.finish()["return_value"]["value"],"30")
    def test_dialog_validation_and_choice_writeback(self):
        app=self.launch(ROOT/"examples/INPUT.ppl",["--gui","--json","--exit-on-finish"])
        app.state("input: Number");app.text("bad");before=app.framebuffer();app.press("enter")
        wait(lambda:app.framebuffer()!=before,"real dialog validation")
        app.state("input: Number");app.text("7");app.press("enter")
        app.state("choose: Add to number");app.press("down");app.press("enter")
        app.state("message:");app.press("enter")
        self.assertEqual(app.finish()["return_value"]["value"],"9")
    def test_minimized_close_cancels_blocked_vm(self):
        app=self.launch(ROOT/"tests/fixtures/WAIT.ppl")
        app.state("Running");self.desktop.minimize(app.window);time.sleep(.15)
        self.desktop.close(app.window);self.assertEqual(app.finish()["status"],"cancelled")
    def test_runtime_crash_keeps_diagnostics_and_reaps_worker(self):
        with tempfile.TemporaryDirectory() as folder:
            file=Path(folder)/"FAIL.ppl";file.write_text("EXPORT FAIL() BEGIN RETURN 1/0; END;")
            app=self.launch(file);app.state("Done: runtime_error")
            self.desktop.close(app.window);report=app.finish()
            self.assertEqual(report["exit_code"],3)
            self.assertTrue(report["stack"])
            self.assertIn("Division",app.diagnostics())
    def test_held_keys_release_on_focus_loss(self):
        with tempfile.TemporaryDirectory() as folder:
            file=Path(folder)/"HELD.ppl"
            file.write_text("EXPORT HELD() BEGIN LOCAL K; REPEAT K:=GETKEY; IF ISKEYDOWN(2) THEN RECT_P(0,0,319,239,RGB(1,2,200),RGB(1,2,200)); ELSE RECT_P(0,0,319,239,0,0); END; WAIT(0.02); UNTIL K==4; RETURN 0; END;")
            app=self.launch(file,["--exit-on-finish"]);app.state("Running")
            self.desktop.key("up",True)
            wait(lambda:app.framebuffer()[0]!=0,"held key state")
            self.desktop.blur(app.window)
            wait(lambda:app.framebuffer()[0]==0,"focus loss releases calculator keys")
            self.desktop.key("up",False);self.desktop.focus(app.window)
            app.press("escape");self.assertEqual(app.finish()["status"],"success")
    @unittest.skipUnless(sys.platform=="darwin","macOS accelerators")
    def test_command_w_and_command_q_close_cleanly(self):
        for key in ("w","q"):
            app=self.launch(ROOT/"tests/fixtures/WAIT.ppl");app.state("Running")
            self.desktop.key("command",True);app.press(key);self.desktop.key("command",False)
            self.assertEqual(app.finish()["status"],"cancelled")

def main():
    global BINARY,ARTIFACTS
    parser=argparse.ArgumentParser()
    parser.add_argument("--binary",type=Path,required=True)
    parser.add_argument("--artifacts",type=Path,default=ROOT/"test-results/native")
    parser.add_argument("--probe",action="store_true")
    args=parser.parse_args()
    BINARY=args.binary.resolve();ARTIFACTS=args.artifacts.resolve()
    ARTIFACTS.mkdir(parents=True,exist_ok=True)
    status={"os":platform.platform(),"architecture":platform.machine(),"binary":str(BINARY),"interactive":"unavailable"}
    try:desktop=driver();desktop.cleanup()
    except Exception as error:
        status["reason"]=str(error);(ARTIFACTS/"status.json").write_text(json.dumps(status,indent=2))
        print(json.dumps(status));return 78
    status["interactive"]="available"
    if args.probe:
        (ARTIFACTS/"status.json").write_text(json.dumps(status,indent=2));print(json.dumps(status));return 0
    suite=unittest.defaultTestLoader.loadTestsFromTestCase(NativeTests)
    result=unittest.TextTestRunner(verbosity=2).run(suite)
    status.update(interactive="passed" if result.wasSuccessful() else "failed",tests=result.testsRun,skipped=len(result.skipped),observed_dpi=sorted(set(OBSERVED_DPI)),high_dpi_runtime=any(dpi>1 for dpi in OBSERVED_DPI))
    (ARTIFACTS/"status.json").write_text(json.dumps(status,indent=2));print(json.dumps(status))
    return 0 if result.wasSuccessful() else 1

if __name__=="__main__":sys.exit(main())
