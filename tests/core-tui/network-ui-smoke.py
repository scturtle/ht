#!/usr/bin/env python3
"""Private userspace PTY transport impairment, not an SSH/TCP-stack or live Pi test."""
from collections import deque
import importlib.util
import itertools
import json
import os
from pathlib import Path
import select
import signal
import statistics
import subprocess
import sys
import tempfile
import time
import harness as H

SPEC=importlib.util.spec_from_file_location("scheduling",Path(__file__).with_name("scheduling-ui-smoke.py"))
SCHED=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCHED)


class ImpairedViewer(H.Viewer):
    """Ordered, bounded, two-way delivery. No system network settings are touched.

    Each read advances the simulated link. Pausing reads really stops draining
    the child's PTY, allowing the actual TUI output path to hit backpressure.
    Jitter never reorders TCP-like stream bytes; partial writes are retained.
    """
    def __init__(self,env):
        super().__init__(env)
        self.up,self.down=deque(),deque()
        self.fragments=itertools.cycle((1,2,7,31,127,1024))
        self.jitter=itertools.cycle((-.004,.003,0,.005,-.002))
        self.paused_until=0
        self.peak=0
        self.fragment_count=0
        self.raw_cleanup=False

    def enqueue(self,queue,data):
        due=max(time.monotonic()+.020+next(self.jitter),queue[-1][0] if queue else 0)
        while data:
            n=next(self.fragments)
            queue.append((due,data[:n]))
            data=data[n:]
            due+=.0003
            self.fragment_count+=1

    def send(self,data):
        if sum(len(part) for _,part in self.up)+len(data)>32768:
            raise BufferError("test upstream queue exceeded its bound")
        self.enqueue(self.up,data)

    def read(self):
        if self.raw_cleanup:
            return super().read()
        deadline=time.monotonic()+.03
        while time.monotonic()<deadline:
            now=time.monotonic()
            if now>=self.paused_until:
                while self.up and self.up[0][0]<=now:
                    due,data=self.up.popleft()
                    written=os.write(self.fd,data)
                    if written<len(data):
                        self.up.appendleft((due,data[written:]))
                        break
                while self.down and self.down[0][0]<=now:
                    _,data=self.down.popleft()
                    self.output.extend(data)
                queued=sum(len(data) for _,data in self.down)
                if queued<32768 and select.select([self.fd],[],[],0)[0]:
                    incoming=b""
                    try:
                        incoming=os.read(self.fd,min(4096,32768-queued))
                        if incoming:self.enqueue(self.down,incoming)
                    except OSError:
                        pass  # A clean PTY child exit is reported by wait_exit.
                    self.peak=max(self.peak,queued+len(incoming))
            time.sleep(.001)
        return bytes(self.output)

    def cleanup(self):
        self.raw_cleanup=True
        super().cleanup()


def pump(view,seconds):
    end=time.monotonic()+seconds
    while time.monotonic()<end:view.read()


