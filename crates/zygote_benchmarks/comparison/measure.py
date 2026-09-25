#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

"""Paired Linux build/link/size/direct-startup observations."""

import argparse
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parent
TARGET_ROOT = ROOT.parents[2] / "target" / "zygote-build-comparison"


def run(*args, env=None):
    return subprocess.run(args, check=True, env=env, capture_output=True, text=True)


def timed(*args, env=None):
    started = time.perf_counter()
    run(*args, env=env)
    return time.perf_counter() - started


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=100, help="direct startup repetitions")
    args = parser.parse_args()
    if args.runs < 1 or os.name != "posix" or not Path("/proc/self").exists():
        parser.error("requires Linux and at least one repetition")

    shutil.rmtree(TARGET_ROOT, ignore_errors=True)
    try:
        for package, prefix in [("zygote_bench_plain", "plain"), ("zygote_bench_wrapped", "wrapped")]:
            env = os.environ.copy()
            env["CARGO_TARGET_DIR"] = str(TARGET_ROOT / package)
            command = ("cargo", "build", "--manifest-path", str(ROOT / "Cargo.toml"), "--release", "-p", package)
            clean = timed(*command, env=env)
            incremental = timed(*command, env=env)
            source = ROOT / "shared" / "representative.rs"
            original_times = (source.stat().st_atime_ns, source.stat().st_mtime_ns)
            os.utime(source, ns=(original_times[0], time.time_ns()))
            try:
                source_change = timed(*command, env=env)
            finally:
                os.utime(source, ns=original_times)
            relink_samples = []
            for kind in ("minimal", "representative"):
                binary = Path(env["CARGO_TARGET_DIR"]) / "release" / f"{prefix}_{kind}"
                binary.unlink()
                relink_samples.append(timed(*command, env=env))
            print(
                f"{package} clean_build_s={clean:.3f} noop_incremental_build_s={incremental:.3f} "
                f"source_change_incremental_s={source_change:.3f} final_link_rebuild_s={statistics.median(relink_samples):.3f}"
            )
            for kind in ("minimal", "representative"):
                binary = Path(env["CARGO_TARGET_DIR"]) / "release" / f"{prefix}_{kind}"
                print(f"{package}/{kind} executable_bytes={binary.stat().st_size}")
                print(run("size", "-A", str(binary)).stdout.rstrip())
                samples = [timed(str(binary), env=env) for _ in range(args.runs)]
                print(f"{package}/{kind} direct_startup_median_us={statistics.median(samples) * 1e6:.0f} "
                      f"p95_us={sorted(samples)[int((len(samples) - 1) * .95)] * 1e6:.0f} n={args.runs}")
    finally:
        shutil.rmtree(TARGET_ROOT, ignore_errors=True)


if __name__ == "__main__":
    main()
