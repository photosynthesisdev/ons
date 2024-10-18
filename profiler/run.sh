#!/bin/bash

# PID check
if [ -z "$1" ]; then
  echo "Usage: $0 <PID>"
  exit 1
fi

PID=$1
DURATION=10
FLAMEGRAPH_DIR="../FlameGraph"

echo "Profiling process with PID: $PID for $DURATION seconds..."

# Record perf data for the specified process
sudo perf record -F 99 -p "$PID" -g -- sleep $DURATION

# Generate stack traces from perf.data
echo "Generating stack traces..."
sudo perf script > out.perf

# Collapse sta
echo "Collapsing stack traces..."
"$FLAMEGRAPH_DIR/stackcollapse-perf.pl" out.perf > out.folded

# Generate the flame graph
echo "Generating flame graph..."
"$FLAMEGRAPH_DIR/flamegraph.pl" out.folded > flamegraph.svg

echo "Profiling complete. Flame graph generated as flamegraph.svg."