def main():
    with tempfile.TemporaryDirectory(prefix="hc-network-ui-",dir="/var/tmp") as scratch:
        home=Path(scratch);core=home/"core"
        env=H.isolated_env(home,core)
        (home/"config.toml").write_text('onboarding=false\n[experimental]\nallow_nested=true\n[keys]\ncopy_mode="f7"\ndetach="f10"\n')
        script=home/"fixture.py"
        script.write_text('''import os,sys,time,tty
tty.setraw(0)
for i in range(200):print(f"ROW_{i:04d} 中文 scroll fixture",end="\\r\\n")
print("NETWORK_READY",end="",flush=True)
n=0
while True:
    key=os.read(0,1)
    if not key:break
    if key==b"x":
        print("\\x1b[H"+chr(65+n)*12+" 中文",end="",flush=True);n+=1
    elif key==b"b":
        for i in range(400):
            print("\\r\\n"+str(i)+" "+chr(65+i%26)*65+" 中文",end="",flush=True)
            time.sleep(.002)
        print("\\r\\nBURST_END",end="",flush=True)
''')
        (core/"config/herdr-core").mkdir(parents=True)
        (core/"config/herdr-core/config.toml").write_text("shell="+json.dumps([sys.executable,"-u",str(script)])+"\nnotes=false\n")
        viewer=None
        def hc(*args):
            H.private_socket(env)
            p=subprocess.run([str(H.HC),"--json",*args],env=env,capture_output=True,text=True,check=True,timeout=15)
            return json.loads(p.stdout)
        def screen():
            data=viewer.read()
            p=subprocess.run([str(H.ROOT/"tests/core-tui/target/debug/screen-oracle"),"100","28"],input=data,capture_output=True,check=True,timeout=5)
            return p.stdout.decode(errors="replace")
        try:
            viewer=ImpairedViewer(env)
            H.until(lambda:b"NETWORK_READY" in viewer.read(),"ready over fragmented delayed output")
            address=hc("ls")["terminals"][0]["address"];pid=hc("get",address)["pid"]
            original_grid=(hc("get",address)["cols"],hc("get",address)["rows"])
            viewer.send(b"\x1b[18~")
            H.until(lambda:"copy" in screen().lower(),"copy mode across split escape input")
            viewer.send(b"\x1b[5~")
            H.until(lambda:"NETWORK_READY" not in screen() and "ROW_0" in screen(),"history scroll under delayed delivery")
            viewer.send(b"\x1b")
            H.until(lambda:"NETWORK_READY" in screen(),"copy exit restores live viewport")
            latencies=[]
            for index in range(12):
                offset=len(viewer.output);start=time.monotonic()
                viewer.send(b"x")
                H.until(lambda:bytes([65+index])*8 in viewer.read()[offset:],"ordered input response")
                latencies.append((time.monotonic()-start)*1000)
            viewer.send(b"b");pump(viewer,.20)
            viewer.paused_until=time.monotonic()+.65
            time.sleep(.65)  # Deliberately stop host reads while the owned fixture streams.
            wait_channel=Path(f"/proc/{viewer.pid}/wchan").read_text().strip()
            H.until(lambda:b"BURST_END" in viewer.read(),"slow-host backlog drains without geometry recovery")
            pump(viewer,.2)
            for index in range(8):
                offset=len(viewer.output)
                hc("label",address,"herdr.display_name="+str(index)*12)
                H.until(lambda:b"\x1b[?2026l" in viewer.read()[offset:],"metadata frame over delayed link")
            relay=SCHED.owned_relay(viewer,H.private_socket(env));assert relay
            offset=len(viewer.output);pidfd=os.pidfd_open(relay)
            try:
                assert SCHED.owned_relay(viewer,H.private_socket(env))==relay
                signal.pidfd_send_signal(pidfd,signal.SIGKILL)
            finally:os.close(pidfd)
            H.until(lambda:(new:=SCHED.owned_relay(viewer,H.private_socket(env))) and new!=relay,"new owned relay")
            H.until(lambda:b"\x1b[?2026l" in viewer.read()[offset:],"recovered epoch reaches delayed host")
            offset=len(viewer.output);viewer.send(b"x")
            H.until(lambda:b"M"*8 in viewer.read()[offset:],"input after relay replacement without split/resize/focus")
            facts=hc("get",address)
            assert facts["pid"]==pid
            assert (facts["cols"],facts["rows"])==original_grid
            assert viewer.peak<=32768
            viewer.detach()
            print(json.dumps({"nominal_rtt_ms":40,"jitter_ms":5,"fragment_sizes":[1,2,7,31,127,1024],
                "fragments":viewer.fragment_count,"input_samples":12,"input_median_ms":round(statistics.median(latencies),2),
                "input_max_ms":round(max(latencies),2),"slow_read_pause_seconds":.65,"owned_client_wait_channel":wait_channel,
                "max_transport_queue_bytes":viewer.peak,"scroll_copy_exit":True,"relay_recovery_after_metadata":True,
                "same_program_pid_and_grid":True,"manual_resize_focus_split":False,
                "scope":"userspace private PTY transport, not actual SSH/TCP or live Pi"}))
        finally:
            try:
                if viewer:viewer.cleanup()
            finally:H.stop_private_core(env)


if __name__=="__main__":main()
