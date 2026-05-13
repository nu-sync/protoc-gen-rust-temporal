#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "rich>=13",
#   "httpx>=0.27",
# ]
# ///
"""Dev orchestrator for the job-queue stack.

Commands:
    up        Start Temporal, worker, and api with cargo-watch.
    down      Stop everything started by `up`.
    temporal  Start only the local Temporal dev server.
    status    Print the state of all components.
    logs      Tail logs for managed components.
    smoke     Run a short health-check across the stack.
    e2e       Run scripts/e2e.sh.

Run via `uv run --script scripts/dev.py <command>`, or via `just dev`.
"""

from __future__ import annotations

import argparse
import os
import shutil
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

try:
    import httpx
    from rich.console import Console
    from rich.table import Table
except ImportError:  # pragma: no cover — uv resolves deps before exec
    sys.stderr.write("This script must be launched via `uv run --script`.\n")
    sys.exit(2)

ROOT = Path(__file__).resolve().parent.parent
PIDS_DIR = ROOT / ".dev-pids"
LOGS_DIR = ROOT / ".dev-logs"
console = Console()


@dataclass
class Service:
    name: str
    cmd: list[str]
    cwd: Path
    health: str | None = None  # URL probed by `status`

    @property
    def pid_file(self) -> Path:
        return PIDS_DIR / f"{self.name}.pid"

    @property
    def log_file(self) -> Path:
        return LOGS_DIR / f"{self.name}.log"

    def is_alive(self) -> bool:
        if not self.pid_file.exists():
            return False
        try:
            pid = int(self.pid_file.read_text().strip())
            os.kill(pid, 0)
            return True
        except (ProcessLookupError, ValueError):
            return False


SERVICES: list[Service] = [
    Service(
        name="worker",
        cmd=["cargo", "watch", "-q", "-c", "-x", "run -p job-worker"],
        cwd=ROOT,
    ),
    Service(
        name="api",
        cmd=["cargo", "watch", "-q", "-c", "-x", "run -p job-api"],
        cwd=ROOT,
        health="http://127.0.0.1:3030/health",
    ),
]

TEMPORAL = Service(
    name="temporal",
    cmd=[
        "temporal",
        "server",
        "start-dev",
        "--namespace",
        "default",
        "--ip",
        "127.0.0.1",
        "--port",
        "7233",
        "--ui-port",
        "8233",
    ],
    cwd=ROOT,
)


def _check_tools(*, stack: bool) -> None:
    missing = []
    if stack and shutil.which("cargo") is None:
        missing.append("cargo")
    if not _temporal_reachable() and shutil.which("temporal") is None:
        missing.append("temporal")
    if missing:
        console.print(f"[red]missing tools on PATH:[/red] {', '.join(missing)}")
        sys.exit(1)
    if stack and shutil.which("cargo-watch") is None:
        console.print("[yellow]installing cargo-watch …[/yellow]")
        subprocess.run(["cargo", "install", "cargo-watch", "--locked"], check=True)


def _temporal_reachable(host: str = "127.0.0.1", port: int = 7233, timeout_s: float = 0.5) -> bool:
    import socket

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(timeout_s)
        try:
            s.connect((host, port))
            return True
        except (TimeoutError, ConnectionRefusedError, OSError):
            return False


def _start_temporal() -> None:
    if _temporal_reachable():
        console.print(
            "[green]Temporal already reachable at 127.0.0.1:7233[/green] — reusing it"
        )
        return

    console.print("[cyan]starting Temporal via temporal server start-dev …[/cyan]")
    _spawn(TEMPORAL)
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if _temporal_reachable():
            break
        time.sleep(1)
    if not _temporal_reachable():
        console.print(
            "[yellow]Temporal frontend port 7233 still not accepting connections after 30s[/yellow]"
        )
        console.print(f"[yellow]see log:[/yellow] {TEMPORAL.log_file}")
        sys.exit(1)
    console.print("[green]Temporal up[/green] at http://localhost:7233 (UI :8233)")


def _stop_temporal() -> None:
    # Only tear down a Temporal that this script started. If another dev
    # server is already listening on :7233, leave it alone.
    if TEMPORAL.pid_file.exists():
        _stop(TEMPORAL)
    else:
        console.print("[dim]no managed Temporal process to stop (was reused)[/dim]")


