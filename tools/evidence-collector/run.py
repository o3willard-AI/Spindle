#!/usr/bin/env python3
"""Entry point for the evidence collector."""

import sys
import os

# Add src to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "src"))

from evidence_collector.main import main

if __name__ == "__main__":
    main()
