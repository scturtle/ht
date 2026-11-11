#!/usr/bin/env python3
"""Repeat two saved render-test executables on one CPU; no Core/PTY is started."""
import argparse
import json
import os
from pathlib import Path
import re
import statistics
import subprocess


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before",type=Path)
    parser.add_argument("after",type=Path)
    parser.add_argument("--cycles",type=int,default=3)
    parser.add_argument("--cpu",type=int,default=min(os.sched_getaffinity(0)))
    args=parser.parse_args()
    if not 1<=args.cycles<=10:parser.error("cycles must be 1..10")
    if args.cpu not in os.sched_getaffinity(0):parser.error("CPU is outside the allowed affinity")
    binaries={k:str(v.resolve(strict=True)) for k,v in {"before":args.before,"after":args.after}.items()}
    samples={k:{} for k in binaries}
    for _ in range(args.cycles):
        for side in ("before","after","after","before"):
            p=subprocess.run(["taskset","-c",str(args.cpu),binaries[side],"render_scale_profile_core",
                              "--ignored","--nocapture","--test-threads=1"],capture_output=True,text=True,timeout=60,check=True)
            found=re.findall(r"core_render_scale ([^\n]*?) mean_us=([\d.]+)",p.stdout+p.stderr)
            if not found:raise RuntimeError(f"{side} did not run the expected render benchmark")
            for case,value in found:samples[side].setdefault(case,[]).append(float(value))
    if samples["before"].keys()!=samples["after"].keys():raise RuntimeError("benchmark case sets differ")
    report={"cpu":args.cpu,"cycles":args.cycles,"method":"ABBA; median of per-run means; not runtime CPU or WAN latency",
            "binaries":binaries,"cases":{}}
    for case,before in samples["before"].items():
        after=samples["after"][case]
        if len(before)!=args.cycles*2 or len(after)!=args.cycles*2:raise RuntimeError("incomplete samples")
        a,b=statistics.median(before),statistics.median(after)
        report["cases"][case]={"before_us":a,"after_us":b,"change_percent":round((b/a-1)*100,2),
                              "before_samples":before,"after_samples":after}
    print(json.dumps(report,indent=2))


if __name__=="__main__":main()
