#!/usr/bin/env python3
"""Measure the shipped pinned ILSpy NativeAOT engine through the real rxd CLI.

Uses the same acceptance workloads/report as dotnet.py, including sugar recovery,
original IL/EH checks, fresh-process latency and unaffected-backend measurements.
There is no external managed runner or removed cil_ir reconstruction baseline.
"""
from dotnet import main


if __name__ == "__main__":
    main()