def _spawn(svc: Service) -> None:
    PIDS_DIR.mkdir(exist_ok=True)
    LOGS_DIR.mkdir(exist_ok=True)
    if svc.is_alive():
        console.print(f"[yellow]{svc.name} already running[/yellow]")
        return
    log = svc.log_file.open("ab")
    proc = subprocess.Popen(
        svc.cmd,
        cwd=svc.cwd,
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    svc.pid_file.write_text(str(proc.pid))
    console.print(f"[green]started[/green] {svc.name} pid={proc.pid} → {svc.log_file}")


def _stop(svc: Service) -> None:
    if not svc.pid_file.exists():
        return
    try:
        pid = int(svc.pid_file.read_text().strip())
        # Kill the whole process group (cargo-watch + child cargo run).
        os.killpg(os.getpgid(pid), signal.SIGTERM)
    except (ProcessLookupError, ValueError):
        pass
    svc.pid_file.unlink(missing_ok=True)
    console.print(f"[red]stopped[/red] {svc.name}")


def cmd_up(args: argparse.Namespace) -> int:
    _check_tools(stack=True)
    _start_temporal()
    for svc in SERVICES:
        _spawn(svc)
    console.print("\n[bold]dev stack up.[/bold] tail logs with:")
    if TEMPORAL.pid_file.exists():
        console.print(f"  tail -f {TEMPORAL.log_file}")
    for svc in SERVICES:
        console.print(f"  tail -f {svc.log_file}")
    return 0


def cmd_temporal(args: argparse.Namespace) -> int:
    _check_tools(stack=False)
    _start_temporal()
    if TEMPORAL.pid_file.exists():
        console.print(f"tail logs with: tail -f {TEMPORAL.log_file}")
    else:
        console.print("[dim]using an external Temporal server; no managed log[/dim]")
    return 0


def cmd_down(args: argparse.Namespace) -> int:
    for svc in SERVICES:
        _stop(svc)
    _stop_temporal()
    return 0


def cmd_status(args: argparse.Namespace) -> int:
    table = Table(title="job-queue dev stack")
    table.add_column("component")
    table.add_column("state")
    table.add_column("detail")

    # Temporal: probe the frontend port directly. Reports "up" regardless of
    # whether this script started the server or an external dev server is
    # already running.
    temporal_state = "up" if _temporal_reachable() else "down"
    if temporal_state == "up" and TEMPORAL.is_alive():
        temporal_state = "up (managed)"
    elif temporal_state == "up":
        temporal_state = "up (external)"
    table.add_row("temporal", temporal_state, "tcp://127.0.0.1:7233 (UI :8233)")

    for svc in SERVICES:
        state = "up" if svc.is_alive() else "down"
        detail = svc.health or ""
        if svc.health and svc.is_alive():
            try:
                r = httpx.get(svc.health, timeout=1.0)
                detail = f"{svc.health} → {r.status_code}"
            except httpx.HTTPError as e:
                detail = f"{svc.health} → {type(e).__name__}"
        table.add_row(svc.name, state, detail)

    console.print(table)
    return 0


def cmd_smoke(args: argparse.Namespace) -> int:
    failures = 0
    for svc in SERVICES:
        if not svc.health:
            continue
        try:
            r = httpx.get(svc.health, timeout=2.0)
            ok = r.status_code == 200
            console.print(f"[{'green' if ok else 'red'}]{svc.name}:[/] {svc.health} → {r.status_code}")
            if not ok:
                failures += 1
        except httpx.HTTPError as e:
            console.print(f"[red]{svc.name}: {e}[/red]")
            failures += 1
    return 0 if failures == 0 else 1


def cmd_logs(args: argparse.Namespace) -> int:
    services = [TEMPORAL, *SERVICES]
    selected = (
        services
        if args.service == "all"
        else [s for s in services if s.name == args.service]
    )
    files = [s.log_file for s in selected if s.log_file.exists()]
    if not files:
        console.print("[yellow]no logs found; start the stack first[/yellow]")
        return 1
    return subprocess.run(["tail", "-f", *map(str, files)], cwd=ROOT).returncode


def cmd_e2e(args: argparse.Namespace) -> int:
    script = ROOT / "scripts/e2e.sh"
    return subprocess.run(["bash", str(script)], cwd=ROOT).returncode


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="dev stack orchestrator")
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("up").set_defaults(fn=cmd_up)
    sub.add_parser("temporal").set_defaults(fn=cmd_temporal)
    sub.add_parser("down").set_defaults(fn=cmd_down)
    sub.add_parser("status").set_defaults(fn=cmd_status)
    sub.add_parser("smoke").set_defaults(fn=cmd_smoke)
    logs = sub.add_parser("logs")
    logs.add_argument(
        "service",
        nargs="?",
        choices=["all", "temporal", "worker", "api"],
        default="all",
    )
    logs.set_defaults(fn=cmd_logs)
    sub.add_parser("e2e").set_defaults(fn=cmd_e2e)
    args = parser.parse_args(list(argv) if argv is not None else None)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
