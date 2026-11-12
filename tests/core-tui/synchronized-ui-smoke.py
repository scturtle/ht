#!/usr/bin/env python3
"""Owned Core/PTY only: capture every completed ht frame during split sync output.
No model calls, no user sockets. Intentionally pause a simulated renderer between
clear and repaint to make a leaked intermediate frame observable in the byte tape.
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('sync_harness', ROOT / 'tests/core-tui/harness.py')
UI = importlib.util.module_from_spec(spec)
spec.loader.exec_module(UI)
BINARY = Path(os.environ.get('CORE_UI_HERDR', ROOT / 'target/release/ht')).resolve()
ORACLE = Path(os.environ.get('CORE_UI_ORACLE', ROOT / 'tests/core-tui/target/debug/screen-oracle'))
END = b'\x1b[?2026l'
OLD = 'BASELINE_STABLE_'
NEW = 'AFTER_COMPLETE_'

FIXTURE = r'''import json, os, sys, time, tty
from pathlib import Path
tty.setraw(0)
root=Path(sys.argv[1]); serial=0
def state(phase, **extra):
    pending=root/'phase.pending'
    pending.write_text(json.dumps(dict(phase=phase,serial=serial,time_ns=time.monotonic_ns(),**extra)))
    os.replace(pending,root/'phase.json')
def write(data):
    while data:
        n=os.write(1,data); data=data[n:]
def body(marker):
    size=os.get_terminal_size(1)
    rows=min(size.lines-2,16); cols=max(20,size.columns-2)
    return b''.join((f'\x1b[{r+1};1H'+(marker+chr(65+r)*cols)[:cols]).encode() for r in range(rows))
write(b'\x1b[?2026h\x1b[2J\x1b[H'+body('BASELINE_STABLE_')+b'\x1b[?25l\x1b[?2026l')
state('ready')
while True:
    command=os.read(0,1)
    if not command or command==b'q': break
    serial+=1
    if command==b'w':
        write(b'\x1b]7501;state=working:app=pi\x1b\\')
        state('working')
        continue
    if command in [b'd',b'e',b'k',b'j',b'c']:
        word={b'd':'done',b'e':'error',b'k':'blocked',b'j':'idle',b'c':'clear'}[command]
        write(f'\x1b]7501;state={word}:app=pi\x1b\\'.encode()+b'\x1b[0m')
        state('task_complete',word=word)
        continue
    if command==b'b':
        write(b'\x1b[?2026h\x1b[2J\x1b[H'+body('BASELINE_STABLE_')+b'\x1b[?2026l')
        state('baseline')
        continue
    if command==b'z': prefix=0; sync=False; inside=False
    elif command==b't': prefix=0; sync=True; inside=False
    elif command==b'i': prefix=65536; sync=True; inside=True
    else: prefix=[0,1024,4096,8192,16384,32768,65536,131072][int(command)]; sync=True; inside=False
    padding=b'\x1b[0m'*(prefix//4)
    head=b'\x1b[?2026h' if sync else b''
    packet=(head+padding if inside else padding+head)+b'\x1b[2J\x1b[H'
    if command==b't': packet+=body('TIMEOUT_PARTIAL_')
    write(packet)
    state('sync_open' if sync else 'plain_clear',prefix=prefix,inside=inside)
    # Timing is the tested variable: hold the program's clear/repaint transaction
    # open. The harness never resizes, focuses, attaches or injects recovery here.
    time.sleep(1.3 if command==b't' else .22)
    write(body('AFTER_COMPLETE_')+(b'\x1b[?2026l' if sync and command!=b't' else b''))
    state('complete',prefix=prefix,inside=inside)
'''


def main():
    root = Path(tempfile.mkdtemp(prefix='hc-sync-flash-', dir='/var/tmp'))
    root.chmod(0o700)
    os.chdir(root)
    core = root / 'core'
    env = UI.isolated_env(root, core)
    env.update(XDG_CONFIG_HOME=str(core/'config'), XDG_STATE_HOME=str(core/'state'))
    (root/'config.toml').write_text('[experimental]\nallow_nested=true\n[keys]\ndetach="f10"\n')
    fixture = root/'screen_fixture.py'
    fixture.write_text(FIXTURE)
    settings=core/'config/herdr-core'; settings.mkdir(parents=True)
    (settings/'config.toml').write_text('shell='+json.dumps([sys.executable,'-u',str(fixture),str(root)])+'\nnotes=false\n')
    viewer=None; outcome={'artifact_dir':str(root),'binary':str(BINARY),'hold_ms':220,'trials':[]}
    events=[]
    address=None
    program_killed=False
    def hc(*args):
        UI.private_socket(env)
        p=subprocess.run([str(UI.HC),'--json',*args],env=env,capture_output=True,text=True,timeout=15,check=True)
        return json.loads(p.stdout)
    def pump(seconds):
        until=time.monotonic()+seconds
        while time.monotonic()<until:
            before=len(viewer.output); viewer.read()
            if len(viewer.output)!=before: events.append({'time_ns':time.monotonic_ns(),'start':before,'end':len(viewer.output)})
    def phase():
        p=root/'phase.json'
        return json.loads(p.read_text()) if p.exists() else {}
    def waitphase(expected,serial):
        until=time.monotonic()+5
        while time.monotonic()<until:
            pump(.02)
            p=phase()
            if p.get('serial')==serial and p.get('phase')==expected:return p
        raise AssertionError((expected,serial,phase()))
    def screenshot(at=None):
        data=bytes(viewer.output if at is None else viewer.output[:at])
        p=subprocess.run([str(ORACLE),'100','28',json.dumps(viewer.resizes)],input=data,capture_output=True,timeout=5,check=True)
        return p.stdout.decode(errors='replace')
    try:
        viewer=UI.Viewer(env,BINARY)
        waitphase('ready',0);pump(.5)
        assert OLD in screenshot()
        terminals=hc('ls')['terminals']; assert len(terminals)==1
        address=terminals[0]['address']; before=hc('get',address)
        outcome.update(terminal=address,program_pid=before['pid'],initial=before,viewer_pid=viewer.pid,
                       socket=str(UI.private_socket(env)),resizes=viewer.resizes)
        serial=0
        # Plain clear is a positive observation control. No-prefix sync and
        # padding entirely inside sync are negative controls. Others try to leave
        # a previous non-synchronized render request pending in the worker.
        for kind in ['plain','sync_clean','sync_inside']+['sync_prefixed']*16+['sync_timeout']:
            viewer.send(b'b');serial+=1;waitphase('baseline',serial);pump(.16)
            assert OLD in screenshot()
            start=len(viewer.output)
            command={'plain':b'z','sync_clean':b'0','sync_inside':b'i','sync_timeout':b't'}.get(kind)
            if command is None: command=str(1+(len(outcome['trials'])-3)%7).encode()
            viewer.send(command);serial+=1
            opened=waitphase('plain_clear' if kind=='plain' else 'sync_open',serial)
            waitphase('complete',serial);pump(.13)
            stop=len(viewer.output); end_screen=screenshot()
            assert NEW in end_screen, (kind, end_screen)
            boundaries=[];cursor=start
            data=bytes(viewer.output)
            while True:
                at=data.find(END,cursor,stop)
                if at<0:break
                at+=len(END);boundaries.append(at);cursor=at
            frames=[]
            previous=start
            for index,at in enumerate(boundaries):
                screen=screenshot(at)
                status='old' if OLD in screen else ('new' if NEW in screen else ('partial' if 'TIMEOUT_PARTIAL_' in screen else 'neither'))
                frames.append(dict(end=at,state=status,bytes=at-previous,outer_ed2=b'\x1b[2J' in data[previous:at],
                                   received_ns=next(e['time_ns'] for e in events if e['end']>=at)))
                if status=='neither':(root/f'blank-{len(outcome["trials"]):02d}-{index:02d}.txt').write_text(screen)
                previous=at
            trial=dict(kind=kind,command=command.decode(),start=start,stop=stop,open=opened,frames=frames,
                       leaked=any(f['state']=='neither' for f in frames))
            outcome['trials'].append(trial)
            if kind=='plain': assert trial['leaked'], 'positive observation control must see clear'
            else: assert not trial['leaked'], f'intermediate blank in {kind}: {frames}'
            if kind=='sync_timeout':
                partial=next(f for f in frames if f['state']=='partial')
                delay=(partial['received_ns']-opened['time_ns'])/1e9
                assert .85<=delay<=1.25, f'paint hold timeout was not bounded: {delay}'
                outcome['timeout_observed_seconds']=delay
            print(json.dumps({'kind':kind,'command':command.decode(),'frames':frames,'leaked':trial['leaked']}),flush=True)
        # Real Pi completion emits a turn event whose state is NOT terminal
        # lifecycle state. Replay that contract without paid model calls. A
        # final ordinary DATA chunk must not be mistaken for data after CLOSE.
        def relay_children():
            return (Path('/proc')/str(viewer.pid)/'task'/str(viewer.pid)/'children').read_text().split()
        relays=relay_children()
        assert len(relays)==1, relays
        outcome['status_turns']=[]
        for command in [b'd',b'e',b'k',b'j',b'c']:
            viewer.send(b'b');serial+=1;waitphase('baseline',serial);pump(.12)
            start=len(viewer.output)
            viewer.send(b'w');serial+=1;waitphase('working',serial);pump(.12)
            viewer.send(command);serial+=1;done=waitphase('task_complete',serial);pump(.22)
            stop=len(viewer.output)
            cursor=start
            while True:
                at=bytes(viewer.output).find(END,cursor,stop)
                if at<0:break
                cursor=at+len(END)
                assert OLD in screenshot(cursor), f'blank pane at {done["word"]} completion'
            assert OLD in screenshot()
            assert relay_children()==relays, 'task completion must not reconnect the Core relay'
            outcome['status_turns'].append(done['word'])
        client_log=(core/'herdr-client.log').read_text()
        assert 'Core connection recovered' not in client_log, client_log
        after=hc('get',address)
        assert after['state']=='live'
        assert after['pid']==before['pid']
        assert (after['cols'],after['rows'])==(before['cols'],before['rows'])
        outcome['final']=after
        outcome['same_program_and_size']=True
        assert [tuple(r) for r in outcome['resizes']]==[(0,100,28)], 'no resize may rescue the test'
        viewer.detach()
        hc('kill',address)
        program_killed=True
    except BaseException as error:
        outcome['error']=repr(error)
        raise
    finally:
        if viewer:
            (root/'host-output.bin').write_bytes(viewer.output)
            (root/'host-events.json').write_text(json.dumps(events,indent=2))
            (root/'resizes.json').write_text(json.dumps(viewer.resizes))
            viewer.cleanup()
        if address is not None and not program_killed and UI.private_socket(env).exists():
            hc('kill',address)
        UI.stop_private_core(env)
        outcome['private_core_stopped']=not UI.private_socket(env).exists()
        outcome['sync_leaks']=sum(t['leaked'] and t['kind']!='plain' for t in outcome['trials'])
        (root/'result.json').write_text(json.dumps(outcome,indent=2))
        print(json.dumps({k:v for k,v in outcome.items() if k not in {'initial','trials'}}),flush=True)

if __name__=='__main__':main()
