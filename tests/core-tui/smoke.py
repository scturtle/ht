"""Run the private Core/TUI smoke gates serially, including after a failure."""
from pathlib import Path
import subprocess
import sys


def main(names):
    suffix = "-ui-smoke.py"
    cases = {
        path.name.removesuffix(suffix): path
        for path in sorted(Path(__file__).resolve().parent.glob(f"*{suffix}"))
    }
    if not cases:
        sys.exit("No smoke tests found")
    for name in names:
        if name not in cases:
            sys.exit(f"Unknown smoke test {name!r}. Available: {', '.join(cases)}")

    failed = []
    for name in names or cases:
        print(f"\n==> {name}", flush=True)
        if subprocess.run([sys.executable, cases[name]]).returncode:
            failed.append(name)
    count = len(names or cases)
    print(f"\nSmoke: {count - len(failed)}/{count} passed", flush=True)
    if failed:
        sys.exit(f"Failed: {', '.join(failed)}")


if __name__ == "__main__":
    main(sys.argv[1:])
